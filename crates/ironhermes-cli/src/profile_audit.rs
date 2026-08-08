//! WR-03 — detect kanban worker profiles possibly exfiltrated during the CR-03 window.
//!
//! Phase 47.4 plan 20 closed the *future* CR-03 exfiltration path. It did nothing
//! about profiles already compromised before that patch landed, and a successful
//! pre-patch exploit **persists the dereferenced root secret in plaintext** into the
//! victim profile's `.env` permanently. This module is the operator-facing detection
//! that gap asked for.
//!
//! # The threat being detected
//!
//! Pre-plan-20, a manual key value of `${SOME_ROOT_VAR}` was written raw.
//! `read_env_keys` reads via `dotenvy`, whose `apply_substitution` resolves
//! `env::var` FIRST — and the web server loads root's `.env` into its own process
//! env — so the value dereferenced to root's real credential. Because every write
//! path read-merge-rewrites the whole file, the next save PERSISTED that plaintext
//! into the profile's own `.env`.
//!
//! Two distinct on-disk states, both reported:
//!
//! - [`Exposure::LiveSubstitution`] — the file still holds a raw, non-strong-quoted
//!   `${...}`. Not yet persisted, but it dereferences on EVERY read, so a dispatched
//!   worker resolves root's secret at runtime. Still live.
//! - [`Exposure::PersistedRootValue`] — a profile key's value byte-equals a ROOT
//!   key's value under a DIFFERENT key name. That cross-name match is the fingerprint
//!   of a dereference that already landed on disk.
//!
//! # What is deliberately NOT flagged
//!
//! **Same-name matches.** Legitimate inheritance (D-07) copies root's `X` into a
//! profile's `X` by design — that is the wizard's documented behavior, not an
//! exploit. Flagging it would fire on every correctly configured profile and train
//! the operator to ignore this check, which is worse than not shipping it.
//!
//! # Invariants
//!
//! - **No finding ever carries a value.** Every type here holds key NAMES only, and
//!   [`render_findings`] prints names only. An audit tool that dumped secrets into a
//!   terminal or CI log would itself be the next occurrence of this bug class.
//! - **Nothing is modified.** Detection only. A leaked credential is remediated by
//!   ROTATION, not by deleting the file — deleting it destroys evidence and un-leaks
//!   nothing — so this module never writes, rewrites, or removes anything.

use std::collections::HashMap;
use std::path::Path;

/// One way a profile `.env` shows CR-03 exposure. Key NAMES only — never values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exposure {
    /// A raw, non-strong-quoted `${...}` is still on disk: it re-dereferences on
    /// every read, so a dispatched worker resolves the root credential at runtime.
    LiveSubstitution { key: String },
    /// This profile key's value equals a ROOT `.env` value stored under a different
    /// name — the fingerprint of an already-persisted dereference.
    PersistedRootValue { key: String, root_key: String },
}

/// Findings for one profile. `exposures` empty means the profile looks clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFindings {
    pub profile: String,
    pub exposures: Vec<Exposure>,
}

