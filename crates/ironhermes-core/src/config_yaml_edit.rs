//! Pure text functions for the Phase 54.13 raw `config.yaml` web editor.
//!
//! No I/O, no globals. The web server uses these to keep inline secrets out of
//! the browser (D-07/D-08/D-09/D-09a) and to validate submitted text.
//!
//! The safety argument for D-07 is structural, not textual: [`mask`] builds a
//! candidate masked text and then re-parses BOTH the original and the
//! candidate as YAML, and refuses (fail closed, naming only the key path) if
//! any secret-named scalar in the original is not exactly [`PLACEHOLDER`] in
//! the candidate. That verification is the guarantee; the candidate builder
//! is only an optimisation that lets more files load.

use serde_yaml::Value;

/// What a hidden secret value looks like in the editor. A valid YAML plain
/// scalar, so a masked file still parses.
pub const PLACEHOLDER: &str = "•••• (kept)";

/// D-09 / D-09a / D-09b: whether a mapping key names a secret. The name is
/// lowercased with `-` mapped to `_` first (MCP header `X-API-Key`).
pub fn is_secret_key(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace('-', "_");
    // `*_env` names an env var, not a secret; `record_key` is a keybinding.
    if n.ends_with("_env") || n == "record_key" {
        return false;
    }
    if matches!(
        n.as_str(),
        "password_hash" | "authorization" | "cookie" | "key"
    ) {
        return true;
    }
    [
        "api_key",
        "_key",
        "token",
        "secret",
        "password",
        "passphrase",
        "credential",
        "credentials",
        "dsn",
        "connection_string",
    ]
    .iter()
    .any(|suffix| n.ends_with(suffix))
}

/// D-09b(4): whether `s` contains URL userinfo with a password
/// (`scheme://user:pass@host`), i.e. `://[^/@:\s]+:[^/@\s]+@`.
fn has_url_password(s: &str) -> bool {
    s.match_indices("://").any(|(i, _)| {
        let rest = &s[i + 3..];
        let user = rest
            .find(|c: char| matches!(c, '/' | '@' | ':') || c.is_whitespace())
            .unwrap_or(rest.len());
        if user == 0 || !rest[user..].starts_with(':') {
            return false;
        }
        let after = &rest[user + 1..];
        let pass = after
            .find(|c: char| matches!(c, '/' | '@') || c.is_whitespace())
            .unwrap_or(after.len());
        pass > 0 && after[pass..].starts_with('@')
    })
}

/// Why a file could not be masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaskError {
    /// A secret-named scalar sits in a layout the editor cannot hide.
    /// Carries the dotted key path only, never the value.
    Unsupported { key_path: String },
    /// The text is not YAML, so its secrets cannot be verified hidden.
    NotYaml,
}

/// Replace secret values with [`PLACEHOLDER`], failing closed.
pub fn mask(text: &str) -> Result<String, MaskError> {
    let mut candidate = text.to_string();
    for span in scan(text).iter().rev() {
        if span.secret && !is_nullish(&span.raw) {
            candidate.replace_range(span.value.clone(), PLACEHOLDER);
        }
    }
    verify_masked(text, &candidate)?;
    Ok(candidate)
}

/// A submitted placeholder that has no original to restore (D-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnmaskError {
    Unmatched { key_path: String },
}

/// Turn submitted editor text back into file text. The placeholder text must
/// never reach disk: any leftover placeholder is refused with its key path.
pub fn unmask(submitted: &str, on_disk: &str) -> Result<String, UnmaskError> {
    let originals = scan(on_disk);
    let mut candidate = submitted.to_string();
    let submitted_spans = scan(submitted);
    for span in submitted_spans.iter().rev() {
        if strip_quotes(&span.raw) != PLACEHOLDER {
            continue;
        }
        let unmatched = || UnmaskError::Unmatched {
            key_path: span.key_path.clone(),
        };
        let original = originals
            .iter()
            .find(|o| o.secret && o.key_path == span.key_path)
            .ok_or_else(unmatched)?;
        // A list position is not an identity: restore only into an item whose
        // non-secret scalars are unchanged, so a key never lands in another entry.
        if !list_items_unchanged(&span.key_path, &submitted_spans, &originals) {
            return Err(unmatched());
        }
        candidate.replace_range(span.value.clone(), &original.raw);
    }
    verify_no_placeholder(&candidate)?;
    Ok(candidate)
}

