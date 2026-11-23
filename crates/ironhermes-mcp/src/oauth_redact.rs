//! One OAuth-aware detector and redactor shared by every surface that might see a
//! callback URL (54.8-16, D-15, T-54.8-49).
//!
//! Consumers: the CLI paste-back output (plan 02), the Telegram intercept and the
//! runner's inbound log line (plan 09), and [`crate::security::sanitize_error`].
//!
//! Decisions are made on DECODED query parameters, so a percent-encoded `code` or
//! `state` name (`%63ode`) is still a callback and still masked, matching how
//! [`crate::transport::oauth_state_from_url`] reads the same URL. A URL parser,
//! never a regex, makes every detection decision; the one regex here only masks text.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;
use url::Url;

/// Marker substituted for a sensitive value.
const REDACTED: &str = "[REDACTED]";

/// Query-parameter names (decoded, lowercased) whose values are masked.
const SENSITIVE_NAMES: &[&str] = &[
    "code",
    "state",
    "access_token",
    "refresh_token",
    "id_token",
    "client_secret",
    "code_verifier",
];

/// Characters stripped from both ends of a whitespace-delimited token before it is
/// parsed as a URL: chat clients wrap links in `<>` or quotes and prose adds
/// trailing punctuation.
fn is_wrapper_char(c: char) -> bool {
    matches!(
        c,
        '<' | '>'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '"'
            | '\''
            | '.'
            | ','
            | ';'
            | ':'
            | '!'
            | '?'
    )
}

/// Parse `text` as an http(s) URL on a loopback host (`127.0.0.1`, `localhost`, `[::1]`).
fn parse_loopback_url(text: &str) -> Option<Url> {
    let url = Url::parse(text.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let loopback = match url.host()? {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip == std::net::Ipv4Addr::LOCALHOST,
        url::Host::Ipv6(ip) => ip == std::net::Ipv6Addr::LOCALHOST,
    };
    loopback.then_some(url)
}

/// True when `url` carries a non-empty decoded value for `name`.
fn has_non_empty_param(url: &Url, name: &str) -> bool {
    url.query_pairs().any(|(k, v)| k == name && !v.is_empty())
}

/// True when `text` is an OAuth authorization callback: an http(s) URL on a
/// loopback host whose DECODED query carries a non-empty `code` and `state`.
pub fn is_oauth_callback_url(text: &str) -> bool {
    parse_loopback_url(text)
        .is_some_and(|u| has_non_empty_param(&u, "code") && has_non_empty_param(&u, "state"))
}

/// True when `text` is an OAuth denial callback: an http(s) loopback URL whose
/// DECODED query carries an `error` and a non-empty `state`.
pub fn is_oauth_denial(text: &str) -> bool {
    parse_loopback_url(text).is_some_and(|u| {
        u.query_pairs().any(|(k, _)| k == "error") && has_non_empty_param(&u, "state")
    })
}

/// Find the first callback or denial URL embedded in free text.
///
/// Splits on whitespace, strips wrapper characters (`<>()[]{}"'` and trailing
/// `.,;:!?`) from each token and returns the first token either detector accepts.
pub fn find_oauth_callback_in_text(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|token| token.trim_matches(is_wrapper_char))
        .find(|core| is_oauth_callback_url(core) || is_oauth_denial(core))
        .map(str::to_string)
}