/// Scan RAW `.env` text for live `${...}` substitution vectors.
///
/// Deliberately does NOT go through `dotenvy`: that would *resolve* the
/// substitution and mask the exact thing being detected.
///
/// A strong-quoted (`'...'`) value is inert — dotenvy's `parse_value` checks the
/// strong-quote branch before the `$` branch, so `${...}` inside single quotes is
/// literal. Plan 20's renderer always emits strong quotes, so post-fix files are
/// correctly ignored here. A weak-quoted (`"..."`) value DOES substitute, so it is
/// treated as live.
fn scan_raw_for_live_substitution(raw: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // dotenv permits an `export ` prefix.
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let Some((raw_key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty() {
            continue;
        }
        let value = raw_value.trim_start();
        if value.starts_with('\'') {
            continue; // strong-quoted: inert
        }
        if value.contains("${") {
            found.push(key.to_string());
        }
    }
    found
}

/// Parse a `.env` into name -> value pairs for value comparison.
///
/// Only called for files that raw-scanned clean, so there is no `${...}` left for
/// `dotenvy` to substitute and this read cannot itself dereference anything.
fn read_pairs(path: &Path) -> Option<HashMap<String, String>> {
    let iter = dotenvy::from_path_iter(path).ok()?;
    let mut map = HashMap::new();
    for item in iter {
        let (k, v) = item.ok()?;
        map.insert(k, v);
    }
    Some(map)
}

/// Audit every profile under `profiles_root` against the root `.env`.
///
/// Returns only profiles with at least one exposure. Never reads or returns a value.
pub fn audit_profiles_at(profiles_root: &Path, root_env_path: &Path) -> Vec<ProfileFindings> {
    let root_pairs = read_pairs(root_env_path).unwrap_or_default();

    let Ok(entries) = std::fs::read_dir(profiles_root) else {
        return Vec::new();
    };

    let mut results: Vec<ProfileFindings> = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let profile = entry.file_name().to_string_lossy().to_string();
        let env_path = entry.path().join(".env");
        let Ok(raw) = std::fs::read_to_string(&env_path) else {
            continue;
        };

        let mut exposures: Vec<Exposure> = scan_raw_for_live_substitution(&raw)
            .into_iter()
            .map(|key| Exposure::LiveSubstitution { key })
            .collect();

        // Only compare values when nothing live remains. A file with a raw `${...}`
        // is reported under LiveSubstitution and skipped here on purpose: reading it
        // would dereference. Documented limitation — fix the live vector, re-run.
        if exposures.is_empty()
            && let Some(profile_pairs) = read_pairs(&env_path)
        {
            let mut persisted: Vec<Exposure> = Vec::new();
            for (key, value) in &profile_pairs {
                if value.is_empty() {
                    continue; // two empty values matching means nothing
                }
                for (root_key, root_value) in &root_pairs {
                    // Same-name match is legitimate inheritance (D-07), not an exploit.
                    if root_key != key && root_value == value {
                        persisted.push(Exposure::PersistedRootValue {
                            key: key.clone(),
                            root_key: root_key.clone(),
                        });
                    }
                }
            }
            // Deterministic output regardless of HashMap iteration order.
            persisted.sort_by_key(exposure_sort_key);
            exposures.extend(persisted);
        }

        if !exposures.is_empty() {
            results.push(ProfileFindings { profile, exposures });
        }
    }

    results.sort_by(|a, b| a.profile.cmp(&b.profile));
    results
}

fn exposure_sort_key(e: &Exposure) -> (String, String) {
    match e {
        Exposure::LiveSubstitution { key } => (key.clone(), String::new()),
        Exposure::PersistedRootValue { key, root_key } => (key.clone(), root_key.clone()),
    }
}