/// For every `[i]` in `key_path`, the non-secret scalars under that item prefix
/// must be identical (path and raw token) in both documents.
fn list_items_unchanged(key_path: &str, submitted: &[ScalarSpan], on_disk: &[ScalarSpan]) -> bool {
    let non_secret = |spans: &[ScalarSpan], prefix: &str| -> Vec<(String, String)> {
        spans
            .iter()
            .filter(|s| {
                !s.secret
                    && s.key_path
                        .strip_prefix(prefix)
                        .is_some_and(|r| r.is_empty() || r.starts_with('.') || r.starts_with('['))
            })
            .map(|s| (s.key_path.clone(), s.raw.clone()))
            .collect()
    };
    key_path.match_indices(']').all(|(i, _)| {
        let prefix = &key_path[..=i];
        non_secret(submitted, prefix) == non_secret(on_disk, prefix)
    })
}

/// The placeholder text must never be written to disk: refuse any key or
/// string scalar still containing it, naming the key path only.
fn verify_no_placeholder(candidate: &str) -> Result<(), UnmaskError> {
    // Text that does not parse is reported by validation and never written.
    let Ok(doc) = serde_yaml::from_str::<Value>(candidate) else {
        return Ok(());
    };
    match find_placeholder(&doc, "") {
        Some(key_path) => Err(UnmaskError::Unmatched { key_path }),
        None => Ok(()),
    }
}

fn find_placeholder(v: &Value, path: &str) -> Option<String> {
    match untag(v) {
        Value::String(s) if s.contains(PLACEHOLDER) => Some(path.to_string()),
        Value::Mapping(m) => m.iter().find_map(|(k, child)| {
            let seg = key_segment(k);
            let child_path = if path.is_empty() {
                seg.clone()
            } else {
                format!("{path}.{seg}")
            };
            if seg.contains(PLACEHOLDER) {
                return Some(child_path);
            }
            find_placeholder(child, &child_path)
        }),
        Value::Sequence(seq) => seq
            .iter()
            .enumerate()
            .find_map(|(i, item)| find_placeholder(item, &format!("{path}[{i}]"))),
        _ => None,
    }
}

/// Submitted text that unmasked and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub text: String,
    pub unknown_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    Unmatched { key_path: String },
    Invalid(crate::config::YamlIssue),
}

/// [`unmask`] then [`crate::config::Config::validate_yaml_str`].
pub fn prepare(submitted: &str, on_disk: &str) -> Result<Prepared, PrepareError> {
    let text = unmask(submitted, on_disk)
        .map_err(|UnmaskError::Unmatched { key_path }| PrepareError::Unmatched { key_path })?;
    let unknown_keys = crate::config::Config::validate_yaml_str(&text).map_err(|mut issue| {
        // serde messages quote the offending value, which may be a secret.
        let mut values = secret_values(on_disk);
        values.extend(secret_values(&text));
        issue.message = scrub(&issue.message, &values);
        PrepareError::Invalid(issue)
    })?;
    Ok(Prepared { text, unknown_keys })
}

/// Secret scalar values in `text`, unquoted, for scrubbing messages.
pub fn secret_values(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in scan(text).iter().filter(|s| s.secret) {
        let value = strip_quotes(&span.raw);
        // Shorter values carry no secrecy and replacing them would mangle the message.
        if is_nullish(&span.raw) || value.chars().count() < 4 || out.contains(&value) {
            continue;
        }
        out.push(value);
    }
    out
}

/// Replace every occurrence of each secret in `message` with [`PLACEHOLDER`].
pub fn scrub(message: &str, secrets: &[String]) -> String {
    let mut ordered: Vec<&String> = secrets.iter().filter(|s| !s.is_empty()).collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    ordered.into_iter().fold(message.to_string(), |msg, s| {
        msg.replace(s.as_str(), PLACEHOLDER)
    })
}

/// One single-line scalar value found by [`scan`].
struct ScalarSpan {
    /// Dotted key path, sequence items as `[i]` (same shape as the verify walk).
    key_path: String,
    /// Absolute byte range of the value token, quotes included.
    value: std::ops::Range<usize>,
    /// The value token exactly as written.
    raw: String,
    /// Whether [`mask`] must hide this value.
    secret: bool,
}

fn is_nullish(raw: &str) -> bool {
    matches!(raw, "~" | "null" | "Null" | "NULL" | "\"\"" | "''")
}

/// A value token without its surrounding quotes (`''` collapsed in single quotes).
fn strip_quotes(raw: &str) -> String {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        raw[1..raw.len() - 1].to_string()
    } else if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        raw[1..raw.len() - 1].replace("''", "'")
    } else {
        raw.to_string()
    }
}

/// An open block container on the scanner's indentation stack.
struct Level {
    /// Column of the opening key (or of the `- ` of an item).
    indent: usize,
    path: String,
    /// An anonymous `- ` item: popped by any line at or left of its column.
    is_item: bool,
    /// Every scalar beneath is secret (D-09b(3)).
    secret: bool,
    /// Next `[i]` index for `- ` items opened directly under this level.
    items: usize,
}