/// Decoded, lowercased name of one raw query-parameter name.
fn decoded_name(raw_name: &str) -> String {
    url::form_urlencoded::parse(format!("{raw_name}=").as_bytes())
        .next()
        .map(|(k, _)| k.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_sensitive_name(decoded_lowercase: &str) -> bool {
    SENSITIVE_NAMES.contains(&decoded_lowercase)
}

/// Pass 1 for one URL-shaped core: mask sensitive pairs in its query, emitting the
/// DECODED name. Returns `None` when `core` is not a URL with a query or nothing
/// in it is sensitive.
fn redact_url_core(core: &str) -> Option<String> {
    let url = Url::parse(core).ok()?;
    url.query()?;
    let q = core.find('?')?;
    let (head, rest) = core.split_at(q + 1);
    let (query, fragment) = match rest.find('#') {
        Some(h) => rest.split_at(h),
        None => (rest, ""),
    };
    let mut changed = false;
    let rebuilt: Vec<Cow<'_, str>> = query
        .split('&')
        .map(|segment| match segment.split_once('=') {
            Some((raw_name, _)) => {
                let name = decoded_name(raw_name);
                if is_sensitive_name(&name) {
                    changed = true;
                    Cow::Owned(format!("{name}={REDACTED}"))
                } else {
                    Cow::Borrowed(segment)
                }
            }
            None => Cow::Borrowed(segment),
        })
        .collect();
    changed.then(|| format!("{head}{}{fragment}", rebuilt.join("&")))
}

/// Pass 1 for one whitespace-delimited token: strip wrappers, redact the URL core.
fn redact_token(token: &str) -> Option<String> {
    let lead = token.len() - token.trim_start_matches(is_wrapper_char).len();
    let after_lead = &token[lead..];
    let core = after_lead.trim_end_matches(is_wrapper_char);
    let redacted = redact_url_core(core)?;
    Some(format!(
        "{}{}{}",
        &token[..lead],
        redacted,
        &after_lead[core.len()..]
    ))
}

/// Pass 2 matcher: `name=value`, the value running to the next `&`, whitespace,
/// `#`, quote or angle bracket. The name class includes `%` so encoded names match.
fn pair_regex() -> &'static Regex {
    static PAIR: OnceLock<Regex> = OnceLock::new();
    PAIR.get_or_init(|| {
        Regex::new(r#"([A-Za-z0-9_%.\-]+)=([^&\s#"'<>]*)"#).expect("static regex is valid")
    })
}

/// Mask OAuth code, state and token values in free text.
///
/// Pass 1 rebuilds the query of every whitespace-delimited token that parses as a
/// URL with a query, emitting the decoded name and `[REDACTED]` for sensitive
/// values. Pass 2 sweeps the remaining text for `name=value` pairs whose decoded,
/// lowercased name is sensitive (code, state, access_token, refresh_token,
/// id_token, client_secret, code_verifier). Returns `Cow::Borrowed` when nothing
/// changed.
pub fn redact_oauth_params(text: &str) -> Cow<'_, str> {
    // Pass 1: URL-shaped tokens (whitespace preserved exactly).
    let mut pass1 = String::with_capacity(text.len());
    for piece in text.split_inclusive(char::is_whitespace) {
        let token = piece.trim_end_matches(char::is_whitespace);
        let whitespace = &piece[token.len()..];
        match redact_token(token) {
            Some(redacted) => pass1.push_str(&redacted),
            None => pass1.push_str(token),
        }
        pass1.push_str(whitespace);
    }

    // Pass 2: bare name=value pairs anywhere in the remaining text.
    let pass2 = pair_regex().replace_all(&pass1, |caps: &regex::Captures<'_>| {
        if is_sensitive_name(&decoded_name(&caps[1])) {
            format!("{}={REDACTED}", &caps[1])
        } else {
            caps[0].to_string()
        }
    });

    if pass2 == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(pass2.into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_detector_accepts_loopback_code_and_state() {
        for url in [
            "http://127.0.0.1:8765/callback?code=a&state=b",
            "http://localhost:9/cb?state=b&code=a",
            "http://[::1]:9/cb?code=a&state=b",
            "  http://127.0.0.1:8765/callback?code=a&state=b \n",
            "http://127.0.0.1:8765/callback?%63ode=a&%73tate=b",
        ] {
            assert!(is_oauth_callback_url(url), "must accept {url:?}");
        }
    }

    #[test]
    fn callback_detector_rejects_non_callbacks() {
        for text in [
            "https://example.com/?code=a&state=b",
            "http://127.0.0.1/cb?code=a",
            "http://127.0.0.1/cb?code=&state=b",
            "hello",
            "/mcp connect x",
            "",
        ] {
            assert!(!is_oauth_callback_url(text), "must reject {text:?}");
        }
    }

    #[test]
    fn denial_detector_requires_error_and_state() {
        assert!(is_oauth_denial(
            "http://127.0.0.1:8765/callback?error=access_denied&state=st-1"
        ));
        assert!(!is_oauth_denial(
            "http://127.0.0.1:8765/callback?error=access_denied"
        ));
    }

    #[test]
    fn finds_callback_embedded_in_prose() {
        assert_eq!(
            find_oauth_callback_in_text(
                "here you go: <http://127.0.0.1:8765/callback?code=cd-123&state=st-1>, thanks"
            )
            .as_deref(),
            Some("http://127.0.0.1:8765/callback?code=cd-123&state=st-1")
        );
        assert_eq!(find_oauth_callback_in_text("no url here"), None);
        assert_eq!(
            find_oauth_callback_in_text(
                "one http://127.0.0.1:1/cb?code=c1&state=s1 two http://localhost:2/cb?code=c2&state=s2"
            )
            .as_deref(),
            Some("http://127.0.0.1:1/cb?code=c1&state=s1")
        );
    }

    #[test]
    fn redactor_masks_oauth_values_in_free_text() {
        let out =
            redact_oauth_params("paste http://127.0.0.1:8765/callback?code=cd-123&state=st-1 now");
        assert!(out.contains("code=[REDACTED]"), "got {out}");
        assert!(out.contains("state=[REDACTED]"), "got {out}");
        assert!(!out.contains("cd-123"), "got {out}");
        assert!(!out.contains("st-1"), "got {out}");
        assert!(out.contains("paste") && out.contains("now"), "got {out}");
    }

    #[test]
    fn redactor_masks_percent_encoded_names() {
        let out = redact_oauth_params("http://127.0.0.1:8765/callback?%63ode=cd-123&%73tate=st-1");
        assert!(!out.contains("cd-123"), "got {out}");
        assert!(!out.contains("st-1"), "got {out}");
    }

    #[test]
    fn redactor_masks_token_params_case_insensitively() {
        let out = redact_oauth_params(
            "ACCESS_TOKEN=abc&refresh_token=def&id_token=ghi&client_secret=jkl",
        );
        for secret in ["abc", "def", "ghi", "jkl"] {
            assert!(!out.contains(secret), "{secret} leaked: {out}");
        }
    }

    #[test]
    fn redactor_borrows_when_nothing_to_redact() {
        let out = redact_oauth_params("hello world");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "hello world");
    }
}