/// Render findings as operator-facing lines. Key NAMES only — never a value.
///
/// Returns an empty vec when there is nothing to report, so the caller can print a
/// single clean-status line instead.
pub fn render_findings(findings: &[ProfileFindings]) -> Vec<String> {
    if findings.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for f in findings {
        out.push(format!("profile \"{}\":", f.profile));
        for e in &f.exposures {
            match e {
                Exposure::LiveSubstitution { key } => out.push(format!(
                    "    {key}: value contains a live ${{...}} substitution — it resolves \
                     against the server's own environment on every read"
                )),
                Exposure::PersistedRootValue { key, root_key } => out.push(format!(
                    "    {key}: value is identical to the root .env's {root_key} \
                     (different name — consistent with a CR-03 dereference)"
                )),
            }
        }
    }
    out.push(String::new());
    out.push(
        "ROTATE the affected root credentials. Deleting or editing these files does NOT \
         remediate a credential that has already been disclosed to disk."
            .to_string(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ROOT_SECRET: &str = "sk-root-anthropic-REAL-SECRET";

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, contents).expect("write");
    }

    fn setup(profiles: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path().join(".env"),
            &format!("ANTHROPIC_API_KEY={ROOT_SECRET}\nOTHER_ROOT=root-other-value\n"),
        );
        for (name, env) in profiles {
            write(&tmp.path().join("profiles").join(name).join(".env"), env);
        }
        tmp
    }

    fn audit(tmp: &tempfile::TempDir) -> Vec<ProfileFindings> {
        audit_profiles_at(&tmp.path().join("profiles"), &tmp.path().join(".env"))
    }

    /// S1: a raw `${...}` still on disk re-dereferences on every read.
    #[test]
    fn detects_a_live_substitution_vector() {
        let tmp = setup(&[("victim", "OPENROUTER_API_KEY=${ANTHROPIC_API_KEY}\n")]);
        let findings = audit(&tmp);

        assert_eq!(
            findings.len(),
            1,
            "expected one flagged profile: {findings:?}"
        );
        assert_eq!(findings[0].profile, "victim");
        assert_eq!(
            findings[0].exposures,
            vec![Exposure::LiveSubstitution {
                key: "OPENROUTER_API_KEY".to_string()
            }]
        );
    }

    /// Plan 20's renderer strong-quotes every value, which makes `${...}` inert.
    /// Those files must NOT be flagged or the check cries wolf on every fixed profile.
    #[test]
    fn a_strong_quoted_substitution_is_inert_and_not_flagged() {
        let tmp = setup(&[("fixed", "OPENROUTER_API_KEY='${ANTHROPIC_API_KEY}'\n")]);
        assert!(
            audit(&tmp).is_empty(),
            "a single-quoted ${{...}} is literal to dotenvy and must not be flagged"
        );
    }

    /// S2: the dereference already landed — profile value == root value, different name.
    #[test]
    fn detects_an_already_persisted_root_value_under_a_different_name() {
        let tmp = setup(&[("victim", &format!("OPENROUTER_API_KEY='{ROOT_SECRET}'\n"))]);
        let findings = audit(&tmp);

        assert_eq!(
            findings.len(),
            1,
            "expected one flagged profile: {findings:?}"
        );
        assert_eq!(
            findings[0].exposures,
            vec![Exposure::PersistedRootValue {
                key: "OPENROUTER_API_KEY".to_string(),
                root_key: "ANTHROPIC_API_KEY".to_string(),
            }]
        );
    }

    /// D-07 inheritance copies root's X into the profile's X by design. Flagging that
    /// would fire on every correctly configured profile.
    #[test]
    fn same_name_inheritance_is_not_flagged() {
        let tmp = setup(&[("inherited", &format!("ANTHROPIC_API_KEY='{ROOT_SECRET}'\n"))]);
        assert!(
            audit(&tmp).is_empty(),
            "same-name inheritance is the documented D-07 behavior, not an exploit"
        );
    }

    #[test]
    fn a_clean_profile_reports_nothing() {
        let tmp = setup(&[("clean", "OPENROUTER_API_KEY='sk-its-own-distinct-key'\n")]);
        assert!(audit(&tmp).is_empty());
    }

    #[test]
    fn an_empty_value_is_not_treated_as_a_match() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(&tmp.path().join(".env"), "EMPTY_ROOT=\n");
        write(
            &tmp.path().join("profiles").join("p").join(".env"),
            "SOME_KEY=\n",
        );
        assert!(
            audit_profiles_at(&tmp.path().join("profiles"), &tmp.path().join(".env")).is_empty(),
            "two empty values matching is meaningless, not evidence"
        );
    }

    /// The whole point of the tool is to report a leak without becoming one.
    #[test]
    fn rendered_output_never_contains_a_secret_value() {
        let tmp = setup(&[
            ("victim", &format!("OPENROUTER_API_KEY='{ROOT_SECRET}'\n")),
            ("live", "SOME_KEY=${ANTHROPIC_API_KEY}\n"),
        ]);
        let findings = audit(&tmp);
        assert_eq!(findings.len(), 2, "expected both profiles: {findings:?}");

        let rendered = render_findings(&findings).join("\n");
        assert!(
            !rendered.contains(ROOT_SECRET),
            "rendered audit output must never contain a secret value; got:\n{rendered}"
        );
        // Also assert the debug form is clean — findings get logged.
        let debug = format!("{findings:?}");
        assert!(
            !debug.contains(ROOT_SECRET),
            "findings' Debug must never contain a secret value; got:\n{debug}"
        );
        // Still actionable.
        assert!(rendered.contains("victim"), "must name the profile");
        assert!(
            rendered.contains("OPENROUTER_API_KEY"),
            "must name the affected key"
        );
        assert!(
            rendered.to_lowercase().contains("rotate"),
            "must instruct rotation — deleting the file does not un-leak the credential"
        );
    }

    #[test]
    fn a_missing_profiles_root_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(audit_profiles_at(&tmp.path().join("nope"), &tmp.path().join(".env")).is_empty());
    }
}
