use std::sync::OnceLock;

use sha2::Digest;

static PII_PATTERNS: OnceLock<Vec<regex::Regex>> = OnceLock::new();

fn pii_patterns() -> &'static Vec<regex::Regex> {
    PII_PATTERNS.get_or_init(|| {
        vec![
            // Card numbers: 12–19 consecutive digits
            regex::Regex::new(r"\b\d{12,19}\b").unwrap(),
            // Email addresses
            regex::Regex::new(r"[\w.\-+]+@[\w.\-]+\.\w{2,}").unwrap(),
            // API key / token / secret / password patterns
            regex::Regex::new(r"(?i)(api[_-]?key|token|secret|password)\s*[:=]\s*\S+").unwrap(),
        ]
    })
}

/// Recursively walk a `serde_json::Value`, replacing any string that matches
/// a PII pattern with `"[REDACTED]"`. Objects and arrays are walked in-place.
pub fn redact(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(redact_str(&s)),
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(redact).collect())
        }
        serde_json::Value::Object(map) => {
            let new_map = map.into_iter().map(|(k, v)| (k, redact(v))).collect();
            serde_json::Value::Object(new_map)
        }
        other => other,
    }
}

/// Apply all PII patterns to a plain string, replacing matches with `"[REDACTED]"`.
pub fn redact_str(s: &str) -> String {
    let mut result = s.to_string();
    for pattern in pii_patterns() {
        result = pattern.replace_all(&result, "[REDACTED]").into_owned();
    }
    result
}

/// Compute a stable SHA-256 hex hash of a `serde_json::Value`.
///
/// Keys in JSON objects are sorted recursively before serialization so that
/// `{"b":2,"a":1}` and `{"a":1,"b":2}` produce the same hash. This is the
/// canonical argument hash used in `tool_dispatched` events for dedup/lookup.
pub fn argument_hash(args: &serde_json::Value) -> String {
    let sorted = sort_keys(args);
    let canonical = serde_json::to_string(&sorted).unwrap_or_default();
    let hash = sha2::Sha256::digest(canonical.as_bytes());
    format!("{:x}", hash)
}

/// Recursively sort object keys in a JSON value (other types pass through).
fn sort_keys(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<(String, serde_json::Value)> =
                map.iter().map(|(k, v)| (k.clone(), sort_keys(v))).collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_keys).collect())
        }
        other => other.clone(),
    }
}
