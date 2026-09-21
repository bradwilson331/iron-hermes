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

/// Phase 51 Plan 15 (CR-03): the exact inverse of [`quote_env_value`] above.
///
/// Recognises the two four-byte escape runs [`quote_env_value`] emits —
/// `'\''` (a literal `'`) and `'\\'` (a literal `\`) — via a single
/// left-to-right character scan, NEVER a sequence of `str::replace` calls:
/// both runs start with the same byte (`'`), so a two-step replace is
/// order-sensitive and silently corrupts a value containing both a quote and
/// a backslash (replace the `'\''` run first and a literal backslash that
/// happens to sit next to a real quote can be mis-parsed as part of it, and
/// vice versa). A single scan that consumes each run atomically as it is
/// found has no such ordering to get wrong.
///
/// Also decodes the other quoting shape a human might hand-write — a value
/// wrapped in plain double quotes, `"..."`, stripped verbatim with no escape
/// processing (this project's writer never produces that shape, but a
/// hand-edited `.env` might, and the migration this function feeds must not
/// reject it). A value not wrapped in either quote style at all — again,
/// only ever hand-written, since [`quote_env_value`] always strong-quotes —
/// passes through completely unchanged. A trailing `\r` (a CRLF file) is
/// stripped before any of the above analysis, so a CRLF `.env` decodes
/// identically to its LF twin, and is reattached to nothing (the caller
/// already reads per logical line; the `\r` was never part of the value).
///
/// An opening quote with no matching closing quote is malformed input: it is
/// returned UNCHANGED rather than partially stripped. Half-decoding a
/// malformed line would itself be a silent, quiet corruption — exactly the
/// class of bug this function exists to stop introducing a fourth instance
/// of.
pub fn unquote_env_value(raw: &str) -> String {
    let no_cr = raw.strip_suffix('\r').unwrap_or(raw);
    let bytes = no_cr.as_bytes();

    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        return no_cr[1..no_cr.len() - 1].to_string();
    }

    if bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        let interior = &no_cr[1..no_cr.len() - 1];
        let chars: Vec<char> = interior.chars().collect();
        let mut out = String::with_capacity(interior.len());
        let mut i = 0;
        while i < chars.len() {
            // Recognise `'\''` / `'\\'` as a run of exactly 3 chars WITHIN the
            // interior (the surrounding quotes that make each run 4 bytes in
            // the full rendered line are already stripped off here): a `'`,
            // then `\`, then the escaped char itself.
            if chars[i] == '\''
                && i + 2 < chars.len()
                && chars[i + 1] == '\\'
                && (chars[i + 2] == '\'' || chars[i + 2] == '\\')
                && i + 3 < chars.len()
                && chars[i + 3] == '\''
            {
                out.push(chars[i + 2]);
                i += 4;
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        return out;
    }

    // Unquoted, or an unbalanced/malformed quote — return unchanged.
    no_cr.to_string()
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

/// Phase 51 Plan 15 (CR-03) — `unquote_env_value`'s own test module, separate
/// from [`tests`] above so the two functions' coverage stays clearly
/// attributed to which half of the pair each proves.
#[cfg(test)]
mod unquote_tests {
    use super::*;

    /// The thirteen corpus classes `<behavior>` requires, plus the pairing's
    /// dedicated mixed-escape case. Each entry is `(label, plaintext)`; every
    /// entry is round-tripped both ways: `unquote_env_value(&quote_env_value(v))
    /// == v`, and — since that alone would not catch a decoder that is simply
    /// the identity function — the QUOTED form is also asserted to differ from
    /// the plaintext whenever quoting actually changes the bytes (i.e. always,
    /// since `quote_env_value` unconditionally wraps in `'...'`).
    fn corpus() -> Vec<(&'static str, &'static str)> {
        vec![
            ("plain key-shaped string", "OPENROUTER_API_KEY_SK_ABC123"),
            ("empty string", ""),
            ("value with spaces", "sk with spaces in it"),
            ("value with #", "sk-value#with-hash"),
            ("value with $VAR", "sk-$HOME-literal"),
            ("value with ${VAR}", "sk-${HOME}-literal"),
            ("single quote", "it's-a-secret"),
            ("single backslash", r"a\backslash"),
            (r"backslash then quote (\')", "a\\'b"),
            (r"quote then backslash ('\)", "a'\\b"),
            (
                "several of each interleaved",
                "a'b\\c'd\\e''f\\\\g",
            ),
            ("trailing backslash", r"trailing-backslash\"),
            ("non-ASCII text", "sécrét-日本語-🔑"),
            ("mixed quote and backslash together", "a\\'b'\\c"),
        ]
    }

    #[test]
    fn round_trips_every_corpus_value() {
        for (label, value) in corpus() {
            let quoted = quote_env_value(value);
            let decoded = unquote_env_value(&quoted);
            assert_eq!(
                decoded, value,
                "round-trip failed for corpus case {label:?}: quote_env_value produced \
                 {quoted:?}, which unquote_env_value decoded to {decoded:?} instead of the \
                 original {value:?}"
            );
        }
    }

    #[test]
    fn unquoted_value_passes_through_unchanged() {
        let value = "not-quoted-at-all";
        assert_eq!(unquote_env_value(value), value);
    }

    #[test]
    fn double_quoted_value_has_quotes_removed() {
        assert_eq!(unquote_env_value("\"hello world\""), "hello world");
        assert_eq!(unquote_env_value("\"\""), "");
    }

    #[test]
    fn trailing_crlf_carriage_return_decodes_identically_to_lf() {
        let lf_quoted = quote_env_value("crlf-vs-lf-value");
        let crlf_quoted = format!("{lf_quoted}\r");
        assert_eq!(
            unquote_env_value(&crlf_quoted),
            unquote_env_value(&lf_quoted),
            "a CRLF-suffixed rendered value must decode identically to its LF twin"
        );
        assert_eq!(unquote_env_value(&crlf_quoted), "crlf-vs-lf-value");
    }

    #[test]
    fn malformed_unbalanced_opening_quote_is_returned_unchanged() {
        let malformed = "'this-never-closes";
        assert_eq!(
            unquote_env_value(malformed),
            malformed,
            "an opening quote with no matching close must be returned unchanged, never \
             partially stripped"
        );
    }

    #[test]
    fn single_bare_quote_character_is_returned_unchanged() {
        // Too short to be a validly-quoted (len >= 2) value either way.
        assert_eq!(unquote_env_value("'"), "'");
        assert_eq!(unquote_env_value("\""), "\"");
    }

    #[test]
    fn is_a_single_left_to_right_scan_not_a_replace_chain() {
        // Static guard: the whole point of the single-scan requirement is that
        // a `replace`-based implementation gets the mixed quote+backslash case
        // wrong. This corpus case (see `corpus()` above) already asserts the
        // BEHAVIOR; this asserts the STRUCTURAL constraint the module doc
        // requires by grepping the function's own source region.
        let src = include_str!("dotenv_write.rs");
        let start = src
            .find("pub fn unquote_env_value")
            .expect("unquote_env_value must exist in this file");
        let end = start
            + src[start..]
                .find("\n}\n")
                .expect("unquote_env_value's closing brace must exist")
            + 3;
        let body = &src[start..end];
        assert_eq!(
            body.matches("replace(").count(),
            0,
            "unquote_env_value must be a single scan, not a sequence of str::replace calls"
        );
    }
}