/// What follows `key:` or `- `.
enum Tail {
    /// Nothing (or a comment): a nested block starts on the next lines.
    Block,
    /// Block scalar, flow collection or unclosed quote: no span, skip the body.
    Skip,
    /// `*alias`: no span.
    Alias,
    /// A one-line scalar: (start, length) within the tail, quotes included.
    Scalar(usize, usize),
}

fn classify_tail(rest: &str) -> Tail {
    let mut r = rest.trim_start_matches(' ');
    // Skip `&anchor` / `!tag` prefixes; the value token follows them.
    while r.starts_with('&') || r.starts_with('!') {
        let end = r.find(' ').unwrap_or(r.len());
        r = r[end..].trim_start_matches(' ');
    }
    if r.is_empty() || r.starts_with('#') {
        return Tail::Block;
    }
    match value_token(r) {
        Some(len) => Tail::Scalar(rest.len() - r.len(), len),
        None if r.starts_with('*') => Tail::Alias,
        None => Tail::Skip,
    }
}

/// Split `content` (no terminator) at a `key:` into (key, byte offset after the colon).
fn parse_key(content: &str) -> Option<(String, usize)> {
    let b = content.as_bytes();
    let (key, end) = match b.first()? {
        b'"' => {
            let mut i = 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            (content.get(1..i)?.to_string(), i + 1)
        }
        b'\'' => {
            let mut i = 1;
            while i < b.len() && !(b[i] == b'\'' && b.get(i + 1) != Some(&b'\'')) {
                i += if b[i] == b'\'' { 2 } else { 1 };
            }
            (content.get(1..i)?.replace("''", "'"), i + 1)
        }
        b'{' | b'[' | b'&' | b'*' | b'!' | b'|' | b'>' | b'%' | b'@' | b'`' | b'#' => return None,
        _ => {
            let mut i = 0;
            loop {
                let at = i + content.get(i..)?.find(':')?;
                if matches!(b.get(at + 1), None | Some(b' ')) {
                    break (content[..at].trim_end().to_string(), at);
                }
                i = at + 1;
            }
        }
    };
    if b.get(end) != Some(&b':') || !matches!(b.get(end + 1), None | Some(b' ')) {
        return None;
    }
    Some((key, end + 1))
}

/// Byte length of the single-line value token at the start of `rest`, or
/// `None` when it is not a one-line scalar (flow collection, block scalar,
/// alias, or a quote that does not close on this line).
fn value_token(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    match b.first()? {
        b'"' => {
            let mut i = 1;
            while i < b.len() {
                match b[i] {
                    b'\\' => i += 2,
                    b'"' => return Some(i + 1),
                    _ => i += 1,
                }
            }
            None
        }
        b'\'' => {
            let mut i = 1;
            while i < b.len() {
                if b[i] == b'\'' {
                    if b.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    return Some(i + 1);
                }
                i += 1;
            }
            None
        }
        b'{' | b'[' | b'|' | b'>' | b'*' => None,
        _ => {
            let end = rest.find(" #").unwrap_or(rest.len());
            Some(rest[..end].trim_end().len())
        }
    }
}

