//! Phase 47.6 Plan 04 (D-06): the single shared `.env` value-writing
//! implementation for the whole workspace.
//!
//! Hoisted from `crates/iron_hermes_ui/src/server/profile_api.rs` (Phase 47.4
//! Plan 20, CR-03/CR-04/T-47.4-20-04/T-47.4-20-05) — that crate's copy is now
//! a thin delegating wrapper over this module. A second hand-written copy of
//! this exact logic is how this bug class reached six occurrences across
//! Phase 47.4; this module exists so there is exactly one seam, consumed by
//! both `iron_hermes_ui`'s profile wizard and `ironhermes-cli`'s `buzz`
//! subcommands (Phase 47.6 Plan 04).
//!
//! The doc comments on [`quote_env_value`] and [`verify_env_round_trip`] below
//! are carried across verbatim from the original — they are the accumulated
//! record of *why* the implementation is shaped the way it is (dotenvy's
//! splitter and its value parser disagree about a backslash inside a strong
//! quote; a `dotenvy::Error`'s `Display` embeds the raw failing line, which
//! for a secret-bearing line IS the secret). Rewriting them from memory loses
//! the reasoning and re-opens the bug.

/// Phase 47.4 Plan 20 (CR-03/CR-04, D-06/D-07): single-quotes a raw value for
/// a rendered `.env` line. Every character is emitted verbatim inside
/// `'...'` — which suppresses `dotenvy`'s `$`-substitution, space-as-end-of-
/// value, and `#`-as-comment rules (`parse_value`, `dotenvy-0.15.7/src/
/// parse.rs:165-236`, checks `strong_quote` before any of those branches) —
/// EXCEPT `'` and `\`, which have no in-quote escape and must each be
/// represented by leaving the quote to emit an escaped copy of themselves,
/// then reopening: `close-quote, backslash, the character itself, reopen-
/// quote`. This is deliberately NOT symmetric writer-vs-reader cleverness —
/// it is the one substitution `dotenvy`'s own splitter (`iter.rs:69-179`,
/// `eval_end_state`) and its value-level parser agree is unambiguous, and it
/// is why a value ending in `\` cannot swallow the following line (the
/// splitter treats `\` as an escape even inside a strong quote, `parse_value`
/// does not; ending on a raw un-escaped `\` before a closing `'` is exactly
/// the divergence this comment traces).
pub fn quote_env_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        match c {
            '\'' | '\\' => {
                out.push('\''); // close the currently-open quote
                out.push('\\'); // escape...
                out.push(c); // ...this exact character, outside any quote
                out.push('\''); // reopen the quote for what follows
            }
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Phase 47.6 Plan 04: error type for this module's writer/verifier. Its
/// `Display` never embeds a `dotenvy::Error` in any form (no `{e}`,
/// `.to_string()`, `Display`, `Debug`, or wrapping it as an error `source`)
/// and never embeds a value — only the fact of failure, and (when one
/// exists) the offending KEY NAME, which is not a secret.
///
/// D-13's asymmetric-error-branch rule, carried over unchanged: a MISMATCH
/// has a parsed `Vec` in hand, so it names the offending key. A hard PARSE
/// error has no `Vec` — `.collect::<Result<Vec<_>, _>>()` short-circuits on
/// the first `Err` before any entry is collected — so no key name is
/// available, and the raw `dotenvy::Error` (whose `Display` embeds the
/// entire raw failing line — for a rendered `KEY='the-secret-value'` line,
/// that IS the secret) is never surfaced in any form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DotenvWriteError {
    /// The rendered text did not parse cleanly as dotenv at all.
    Parse,
    /// The round-trip produced a different number of entries than expected.
    /// Only the counts are named — never a value.
    CountMismatch { expected: usize, actual: usize },
    /// A specific key round-tripped to a different value (or a different key
    /// occupied that position) than expected. Only the key name is named.
    Mismatch { key: String },
}

impl std::fmt::Display for DotenvWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DotenvWriteError::Parse => write!(
                f,
                "dotenv round-trip verification: rendered .env did not parse cleanly — refusing write"
            ),
            DotenvWriteError::CountMismatch { expected, actual } => write!(
                f,
                "dotenv round-trip verification: rendered .env round-trip produced {actual} entries, expected {expected} — refusing write"
            ),
            DotenvWriteError::Mismatch { key } => write!(
                f,
                "dotenv round-trip verification: rendered .env did not round-trip cleanly for key '{key}' — refusing write"
            ),
        }
    }
}

