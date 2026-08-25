//! Content-type-aware prompt template rendering (D-14).
//!
//! Twilio posts `application/x-www-form-urlencoded` with `From`, `To` and
//! `Body` fields; a renderer that only understands JSON dotted paths makes
//! an entire provider unusable. Both branches ship together for that
//! reason: a `{From}` placeholder resolves against the parsed form map when
//! the request carried one, and a `{user.name}` placeholder resolves
//! against the JSON body by dotted path otherwise.
//!
//! A reference that resolves to nothing renders as an empty string rather
//! than failing the whole request — but a malformed template (unterminated
//! `{`, empty `{}`) returns [`TemplateError`] so a bad route config
//! surfaces at load time, not silently at first request.
//!
//! Every interpolated value, from either branch, is sanitized before it
//! reaches the prompt: newlines/carriage-returns stripped and the value
//! truncated to a bounded length — the same treatment
//! `ApprovalCoordinator` already applies to interpolated text
//! (`crates/ironhermes-gateway/src/approval.rs:244-263`). Webhook payload
//! content is attacker-controlled and this is the injection boundary; the
//! boundary does not care which parser produced the string.

use std::collections::HashMap;

/// Bound on any single interpolated value, in `char`s, after sanitization.
/// Matches the treatment `ApprovalCoordinator` already applies.
const MAX_INTERPOLATED_VALUE_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    Malformed(String),
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::Malformed(msg) => write!(f, "malformed prompt template: {msg}"),
        }
    }
}

impl std::error::Error for TemplateError {}

/// The payload representation a template renders against. `Empty` covers
/// the case of an unrecognised content type — placeholders resolve to
/// nothing (empty string) rather than erroring.
#[derive(Debug, Clone, Copy)]
pub enum PayloadView<'a> {
    Form(&'a HashMap<String, String>),
    Json(&'a serde_json::Value),
    Empty,
}

/// Render `template`, substituting every `{key}` placeholder by resolving
/// `key` against `payload` (content-type-aware — see module doc). Returns
/// [`TemplateError`] only for a malformed template (unterminated `{`, or an
/// empty `{}`), never for an unresolvable key.
pub fn render(template: &str, payload: &PayloadView<'_>) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    let mut i = 0usize;

    while i < template.len() {
        match template[i..].find('{') {
            None => {
                out.push_str(&template[i..]);
                break;
            }
            Some(rel_open) => {
                let open = i + rel_open;
                out.push_str(&template[i..open]);

                let Some(rel_close) = template[open + 1..].find('}') else {
                    return Err(TemplateError::Malformed(format!(
                        "unterminated '{{' at byte offset {open}"
                    )));
                };
                let close = open + 1 + rel_close;
                let key = &template[open + 1..close];
                if key.is_empty() {
                    return Err(TemplateError::Malformed(format!(
                        "empty placeholder '{{}}' at byte offset {open}"
                    )));
                }

                let resolved = resolve_key(key, payload).unwrap_or_default();
                out.push_str(&sanitize(&resolved));

                i = close + 1;
            }
        }
    }

    Ok(out)
}

fn resolve_key(key: &str, payload: &PayloadView<'_>) -> Option<String> {
    match payload {
        PayloadView::Form(map) => map.get(key).cloned(),
        PayloadView::Json(value) => {
            let mut cur = *value;
            for part in key.split('.') {
                cur = cur.get(part)?;
            }
            match cur {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            }
        }
        PayloadView::Empty => None,
    }
}

/// D-14: strip newlines/CR and truncate to a bounded length. Applied to
/// every interpolated value regardless of which branch produced it —
/// injection boundary, not a parser concern.
fn sanitize(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
        .chars()
        .take(MAX_INTERPOLATED_VALUE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_placeholder_resolves() {
        let mut form = HashMap::new();
        form.insert("From".to_string(), "+15551234567".to_string());
        form.insert("Body".to_string(), "hello there".to_string());
        let payload = PayloadView::Form(&form);
        let rendered = render("From {From}: {Body}", &payload).unwrap();
        assert_eq!(rendered, "From +15551234567: hello there");
    }

    #[test]
    fn json_dotted_path_resolves() {
        let value: serde_json::Value = serde_json::json!({"user": {"name": "Ada"}});
        let payload = PayloadView::Json(&value);
        let rendered = render("Hello {user.name}", &payload).unwrap();
        assert_eq!(rendered, "Hello Ada");
    }

    #[test]
    fn unresolvable_key_renders_empty_string_not_error() {
        let value: serde_json::Value = serde_json::json!({});
        let payload = PayloadView::Json(&value);
        let rendered = render("Hello {user.name}!", &payload).unwrap();
        assert_eq!(rendered, "Hello !");
    }

    #[test]
    fn form_takes_precedence_when_present_json_ignored() {
        let mut form = HashMap::new();
        form.insert("Body".to_string(), "form value".to_string());
        let payload = PayloadView::Form(&form);
        let rendered = render("{Body}", &payload).unwrap();
        assert_eq!(rendered, "form value");
    }

    #[test]
    fn unterminated_brace_is_malformed_template_error() {
        let payload = PayloadView::Empty;
        let err = render("hello {name", &payload).unwrap_err();
        assert!(matches!(err, TemplateError::Malformed(_)));
    }

    #[test]
    fn empty_placeholder_is_malformed_template_error() {
        let payload = PayloadView::Empty;
        let err = render("hello {}", &payload).unwrap_err();
        assert!(matches!(err, TemplateError::Malformed(_)));
    }

    #[test]
    fn interpolated_newlines_stripped_and_truncated() {
        let mut form = HashMap::new();
        let long_with_newlines = format!("line1\nline2\r\n{}", "x".repeat(2500));
        form.insert("Body".to_string(), long_with_newlines);
        let payload = PayloadView::Form(&form);
        let rendered = render("{Body}", &payload).unwrap();
        assert!(!rendered.contains('\n'));
        assert!(!rendered.contains('\r'));
        assert_eq!(rendered.chars().count(), MAX_INTERPOLATED_VALUE_CHARS);
    }

    #[test]
    fn percent_encoded_space_and_plus_decode_exactly_once() {
        // Simulates what url::form_urlencoded::parse already decoded — this
        // test asserts the template layer does not re-decode.
        let mut form = HashMap::new();
        form.insert("q".to_string(), "a b+c".to_string());
        let payload = PayloadView::Form(&form);
        let rendered = render("{q}", &payload).unwrap();
        assert_eq!(rendered, "a b+c");
    }
}