/// Line scanner: find every single-line `key: value` scalar with its key path
/// and byte range. Not a YAML parser: whatever it cannot read yields no span,
/// and [`verify_masked`] then refuses the file.
fn scan(text: &str) -> Vec<ScalarSpan> {
    let mut spans = Vec::new();
    let mut stack: Vec<Level> = Vec::new();
    // Top-level `- ` items (no parent key) are counted here.
    let mut root_items = 0;
    // After a block scalar / flow collection / unclosed quote: lines indented
    // deeper than this column belong to it and are not scanned.
    let mut skip_deeper_than: Option<usize> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let indent = content.len() - content.trim_start_matches(' ').len();
        let body = &content[indent..];
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if let Some(n) = skip_deeper_than {
            if indent > n {
                continue;
            }
            skip_deeper_than = None;
        }
        if matches!(content.trim_end(), "---" | "...") {
            stack.clear();
            root_items = 0;
            continue;
        }
        // Tabs and `? ` complex keys are not read: no span, `verify_masked` decides.
        if body.starts_with('\t') || body.starts_with("? ") {
            continue;
        }
        let dash = body == "-" || body.starts_with("- ");
        while let Some(top) = stack.last() {
            let pop = if top.is_item {
                top.indent >= indent
            } else {
                top.indent > indent || (top.indent == indent && !dash)
            };
            if !pop {
                break;
            }
            stack.pop();
        }
        // Consume every leading `- ` (nested sequences), opening an item level each.
        let mut rest = body;
        while rest == "-" || rest.starts_with("- ") {
            let col = content.len() - rest.len();
            let (parent_path, parent_secret, index) = match stack.last_mut() {
                Some(p) => {
                    p.items += 1;
                    (p.path.clone(), p.secret, p.items - 1)
                }
                None => {
                    root_items += 1;
                    (String::new(), false, root_items - 1)
                }
            };
            stack.push(Level {
                indent: col,
                path: format!("{parent_path}[{index}]"),
                is_item: true,
                secret: parent_secret,
                items: 0,
            });
            rest = rest[1..].trim_start_matches(' ');
        }
        let in_item = rest.len() != body.len();
        if rest.is_empty() || rest.starts_with('#') {
            continue;
        }
        let col = content.len() - rest.len();
        let (parent_path, parent_secret) = stack
            .last()
            .map_or((String::new(), false), |l| (l.path.clone(), l.secret));
        let (key, tail, key_path, secret_key) = match parse_key(rest) {
            Some((key, after)) => {
                let key_path = if parent_path.is_empty() {
                    key.clone()
                } else {
                    format!("{parent_path}.{key}")
                };
                let secret_key = parent_secret || is_secret_key(&key);
                (key, &rest[after..], key_path, secret_key)
            }
            // A bare scalar item (`- value`); otherwise a continuation line.
            None if in_item => (String::new(), rest, parent_path, parent_secret),
            None => continue,
        };
        match classify_tail(tail) {
            Tail::Block => {
                if !key.is_empty() {
                    stack.push(Level {
                        indent: col,
                        path: key_path,
                        is_item: false,
                        secret: secret_key,
                        items: 0,
                    });
                }
            }
            Tail::Skip => skip_deeper_than = Some(if key.is_empty() { indent } else { col }),
            Tail::Alias => {}
            Tail::Scalar(start, len) => {
                let raw = &tail[start..start + len];
                let abs = line_start + (content.len() - tail.len()) + start;
                spans.push(ScalarSpan {
                    secret: secret_key || has_url_password(&strip_quotes(raw)),
                    key_path,
                    value: abs..abs + len,
                    raw: raw.to_string(),
                });
            }
        }
    }
    spans
}

/// The whole safety argument for D-07: every non-empty secret-named scalar in
/// `original` must be exactly [`PLACEHOLDER`] at the same path in `masked`.
fn verify_masked(original: &str, masked: &str) -> Result<(), MaskError> {
    let orig: Value = serde_yaml::from_str(original).map_err(|_| MaskError::NotYaml)?;
    let masked: Value = serde_yaml::from_str(masked).map_err(|_| MaskError::Unsupported {
        key_path: "<document>".to_string(),
    })?;
    walk(&orig, Some(&masked), "", false)
}

fn untag(v: &Value) -> &Value {
    match v {
        Value::Tagged(t) => untag(&t.value),
        other => other,
    }
}

fn key_segment(k: &Value) -> String {
    match untag(k) {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => "?".to_string(),
    }
}

fn is_nonempty_scalar(v: &Value) -> bool {
    match untag(v) {
        Value::String(s) => !s.is_empty(),
        Value::Number(_) | Value::Bool(_) => true,
        _ => false,
    }
}