impl std::error::Error for DotenvWriteError {}

/// Phase 47.4 Plan 20 (T-47.4-20-04): a rendered `.env` body's self-check —
/// round-trips the just-rendered bytes back through the REAL `dotenvy`
/// reader (in memory, no disk I/O, no process-env mutation) and refuses to
/// let the write proceed unless the parse yields exactly the given entry
/// list, in order. This is what makes an unknown-unknown in this class a
/// clean refusal instead of a silent corruption or exfiltration: any future
/// `dotenvy` grammar rule nobody has thought of yet is caught here, not
/// discovered as a sixth recurrence.
pub fn verify_env_round_trip(
    rendered: &str,
    entries: &[(String, String)],
) -> Result<(), DotenvWriteError> {
    let parsed: Vec<(String, String)> =
        match dotenvy::from_read_iter(std::io::Cursor::new(rendered.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(v) => v,
            Err(_) => {
                return Err(DotenvWriteError::Parse);
            }
        };

    if parsed.len() != entries.len() {
        return Err(DotenvWriteError::CountMismatch {
            expected: entries.len(),
            actual: parsed.len(),
        });
    }
    for ((expected_name, expected_value), (parsed_name, parsed_value)) in
        entries.iter().zip(parsed.iter())
    {
        if expected_name != parsed_name || expected_value != parsed_value {
            return Err(DotenvWriteError::Mismatch {
                key: expected_name.clone(),
            });
        }
    }
    Ok(())
}

/// Phase 47.6 Plan 04: merge `entries` into an existing `.env` body,
/// replacing the value for any key already present and appending the keys
/// that were absent — then re-render EVERY value through
/// [`quote_env_value`] and refuse to return unless [`verify_env_round_trip`]
/// confirms the rendered result parses back to exactly the intended
/// key/value set.
///
/// This is the write path a profile `.env` holding provider keys needs: a
/// naive append or rewrite would destroy those keys and take the kanban
/// worker down with it (T-47.6-04-05).
pub fn upsert_env_entries(
    existing_body: &str,
    entries: &[(String, String)],
) -> Result<String, DotenvWriteError> {
    let existing_parsed: Vec<(String, String)> =
        match dotenvy::from_read_iter(std::io::Cursor::new(existing_body.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(v) => v,
            Err(_) => return Err(DotenvWriteError::Parse),
        };

    let updates: std::collections::HashMap<&str, &str> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut merged: Vec<(String, String)> =
        Vec::with_capacity(existing_parsed.len() + entries.len());

    for (key, value) in existing_parsed {
        let final_value = updates
            .get(key.as_str())
            .map(|v| v.to_string())
            .unwrap_or(value);
        seen.insert(key.clone());
        merged.push((key, final_value));
    }
    for (key, value) in entries {
        if seen.insert(key.clone()) {
            merged.push((key.clone(), value.clone()));
        }
    }

    let mut out = String::new();
    for (key, value) in &merged {
        out.push_str(key);
        out.push('=');
        out.push_str(&quote_env_value(value));
        out.push('\n');
    }

    verify_env_round_trip(&out, &merged)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: &str) -> String {
        let quoted = quote_env_value(value);
        let rendered = format!("KEY={quoted}\n");
        let parsed: Vec<(String, String)> =
            dotenvy::from_read_iter(std::io::Cursor::new(rendered.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .expect("rendered value must parse cleanly");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "KEY");
        parsed[0].1.clone()
    }

    #[test]
    fn quotes_a_plain_value() {
        let quoted = quote_env_value("hello");
        assert_eq!(quoted, "'hello'");
    }

    #[test]
    fn dollar_substitution_is_suppressed() {
        let value = "${HOME}/secret";
        assert_eq!(round_trip(value), value);
    }

    #[test]
    fn spaces_do_not_end_the_value() {
        let value = "value with spaces";
        assert_eq!(round_trip(value), value);
    }

    #[test]
    fn hash_does_not_start_a_comment() {
        let value = "value#with-hash";
        assert_eq!(round_trip(value), value);
    }

    #[test]
    fn embedded_single_quote_round_trips() {
        let value = "it's a secret";
        assert_eq!(round_trip(value), value);
    }

    #[test]
    fn trailing_backslash_does_not_swallow_the_next_line() {
        let value_a = r"trailing-backslash\";
        let value_b = "the-next-value";
        let rendered = format!(
            "KEY_A={}\nKEY_B={}\n",
            quote_env_value(value_a),
            quote_env_value(value_b)
        );
        let parsed: Vec<(String, String)> =
            dotenvy::from_read_iter(std::io::Cursor::new(rendered.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .expect("rendered body must parse cleanly");
        assert_eq!(parsed.len(), 2, "trailing backslash must not merge lines");
        assert_eq!(parsed[0], ("KEY_A".to_string(), value_a.to_string()));
        assert_eq!(parsed[1], ("KEY_B".to_string(), value_b.to_string()));
    }

    #[test]
    fn round_trip_verifier_rejects_a_mismatch() {
        let entries = vec![
            ("KEY_A".to_string(), "expected-a".to_string()),
            ("KEY_B".to_string(), "expected-b".to_string()),
        ];
        let rendered = format!(
            "KEY_A={}\nKEY_B={}\n",
            quote_env_value("expected-a"),
            quote_env_value("actual-b"),
        );
        let err = verify_env_round_trip(&rendered, &entries).unwrap_err();
        match err {
            DotenvWriteError::Mismatch { key } => assert_eq!(key, "KEY_B"),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_verifier_error_never_contains_a_value() {
        let entries = vec![("KEY_B".to_string(), "expected-b-secret".to_string())];
        let rendered = format!("KEY_B={}\n", quote_env_value("actual-b-secret"));
        let err = verify_env_round_trip(&rendered, &entries).unwrap_err();
        let rendered_err = err.to_string();
        assert!(rendered_err.contains("KEY_B"));
        assert!(!rendered_err.contains("expected-b-secret"));
        assert!(!rendered_err.contains("actual-b-secret"));
    }

    #[test]
    fn parse_error_branch_never_contains_the_failing_line() {
        // An unparseable line built from a secret-bearing value: an
        // unterminated strong quote is a hard parse error, not a mismatch.
        let secret = "sk-super-secret-marker-value";
        let unparseable = format!("KEY_A='{secret}\n");
        let entries = vec![("KEY_A".to_string(), secret.to_string())];
        let err = verify_env_round_trip(&unparseable, &entries).unwrap_err();
        assert_eq!(err, DotenvWriteError::Parse);
        let rendered_err = err.to_string();
        assert!(!rendered_err.contains(secret));
    }

    #[test]
    fn upsert_replaces_an_existing_key_and_preserves_the_others() {
        let body = format!(
            "KEY_A={}\nKEY_B={}\nKEY_C={}\n",
            quote_env_value("val_a"),
            quote_env_value("old_b"),
            quote_env_value("val_c"),
        );
        let result =
            upsert_env_entries(&body, &[("KEY_B".to_string(), "new_b".to_string())]).unwrap();

        assert!(result.contains(&format!("KEY_A={}\n", quote_env_value("val_a"))));
        assert!(result.contains(&format!("KEY_B={}\n", quote_env_value("new_b"))));
        assert!(result.contains(&format!("KEY_C={}\n", quote_env_value("val_c"))));
        assert!(!result.contains("old_b"));

        let parsed: Vec<(String, String)> =
            dotenvy::from_read_iter(std::io::Cursor::new(result.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .expect("upserted body must parse cleanly");
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn upsert_adds_a_new_key_when_absent() {
        let body = format!("KEY_A={}\n", quote_env_value("val_a"));
        let result =
            upsert_env_entries(&body, &[("KEY_NEW".to_string(), "val_new".to_string())]).unwrap();

        let parsed: std::collections::HashMap<String, String> =
            dotenvy::from_read_iter(std::io::Cursor::new(result.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .expect("upserted body must parse cleanly")
                .into_iter()
                .collect();
        assert_eq!(parsed.get("KEY_A").map(String::as_str), Some("val_a"));
        assert_eq!(
            parsed.get("KEY_NEW").map(String::as_str),
            Some("val_new")
        );
        assert_eq!(parsed.len(), 2);
    }
}