/// `hide` is true under a secret-named key (D-09b(3)): every non-empty scalar
/// beneath it must be hidden. Independently, any string scalar carrying URL
/// userinfo with a password must be hidden (D-09b(4)).
fn walk(orig: &Value, masked: Option<&Value>, path: &str, hide: bool) -> Result<(), MaskError> {
    let masked = masked.map(untag);
    match untag(orig) {
        Value::Mapping(m) => {
            for (k, v) in m {
                let seg = key_segment(k);
                let child_path = if path.is_empty() {
                    seg
                } else {
                    format!("{path}.{seg}")
                };
                let masked_child = masked.and_then(Value::as_mapping).and_then(|mm| mm.get(k));
                let child_hide =
                    hide || matches!(untag(k), Value::String(name) if is_secret_key(name));
                walk(v, masked_child, &child_path, child_hide)?;
            }
            Ok(())
        }
        Value::Sequence(seq) => {
            for (i, item) in seq.iter().enumerate() {
                let masked_child = masked.and_then(Value::as_sequence).and_then(|ms| ms.get(i));
                walk(item, masked_child, &format!("{path}[{i}]"), hide)?;
            }
            Ok(())
        }
        scalar => {
            let url_secret = matches!(scalar, Value::String(s) if has_url_password(s));
            if (hide && is_nonempty_scalar(scalar)) || url_secret {
                let ok = matches!(masked, Some(Value::String(s)) if s == PLACEHOLDER);
                if !ok {
                    return Err(MaskError::Unsupported {
                        key_path: path.to_string(),
                    });
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_secret_key_matches_d09_suffixes_case_insensitive() {
        for k in [
            "api_key",
            "OPENAI_API_KEY",
            "bot_token",
            "client_secret",
            "Password",
            "passphrase",
        ] {
            assert!(is_secret_key(k), "{k} must be secret");
        }
        for k in ["max_tokens", "model", "token_budget"] {
            assert!(!is_secret_key(k), "{k} must not be secret");
        }
    }

    #[test]
    fn is_secret_key_keeps_env_name_keys_visible() {
        for k in ["api_key_env", "token_env", "BOT_TOKEN_ENV"] {
            assert!(!is_secret_key(k), "{k} names an env var, not a secret");
        }
    }

    #[test]
    fn is_secret_key_matches_d09a_exact_names() {
        for k in ["password_hash", "Authorization", "COOKIE"] {
            assert!(is_secret_key(k), "{k} must be secret");
        }
        for k in ["password_hash_algo", "cookies_enabled"] {
            assert!(!is_secret_key(k), "{k} must not be secret");
        }
    }

    #[test]
    fn mask_passes_secret_free_text_through_unchanged() {
        let text =
            "# a comment\nmodel:\n  default: gpt-5 # trailing\nagent:\n  max_iterations: 5\n";
        assert_eq!(mask(text).as_deref(), Ok(text));
    }

    #[test]
    fn mask_refuses_flow_mapping_secret_naming_the_key_path() {
        let text = "providers: {openai: {api_key: sk-flow-1}}\n";
        assert_eq!(
            mask(text),
            Err(MaskError::Unsupported {
                key_path: "providers.openai.api_key".to_string()
            })
        );
    }

    #[test]
    fn mask_refuses_block_scalar_secret() {
        let text = "api_key: |\n  sk-block-1\n";
        assert_eq!(
            mask(text),
            Err(MaskError::Unsupported {
                key_path: "api_key".to_string()
            })
        );
    }

    #[test]
    fn mask_never_returns_a_block_mapping_secret_in_plaintext() {
        let text = "providers:\n  openai:\n    api_key: sk-test-123\n";
        match mask(text) {
            Err(_) => {}
            Ok(masked) => assert!(!masked.contains("sk-test-123"), "leaked: {masked}"),
        }
    }

    #[test]
    fn mask_leaves_null_and_empty_secret_values_visible() {
        for text in ["api_key: ~\n", "token: \"\"\n", "secret:\n"] {
            assert_eq!(mask(text).as_deref(), Ok(text), "{text:?}");
        }
    }

    #[test]
    fn mask_refuses_text_that_is_not_yaml() {
        assert_eq!(mask("a: [unbalanced\n"), Err(MaskError::NotYaml));
    }

    #[test]
    fn unmask_refuses_placeholder_at_path_with_no_disk_secret() {
        let submitted = format!("notes: {PLACEHOLDER}\n");
        assert_eq!(
            unmask(&submitted, ""),
            Err(UnmaskError::Unmatched {
                key_path: "notes".to_string()
            })
        );
    }

    #[test]
    fn prepare_returns_unknown_keys_for_valid_text() {
        let prepared = prepare("bogus: 1\n", "").expect("valid text");
        assert_eq!(prepared.unknown_keys, vec!["bogus".to_string()]);
        assert_eq!(prepared.text, "bogus: 1\n");
    }

    #[test]
    fn prepare_reports_invalid_issue() {
        match prepare("agent:\n  max_iterations: notanumber\n", "") {
            Err(PrepareError::Invalid(issue)) => assert!(issue.line.is_some()),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    const FIXTURE_BLOCK: &str = "# header comment\n\nproviders:\n  openai:\n    base_url: https://x\n    api_key: \"sk-q-9\"\n\n  other:\n    api_key: sk-plain-2\n    model: gpt-5 # keep\nagent:\n  max_iterations: 5\n";

    #[test]
    fn mask_replaces_block_mapping_secret_with_placeholder() {
        let text = "providers:\n  openai:\n    base_url: https://x\n    api_key: sk-test-123\n";
        let masked = mask(text).expect("block mapping masks");
        assert!(
            masked.contains(&format!("    api_key: {PLACEHOLDER}\n")),
            "{masked}"
        );
        assert!(masked.contains("    base_url: https://x\n"), "{masked}");
        assert!(!masked.contains("sk-test-123"), "{masked}");
    }

    #[test]
    fn mask_unmask_round_trip_is_byte_identical() {
        let masked = mask(FIXTURE_BLOCK).expect("masks");
        assert!(!masked.contains("sk-q-9") && !masked.contains("sk-plain-2"));
        assert_eq!(unmask(&masked, FIXTURE_BLOCK).as_deref(), Ok(FIXTURE_BLOCK));
    }

    #[test]
    fn unmask_restores_original_raw_token_including_quotes() {
        let on_disk = "api_key: \"sk-q-9\"\n";
        let submitted = format!("api_key: {PLACEHOLDER}\n");
        assert_eq!(unmask(&submitted, on_disk).as_deref(), Ok(on_disk));
    }

    #[test]
    fn unmask_typed_value_replaces_secret() {
        let out = unmask("api_key: sk-new\n", "api_key: sk-old\n").expect("typed value");
        assert!(out.contains("sk-new") && !out.contains("sk-old"), "{out}");
    }

    #[test]
    fn unmask_refuses_placeholder_moved_to_new_key_path() {
        let on_disk = "providers:\n  openai:\n    api_key: sk-moved-1\n";
        let submitted = format!("providers:\n  openai2:\n    api_key: {PLACEHOLDER}\n");
        assert_eq!(
            unmask(&submitted, on_disk),
            Err(UnmaskError::Unmatched {
                key_path: "providers.openai2.api_key".to_string()
            })
        );
    }

    const TWO_PROVIDERS: &str = "custom_providers:\n  - name: a\n    base_url: https://a\n    api_key: sk-a-1111\n  - name: b\n    base_url: https://b\n    api_key: sk-b-2222\n";

    fn unmatched(path: &str) -> Result<String, UnmaskError> {
        Err(UnmaskError::Unmatched {
            key_path: path.to_string(),
        })
    }

    #[test]
    fn unmask_list_unchanged_round_trips_byte_identical() {
        assert_round_trip(TWO_PROVIDERS);
    }

    #[test]
    fn unmask_refuses_list_placeholder_after_deleting_first_item() {
        let submitted = format!(
            "custom_providers:\n  - name: b\n    base_url: https://b\n    api_key: {PLACEHOLDER}\n"
        );
        assert_eq!(
            unmask(&submitted, TWO_PROVIDERS),
            unmatched("custom_providers[0].api_key")
        );
    }

    #[test]
    fn unmask_refuses_list_placeholder_after_swapping_items() {
        let submitted = format!(
            "custom_providers:\n  - name: b\n    base_url: https://b\n    api_key: {PLACEHOLDER}\n  - name: a\n    base_url: https://a\n    api_key: {PLACEHOLDER}\n"
        );
        assert!(unmask(&submitted, TWO_PROVIDERS).is_err());
    }

    #[test]
    fn unmask_refuses_list_placeholder_after_editing_item_base_url() {
        let submitted = TWO_PROVIDERS.replace("https://a", "https://evil");
        let submitted = mask(&submitted).expect("masks");
        assert_eq!(
            unmask(&submitted, TWO_PROVIDERS),
            unmatched("custom_providers[0].api_key")
        );
    }

    #[test]
    fn prepare_restores_placeholder_and_validates() {
        let t = "providers:\n  openai:\n    api_key: sk-live-1\nagent:\n  max_iterations: 5\n";
        let prepared = prepare(&mask(t).expect("masks"), t).expect("valid");
        assert_eq!(prepared.text, t);
        assert!(prepared.unknown_keys.is_empty());
    }

    fn secret_paths(text: &str) -> Vec<String> {
        scan(text)
            .into_iter()
            .filter(|s| s.secret)
            .map(|s| s.key_path)
            .collect()
    }

    fn assert_round_trip(text: &str) -> String {
        let masked = mask(text).expect("masks");
        assert_eq!(unmask(&masked, text).as_deref(), Ok(text), "{masked}");
        masked
    }

    #[test]
    fn mask_handles_list_item_mappings() {
        let text = "servers:\n  - name: a\n    api_key: sk-list-1\n  - api_key: sk-list-2\n";
        assert_eq!(
            secret_paths(text),
            vec!["servers[0].api_key", "servers[1].api_key"]
        );
        let masked = assert_round_trip(text);
        assert!(!masked.contains("sk-list-"), "{masked}");
    }

    #[test]
    fn mask_handles_quoted_keys_and_values() {
        let text = "\"api_key\": 'sk-''q''-1'\n";
        let masked = assert_round_trip(text);
        assert_eq!(masked, format!("\"api_key\": {PLACEHOLDER}\n"));
    }

    #[test]
    fn mask_preserves_crlf_line_endings() {
        let text = FIXTURE_BLOCK.replace('\n', "\r\n");
        let masked = assert_round_trip(&text);
        assert!(!masked.contains("sk-q-9"), "{masked}");
        assert_eq!(masked.matches('\n').count(), masked.matches("\r\n").count());
    }

    #[test]
    fn mask_keeps_trailing_comment_after_secret() {
        let text = "token: tk-123  # rotate monthly\n";
        let masked = assert_round_trip(text);
        assert_eq!(masked, format!("token: {PLACEHOLDER}  # rotate monthly\n"));
    }

    #[test]
    fn mask_handles_anchor_and_tag_on_secret_value() {
        let text = "secret: &s !!str sec-anchor-1\n";
        let masked = assert_round_trip(text);
        assert_eq!(masked, format!("secret: &s !!str {PLACEHOLDER}\n"));
    }

    #[test]
    fn mask_refuses_secret_reached_through_alias() {
        let text = "base: &k sk-alias-1\napi_key: *k\n";
        assert_eq!(
            mask(text),
            Err(MaskError::Unsupported {
                key_path: "api_key".to_string()
            })
        );
    }

    #[test]
    fn mask_hides_d09a_names() {
        let text = "web_ui:\n  auth:\n    password_hash: $argon2id$fake-hash-1\nmcp_servers:\n  x:\n    headers:\n      Authorization: Bearer auth-tok-1\ncookie: sid=cookie-1\n";
        let masked = assert_round_trip(text);
        for v in ["fake-hash-1", "auth-tok-1", "cookie-1"] {
            assert!(!masked.contains(v), "{v} leaked: {masked}");
        }
    }

    #[test]
    fn mask_keeps_env_name_keys_visible() {
        let text = "api_key_env: OPENAI_API_KEY\n";
        assert_eq!(mask(text).as_deref(), Ok(text));
    }

    #[test]
    fn mask_hides_d09b_hyphenated_header() {
        let text = "mcp_servers:\n  x:\n    headers:\n      X-API-Key: sk-hyph-1\n";
        let masked = assert_round_trip(text);
        assert!(!masked.contains("sk-hyph-1"), "{masked}");
    }

    #[test]
    fn mask_hides_d09b_names() {
        let text = "private_key: v-priv-1\naccess_key: v-acc-1\naws_secret_access_key: v-aws-1\nkey: v-key-1\nclient_credentials: v-cc-1\ndsn: v-dsn-1\ndb_connection_string: v-conn-1\nvoice:\n  record_key: ctrl+b\n";
        let masked = assert_round_trip(text);
        for v in [
            "v-priv-1", "v-acc-1", "v-aws-1", "v-key-1", "v-cc-1", "v-dsn-1", "v-conn-1",
        ] {
            assert!(!masked.contains(v), "{v} leaked: {masked}");
        }
        assert!(masked.contains("record_key: ctrl+b"), "{masked}");
    }

    #[test]
    fn mask_hides_all_scalars_under_secret_mapping() {
        let text = "tools:\n  credentials:\n    EXA_API_KEY: exa-1\n    SOME_SERVICE: svc-2\n";
        assert_eq!(
            secret_paths(text),
            vec![
                "tools.credentials.EXA_API_KEY",
                "tools.credentials.SOME_SERVICE"
            ]
        );
        let masked = assert_round_trip(text);
        assert!(
            !masked.contains("exa-1") && !masked.contains("svc-2"),
            "{masked}"
        );
    }

    #[test]
    fn mask_hides_url_userinfo_password() {
        let text =
            "database_url: postgres://u:pw-url-1@db:5432/x\nhomepage: https://example.com/a@b\n";
        let masked = assert_round_trip(text);
        assert_eq!(
            masked,
            format!("database_url: {PLACEHOLDER}\nhomepage: https://example.com/a@b\n")
        );
    }

    #[test]
    fn mask_ignores_comment_lines_that_look_like_keys() {
        let text = "# api_key: not-a-line\nmodel: x\n";
        assert!(scan(text).iter().all(|s| s.key_path != "api_key"));
        assert_eq!(mask(text).as_deref(), Ok(text));
    }

    #[test]
    fn is_secret_key_matches_d09b_names_and_allowlist() {
        for k in [
            "X-API-Key",
            "private_key",
            "key",
            "Credentials",
            "sentry_dsn",
        ] {
            assert!(is_secret_key(k), "{k} must be secret");
        }
        for k in ["record_key", "keybindings", "api-key-env", "monkey_patch"] {
            assert!(!is_secret_key(k), "{k} must not be secret");
        }
    }

    #[test]
    fn scan_survives_odd_layouts_without_panicking() {
        for text in [
            "-",
            "- ",
            "- - a\n  - b\n",
            "\t\tkey: v\n",
            "? a\n: b\n",
            "k: \"é\n",
            "é: 'x\n",
            "a:\n  - \n  -\n",
            "key: &a\n  x: 1\n",
            "---\n...\n",
            "\"unterminated: 1\n",
        ] {
            let _ = scan(text);
            let _ = mask(text);
        }
    }

    #[test]
    fn secret_values_collects_unquoted_secret_scalars() {
        let text =
            "api_key: \"sk-sv-1234\"\ntoken: tk-sv-5678\nsecret: abc\npassword: ~\nmodel: gpt-5\n";
        assert_eq!(
            secret_values(text),
            vec!["sk-sv-1234".to_string(), "tk-sv-5678".to_string()]
        );
    }

    #[test]
    fn scrub_replaces_secret_values_in_message() {
        let msg = "invalid type: string \"sk-sv-1234\", expected u32";
        let out = scrub(msg, &["sk-sv-1234".to_string()]);
        assert!(
            out.contains(PLACEHOLDER) && !out.contains("sk-sv-1234"),
            "{out}"
        );
        // Overlapping secrets: the longest is replaced first so no tail leaks.
        let out = scrub(
            "bad sk-abcd-extra here",
            &["sk-abcd".to_string(), "sk-abcd-extra".to_string()],
        );
        assert!(!out.contains("extra") && !out.contains("sk-abcd"), "{out}");
    }

    #[test]
    fn prepare_scrubs_restored_secret_from_validation_message() {
        let on_disk = "providers:\n  openai:\n    api_key: sk-scrub-0001\n";
        let submitted = format!(
            "providers:\n  openai:\n    api_key: {PLACEHOLDER}\nagent:\n  max_iterations: sk-scrub-0001\n"
        );
        match prepare(&submitted, on_disk) {
            Err(PrepareError::Invalid(issue)) => {
                assert!(
                    !issue.message.contains("sk-scrub-0001"),
                    "{}",
                    issue.message
                )
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn mask_refuses_multi_document_yaml() {
        assert!(mask("a: 1\n---\nb: 2\n").is_err());
    }

    #[test]
    fn mask_refuses_multi_line_double_quoted_secret() {
        let text = "api_key: \"sk-\n  part2\"\n";
        assert!(matches!(mask(text), Err(MaskError::Unsupported { .. })));
    }

    #[test]
    fn unmask_post_check_catches_placeholder_the_scanner_cannot_see() {
        let submitted = format!("notes: {{a: {PLACEHOLDER}}}\n");
        assert_eq!(
            unmask(&submitted, "model: x\n"),
            Err(UnmaskError::Unmatched {
                key_path: "notes.a".to_string()
            })
        );
    }

    const FIXTURE_ALL_LAYOUTS: &str = "# top comment\n# api_key: not-a-line\n\nproviders:\n  openai:\n    base_url: https://x\n    api_key: \"sk-fixture-1\"\n    api_key_env: OPENAI_API_KEY\n    other:\n      api_key: 'sk-fixture-2'\n  empty:\n    api_key: ~\n    token: \"\"\nservers:\n  - name: a\n    api_key: sk-fixture-3\n  - api_key: sk-fixture-4\nnames:\n  - alpha\n\"quoted_key_token\": sk-fixture-5   # trailing comment\nsecret: &s !!str sk-fixture-6\nweb_ui:\n  auth:\n    password_hash: $argon2id$v=19$m=19456,t=2,p=1$fakesalt$fakehash\nmcp_servers:\n  x:\n    headers:\n      Authorization: Bearer fixture-bearer-1\n      X-API-Key: sk-fixture-7\ncookie: sid=fixture-cookie-1\nprivate_key: sk-fixture-8\naccess_key: sk-fixture-9\naws_secret_access_key: sk-fixture-10\nkey: sk-fixture-11\nclient_credentials: sk-fixture-12\ndsn: https://fixture-dsn-13@sentry.example/1\ndb_connection_string: Server=fixture-conn-14\nvoice:\n  record_key: ctrl+b\ntools:\n  credentials:\n    EXA_API_KEY: sk-fixture-15\n    SOME_SERVICE: svc-fixture-16\n    nested:\n      deeper: svc-fixture-17\n    list:\n      - svc-fixture-18\ndatabase_url: postgres://u:pw-fixture-19@db:5432/x\nhomepage: https://example.com/a@b\nagent:\n  max_iterations: 5\n";

    #[test]
    fn mask_all_layouts_fixture_hides_every_secret_and_round_trips() {
        let masked = assert_round_trip(FIXTURE_ALL_LAYOUTS);
        for n in 1..=12 {
            let needle = format!("sk-fixture-{n}");
            // `sk-fixture-1` is a prefix of `sk-fixture-10`..`12`: compare whole tokens.
            let leaked = masked
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .any(|tok| tok == needle);
            assert!(!leaked, "{needle} leaked: {masked}");
        }
        for v in [
            "fixture-bearer-1",
            "fixture-cookie-1",
            "fakehash",
            "fixture-dsn-13",
            "fixture-conn-14",
            "svc-fixture-16",
            "svc-fixture-17",
            "svc-fixture-18",
            "pw-fixture-19",
        ] {
            assert!(!masked.contains(v), "{v} leaked: {masked}");
        }
        for v in [
            "https://x",
            "OPENAI_API_KEY",
            "ctrl+b",
            "https://example.com/a@b",
            "- alpha",
        ] {
            assert!(masked.contains(v), "{v} must stay visible: {masked}");
        }
    }
}
