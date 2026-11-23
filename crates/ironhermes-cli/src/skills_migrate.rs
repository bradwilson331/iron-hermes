//! On-disk migration for stale `~/.hermes/` path references and the removed
//! `HERMES_HOME` env var inside the operator's INSTALLED `skills` and
//! `optional-skills` trees (D-10, Phase 54.1 Plan 04).
//!
//! The rule set applied here (`R1`-`R7`) is TRANSCRIBED from
//! `54.1-PATH-REWRITE-MANIFEST.md`'s `## Migration pattern set` section — that
//! manifest is the single classification both the repo-content rewrite
//! (Plan 03, already shipped) and this on-disk migration derive from, so the
//! repo rewrite and this migration cannot silently diverge. See that file for
//! the full per-occurrence table and the rationale behind each rule's
//! anchoring.
//!
//! Every rewrite replaces ONLY the classified substrings matched by the rule
//! table below — never a whole file. The installed trees are operator-owned:
//! they may carry the operator's own edits, and a whole-file substitution
//! would silently destroy them — the exact anti-pattern
//! `ironhermes_kanban::skills_bundle::sync_one`'s skip-existing rule exists to
//! prevent on its own, unrelated, surface (this module does not import or
//! depend on that one; the discipline is copied, the mechanism is not).
//! Every write goes through a temp-file-then-rename, mirroring
//! `ironhermes_hub::lock::SkillLock::save_atomic`'s atomic-write sequence, so
//! a crash mid-write cannot leave a truncated `SKILL.md`.
//!
//! Roots are resolved from the ROOT IronHermes home
//! (`ironhermes_core::get_root_hermes_home`) by the CLI caller in
//! `skills_cmd.rs`, never the process home — per D-01/D-10 a `--profile`-
//! pivoted invocation must still target the one shared catalog. This module
//! itself takes the root as a plain `&Path` parameter rather than reading any
//! env var, so its tests are deterministic.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Rule table (R1-R7), transcribed from 54.1-PATH-REWRITE-MANIFEST.md's
// "## Migration pattern set" section.
// ---------------------------------------------------------------------------

/// R1 (bucket B, root-pinned skill-tree paths). Anchored to a preceding
/// `~/`, `$HOME/`, or `/home/<user>/` prefix immediately before `.hermes/`
/// immediately before `skills/` or `optional-skills/`. MUST run before R2 —
/// R2's pattern is a strict superset of this one, and running R2 first would
/// swallow every bucket-B occurrence into the per-profile form.
static R1_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:~|\$HOME|/home/[^/]+)/\.hermes/(skills/|optional-skills/)")
        .expect("R1_RE compilation failed")
});

/// R2 (bucket A, credentials/state, per-profile). Same prefix set as R1, but
/// without the skills/optional-skills anchor — applied AFTER R1 so it only
/// ever matches what R1 did not already consume.
static R2_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:~|\$HOME|/home/[^/]+)/\.hermes/").expect("R2_RE compilation failed"));

/// R3 (bucket C, workspace-relative marker, no home prefix). Deliberately NOT
/// anchored to a home prefix — a bare `.hermes/plans/` with no `~`/`$HOME`/
/// `/home/<user>` prefix is a workspace-relative path and must never gain
/// one. Applied after R1/R2 so any prefixed occurrence of this same literal
/// text has already been consumed by one of those two rules first.
static R3_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.hermes/plans/").expect("R3_RE compilation failed"));

/// R4 (bucket D, removed home env var). Word-boundary only, so
/// `_HERMES_HOME`-style identifiers (a different token) are excluded, and so
/// that a previously-rewritten `IRONHERMES_HOME` never matches again on a
/// second run (the word boundary immediately before `H` fails when the
/// preceding character is the word character `N`) — this is what makes R4
/// idempotent at the pure-function level.
static R4_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bHERMES_HOME\b").expect("R4_RE compilation failed"));

/// R5 (bucket D companion). Bare quoted Python default on a line that R4 also
/// matched, e.g. `Path.home() / ".hermes"`.
static R5_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""\.hermes""#).expect("R5_RE compilation failed"));

/// R6 (bucket D companion). Shell `${VAR:-default}` brace form on a line that
/// R4 also matched, e.g. `${HERMES_HOME:-$HOME/.hermes}`.
static R6_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$HOME/\.hermes\}").expect("R6_RE compilation failed"));

/// R7 (bucket D companion). Quoted tilde default on a line that R4 also
/// matched, e.g. `os.environ.get("HERMES_HOME", "~/.hermes")`.
static R7_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""~/\.hermes""#).expect("R7_RE compilation failed"));

/// R8 (bucket D companion, Phase 54.1 Plan 05 Task 3 deviation — Rule 1 bug
/// fix). R4-R7 correctly wrap the removed env var when it appears inside a
/// recognized CODE-shaped default-assignment pattern (Python `os.getenv`,
/// shell `${VAR:-default}`) on the same line, but R4 by itself is a bare
/// word/variable RENAME with no wrapping — a shell-style `$HERMES_HOME`
/// mention in markdown prose or a shell command with no accompanying
/// default-assignment shape on the same line (e.g. a doc's `` `$HERMES_HOME/config.yaml` ``
/// code span, or `CANVAS="python $HERMES_HOME/skills/..."`) survives R4's
/// rename as a BARE `$IRONHERMES_HOME` reference, violating D-04's mandatory
/// `${VAR:-default}` form. Discovered via the real operator-tree run against
/// `/Users/you/.ironhermes` (7 residual bare references across
/// `qis-communication-jta/scripts/numbering.py` ×3 — a file outside this
/// repo's own `skills/` tree entirely — plus the installed, pre-migration
/// copies of `touchdesigner-mcp/references/troubleshooting.md`,
/// `honcho/SKILL.md`, `memento_cards.py`, and `canvas/SKILL.md`, the same four
/// files Plan 03's own deviation #1 already fixed in the REPO copies for the
/// identical underlying defect pattern) — the manifest-driven repo rewrite
/// (Plan 03) classified and fixed these files' REPO content by hand per
/// occurrence, but this migration tool's REGEX rules (Plan 04) had no rule
/// covering a bare, unwrapped shell-style reference, so the operator's stale
/// installed copies kept it. Applied GLOBALLY (not line-and-R4-gated, unlike
/// R5-R7) and AFTER the per-line pass, so a bare reference surviving from a
/// PRIOR run (one R4 does not re-match, since the word is already
/// `IRONHERMES_HOME`) still gets caught and wrapped on a later run — required
/// for this rule to be idempotent-additive rather than a one-shot patch.
/// Anchored to `$IRONHERMES_HOME` with a trailing word boundary so it never
/// matches the already-correct `${IRONHERMES_HOME:-...}` form (the character
/// right after `$` there is `{`, not `I`) or a longer identifier sharing the
/// same prefix.
static R8_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$IRONHERMES_HOME\b").expect("R8_RE compilation failed"));

/// Apply every rule in the manifest's pattern set to `content`, in order.
/// Returns `None` when no rule produced a change, `Some(new)` otherwise,
/// where `new` differs from `content` only at the matched substrings.
///
/// R1-R3 are applied globally over the whole content (they are substring
/// rules, not line rules). R4-R7 are applied per physical line, and R5/R6/R7
/// fire ONLY on a line where R4 also matched — this is the scoping fix for
/// the exact over-match class Plan 03 hit and reverted (its mechanical
/// "repoint the Python default" step fired on every line instead of only
/// lines carrying a `HERMES_HOME` occurrence). R8 (added Phase 54.1 Plan 05
/// Task 3) is also applied per physical line but, unlike R5-R7, is NEVER
/// gated behind R4's per-run match — it must independently catch any bare
/// `$IRONHERMES_HOME` left on a line, including one R4 does not match this
/// run because the rename already happened in a prior run.
///
/// `rewrite_stale_paths(rewrite_stale_paths(x).unwrap())` returns `None` for
/// every `x` these rules can produce — see the `skills_migrate_rules_are_a_fixed_point`
/// test. This holds because every rule's replacement text can never again
/// match that same rule's own pattern (confirmed per-rule in
/// `54.1-PATH-REWRITE-MANIFEST.md`'s "Scanner non-collision check").
pub fn rewrite_stale_paths(content: &str) -> Option<String> {
    let after_r1 = R1_RE.replace_all(content, |caps: &regex::Captures| {
        format!("~/.ironhermes/{}", &caps[1])
    });
    let after_r2 = R2_RE.replace_all(&after_r1, |_: &regex::Captures| {
        "${IRONHERMES_HOME:-$HOME/.ironhermes}/".to_string()
    });
    let after_r3 = R3_RE.replace_all(&after_r2, |_: &regex::Captures| {
        ".ironhermes/plans/".to_string()
    });

    let eol = if after_r3.contains("\r\n") { "\r\n" } else { "\n" };
    let had_trailing_newline = after_r3.ends_with('\n');

    let lines: Vec<String> = after_r3
        .lines()
        .map(|line| {
            // R4-R7 (the code-shaped default-assignment wrap) only fire on a
            // line R4 itself matched. R8 is deliberately NOT gated behind
            // that check — unlike R5-R7, it must still catch a bare
            // `$IRONHERMES_HOME` reference left over from a run BEFORE this
            // rule existed, on a line that no longer contains the word
            // `HERMES_HOME` at all (it was already renamed), which is
            // exactly the case a gated check would make this rule
            // permanently unable to self-heal on a later run.
            if !R4_RE.is_match(line) {
                return R8_RE
                    .replace_all(line, |_: &regex::Captures| {
                        "${IRONHERMES_HOME:-$HOME/.ironhermes}".to_string()
                    })
                    .into_owned();
            }
            let l = R4_RE.replace_all(line, |_: &regex::Captures| "IRONHERMES_HOME".to_string());
            let l = R5_RE.replace_all(&l, |_: &regex::Captures| "\".ironhermes\"".to_string());
            let l = R6_RE.replace_all(&l, |_: &regex::Captures| "$HOME/.ironhermes}".to_string());
            let l = R7_RE.replace_all(&l, |_: &regex::Captures| "\"~/.ironhermes\"".to_string());
            let l = R8_RE.replace_all(&l, |_: &regex::Captures| {
                "${IRONHERMES_HOME:-$HOME/.ironhermes}".to_string()
            });
            l.into_owned()
        })
        .collect();

    let mut result = lines.join(eol);
    if had_trailing_newline && !result.is_empty() {
        result.push_str(eol);
    }

    if result == content {
        None
    } else {
        Some(result)
    }
}

// ---------------------------------------------------------------------------
// Tree walk
// ---------------------------------------------------------------------------

/// Whether `migrate_tree` writes a pre-rewrite backup copy of every file it
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupMode {
    /// Lazily create `<root>.migration-backup-<unix secs>/` on the first
    /// rewrite of this call, and copy the ORIGINAL bytes of every rewritten
    /// file into it at the same path relative to `root`.
    Mirror,
    /// Perform the rewrites; create no backup directory at all.
    None,
}

/// Outcome of attempting to migrate one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationAction {
    /// The file's content changed; `backup` is the path of its pre-rewrite
    /// copy when [`BackupMode::Mirror`] was in effect, `None` under
    /// [`BackupMode::None`].
    Rewritten {
        path: PathBuf,
        backup: Option<PathBuf>,
    },
    /// The file was read and matched no rule; nothing was written.
    Unchanged,
    /// The entry's canonical path resolves outside `root` (a symlink escape,
    /// or an unreadable/unresolvable path) — skipped and reported, never
    /// read or written.
    SkippedOutsideRoot { path: PathBuf },
}

/// Aggregated result of a [`migrate_tree`] call.
#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    pub actions: Vec<MigrationAction>,
    /// The mirrored backup directory created for this run, if any rewrite
    /// occurred under [`BackupMode::Mirror`]. `None` when no rewrite
    /// happened at all, or when [`BackupMode::None`] was in effect.
    pub backup_root: Option<PathBuf>,
}

impl MigrationReport {
    /// Count of [`MigrationAction::Rewritten`] actions.
    pub fn rewritten_count(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a, MigrationAction::Rewritten { .. }))
            .count()
    }

    /// Count of [`MigrationAction::SkippedOutsideRoot`] actions.
    pub fn skipped_count(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a, MigrationAction::SkippedOutsideRoot { .. }))
            .count()
    }

    /// Count of [`MigrationAction::Unchanged`] actions.
    pub fn unchanged_count(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a, MigrationAction::Unchanged))
            .count()
    }
}

/// The suffix appended to a file's full path (not via `with_extension`, which
/// would clobber an existing extension like `.md`) to build its atomic-write
/// temp sibling. Never left on disk after a successful run.
const TEMP_SUFFIX: &str = ".migrate-tmp";

/// Walk `root` and apply [`rewrite_stale_paths`] to every file found,
/// containing the walk to `root` (a symlink or hardlink that resolves
/// outside it is skipped and reported, never followed into a write), and
/// writing every change through a temp-file-then-rename.
///
/// Returns an empty report immediately (not an error) when `root` does not
/// exist — an operator with no `optional-skills` tree is not an error
/// condition.
pub fn migrate_tree(root: &Path, mode: BackupMode) -> anyhow::Result<MigrationReport> {
    if !root.exists() {
        return Ok(MigrationReport::default());
    }

    let canonical_root = root.canonicalize()?;
    let mut actions: Vec<MigrationAction> = Vec::new();
    let mut backup_root: Option<PathBuf> = None;

    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        let file_type = entry.file_type();
        if file_type.is_dir() {
            continue;
        }
        let path = entry.path();

        let canonical_path = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                actions.push(MigrationAction::SkippedOutsideRoot {
                    path: path.to_path_buf(),
                });
                continue;
            }
        };
        if !canonical_path.starts_with(&canonical_root) {
            actions.push(MigrationAction::SkippedOutsideRoot {
                path: path.to_path_buf(),
            });
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                // Non-UTF-8 or otherwise unreadable: treat as unchanged
                // rather than erroring the whole run.
                actions.push(MigrationAction::Unchanged);
                continue;
            }
        };

        let Some(new_content) = rewrite_stale_paths(&content) else {
            actions.push(MigrationAction::Unchanged);
            continue;
        };

        let backup_path = if mode == BackupMode::Mirror {
            let br = match &backup_root {
                Some(existing) => existing.clone(),
                None => {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let mut br_name = root.as_os_str().to_os_string();
                    br_name.push(format!(".migration-backup-{ts}"));
                    let br = PathBuf::from(br_name);
                    backup_root = Some(br.clone());
                    br
                }
            };
            let rel = path.strip_prefix(root).unwrap_or(path);
            let backup_file = br.join(rel);
            if let Some(parent) = backup_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(path, &backup_file)?;
            Some(backup_file)
        } else {
            None
        };

        let mut tmp_name = path.as_os_str().to_os_string();
        tmp_name.push(TEMP_SUFFIX);
        let tmp_path = PathBuf::from(tmp_name);
        std::fs::write(&tmp_path, &new_content)?;
        std::fs::rename(&tmp_path, path)?;

        actions.push(MigrationAction::Rewritten {
            path: path.to_path_buf(),
            backup: backup_path,
        });
    }

    Ok(MigrationReport {
        actions,
        backup_root,
    })
}

// ---------------------------------------------------------------------------
// Tests (Phase 54.1 Plan 04 Task 3)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Operator-authored content that must survive every rewrite byte-for-byte.
    const OPERATOR_PARAGRAPH: &str =
        "This is an operator-authored paragraph that the migration must never touch, byte for byte.";

    // Bucket B (root-pinned skill-tree path, R1).
    const BUCKET_B_ORIGINAL: &str = "source ~/.hermes/skills/category/skillname/scripts/helper.sh";
    const BUCKET_B_REPLACEMENT: &str =
        "source ~/.ironhermes/skills/category/skillname/scripts/helper.sh";

    // Bucket A (credentials/state, per-profile, R2).
    const BUCKET_A_ORIGINAL: &str = "Add to `~/.hermes/config.yaml`:";
    const BUCKET_A_REPLACEMENT: &str =
        "Add to `${IRONHERMES_HOME:-$HOME/.ironhermes}/config.yaml`:";

    // Bucket C (workspace-relative marker, no home prefix, R3).
    const BUCKET_C_ORIGINAL: &str =
        "Your deliverable is a markdown plan saved under `.hermes/plans/`.";
    const BUCKET_C_REPLACEMENT: &str =
        "Your deliverable is a markdown plan saved under `.ironhermes/plans/`.";

    // Bucket D (removed env var + its quoted-Python-default companion, R4+R5).
    const BUCKET_D_ORIGINAL: &str =
        "HERMES_HOME = Path(os.getenv(\"HERMES_HOME\", Path.home() / \".hermes\"))";
    const BUCKET_D_REPLACEMENT: &str =
        "IRONHERMES_HOME = Path(os.getenv(\"IRONHERMES_HOME\", Path.home() / \".ironhermes\"))";

    const CLEAN_CONTENT: &str = "nothing special here, no stale references at all.\n";

    /// Seeds a fixture tree shaped like an installed catalog:
    /// - `<root>/category/skillname/SKILL.md` — bucket B + operator paragraph + bucket A
    /// - `<root>/category/skillname/scripts/setup.py` — bucket D
    /// - `<root>/category/skillname/scripts/clean.txt` — no stale content at all
    /// - `<root>/software-development/plan/SKILL.md` — bucket C
    ///
    /// Returns the owning [`tempfile::TempDir`] — keep it alive for the
    /// duration of the test; its `Drop` removes the fixture tree.
    fn seed_fixture_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        let skill_dir = root.join("category").join("skillname");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        let skill_md = format!(
            "{BUCKET_B_ORIGINAL}\n{OPERATOR_PARAGRAPH}\n{BUCKET_A_ORIGINAL}\n"
        );
        std::fs::write(skill_dir.join("SKILL.md"), skill_md).expect("write SKILL.md");

        let scripts_dir = skill_dir.join("scripts");
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        std::fs::write(
            scripts_dir.join("setup.py"),
            format!("{BUCKET_D_ORIGINAL}\n"),
        )
        .expect("write setup.py");
        std::fs::write(scripts_dir.join("clean.txt"), CLEAN_CONTENT).expect("write clean.txt");

        let plan_dir = root.join("software-development").join("plan");
        std::fs::create_dir_all(&plan_dir).expect("create plan dir");
        std::fs::write(plan_dir.join("SKILL.md"), format!("{BUCKET_C_ORIGINAL}\n"))
            .expect("write plan SKILL.md");

        dir
    }

    /// Backup directories matching THIS root's own unique prefix only — never a
    /// bare substring scan of the parent tmp directory, which would also match
    /// sibling backup directories left behind by other tests (`tempfile`'s
    /// random names make each root's own prefix unique, so this scoping is
    /// exact and collision-free across tests run in the same process).
    fn backup_dirs_for(root: &Path) -> Vec<PathBuf> {
        let root_name = root
            .file_name()
            .expect("root has a file name")
            .to_string_lossy()
            .into_owned();
        let prefix = format!("{root_name}.migration-backup-");
        let parent = root.parent().expect("root has a parent");
        std::fs::read_dir(parent)
            .expect("read parent dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(&prefix))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Asserts no path anywhere under `root` ends in the atomic-write temp
    /// suffix — a stray `*.migrate-tmp` file would mean a run was interrupted
    /// between the write and the rename.
    fn assert_no_temp_files(root: &Path) {
        for entry in walkdir::WalkDir::new(root) {
            let entry = entry.expect("walk fixture tree");
            let name = entry.file_name().to_string_lossy();
            assert!(
                !name.ends_with(TEMP_SUFFIX),
                "stray temp file left behind: {}",
                entry.path().display()
            );
        }
    }

    /// True when `text` contains a bare `$IRONHERMES_HOME` reference — the
    /// dollar sign directly followed by the identifier, with no `{` in
    /// between. Scoped to the specific var this migration introduces (per
    /// Plan 03's own `grep -rIE '[$]IRONHERMES_HOME([^A-Za-z0-9_]|$)'`
    /// verification and T-54.1-18's threat rationale) rather than every
    /// `$`-prefixed identifier: the mandatory `${IRONHERMES_HOME:-$HOME/...}`
    /// form legitimately contains a BARE `$HOME` inside its fallback clause,
    /// which must never be flagged. `${IRONHERMES_HOME...}` does NOT match
    /// this pattern — right after `$` comes `{`, not `I`.
    fn has_bare_ironhermes_home_reference(text: &str) -> bool {
        static BARE_VAR_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\$IRONHERMES_HOME").expect("BARE_VAR_RE"));
        BARE_VAR_RE.is_match(text)
    }

    #[test]
    fn skills_migrate_rewrites_each_bucket_to_its_manifest_replacement() {
        let dir = seed_fixture_tree();
        let root = dir.path();

        let report = migrate_tree(root, BackupMode::None).expect("migrate_tree");
        assert_eq!(report.rewritten_count(), 3, "SKILL.md + setup.py + plan SKILL.md");
        assert_eq!(report.unchanged_count(), 1, "clean.txt has no stale content");

        let skill_md =
            std::fs::read_to_string(root.join("category/skillname/SKILL.md")).unwrap();
        assert!(skill_md.contains(BUCKET_B_REPLACEMENT));
        assert!(!skill_md.contains(BUCKET_B_ORIGINAL));
        assert!(skill_md.contains(BUCKET_A_REPLACEMENT));
        assert!(!skill_md.contains(BUCKET_A_ORIGINAL));

        let setup_py =
            std::fs::read_to_string(root.join("category/skillname/scripts/setup.py")).unwrap();
        assert!(setup_py.contains(BUCKET_D_REPLACEMENT));
        assert!(!setup_py.contains(BUCKET_D_ORIGINAL));

        let plan_md =
            std::fs::read_to_string(root.join("software-development/plan/SKILL.md")).unwrap();
        assert!(plan_md.contains(BUCKET_C_REPLACEMENT));
        assert!(!plan_md.contains(BUCKET_C_ORIGINAL));
    }

    #[test]
    fn skills_migrate_preserves_operator_added_content() {
        let dir = seed_fixture_tree();
        let root = dir.path();
        let skill_md_path = root.join("category/skillname/SKILL.md");
        let before = std::fs::read_to_string(&skill_md_path).unwrap();

        let report = migrate_tree(root, BackupMode::None).expect("migrate_tree");
        assert!(report.rewritten_count() >= 1);

        let after = std::fs::read_to_string(&skill_md_path).unwrap();
        assert!(
            after.contains(OPERATOR_PARAGRAPH),
            "operator paragraph must survive byte-for-byte"
        );

        let delta_b = BUCKET_B_REPLACEMENT.len() as i64 - BUCKET_B_ORIGINAL.len() as i64;
        let delta_a = BUCKET_A_REPLACEMENT.len() as i64 - BUCKET_A_ORIGINAL.len() as i64;
        let expected_delta = delta_b + delta_a;
        let actual_delta = after.len() as i64 - before.len() as i64;
        assert_eq!(
            actual_delta, expected_delta,
            "file length must change by exactly the sum of the per-occurrence length deltas"
        );
    }

    #[test]
    fn skills_migrate_second_run_reports_zero_changes() {
        let dir = seed_fixture_tree();
        let root = dir.path();

        let first = migrate_tree(root, BackupMode::Mirror).expect("first migrate_tree");
        assert!(first.rewritten_count() > 0);
        assert!(first.backup_root.is_some());

        let second = migrate_tree(root, BackupMode::Mirror).expect("second migrate_tree");
        assert_eq!(second.rewritten_count(), 0);

        let backups = backup_dirs_for(root);
        assert_eq!(
            backups.len(),
            1,
            "exactly one backup directory should exist after two runs"
        );

        assert_no_temp_files(root);
    }

    #[test]
    fn skills_migrate_rules_are_a_fixed_point() {
        let combined = format!(
            "{BUCKET_B_ORIGINAL}\n{BUCKET_A_ORIGINAL}\n{BUCKET_C_ORIGINAL}\n{BUCKET_D_ORIGINAL}\n"
        );
        let once = rewrite_stale_paths(&combined).expect("first rewrite must change something");
        let twice = rewrite_stale_paths(&once);
        assert_eq!(
            twice, None,
            "a second rewrite of already-rewritten content must be a no-op"
        );
    }

    /// R8 (Phase 54.1 Plan 05 Task 3, Rule 1 deviation): a shell-style bare
    /// `$HERMES_HOME` mention with NO accompanying code-shaped default
    /// assignment on the same line — the exact pattern R4-R7 alone leave
    /// unwrapped, discovered via the real operator-tree migration run (7
    /// residual bare `$IRONHERMES_HOME` references across
    /// `qis-communication-jta/scripts/numbering.py`,
    /// `touchdesigner-mcp/references/troubleshooting.md`, `honcho/SKILL.md`,
    /// `memento_cards.py`, and `canvas/SKILL.md`).
    #[test]
    fn skills_migrate_wraps_bare_shell_style_variable_reference() {
        let original = "CANVAS=\"python $HERMES_HOME/skills/productivity/canvas/scripts/canvas_api.py\"";
        let rewritten = rewrite_stale_paths(original).expect("rewrite must change something");
        assert_eq!(
            rewritten,
            "CANVAS=\"python ${IRONHERMES_HOME:-$HOME/.ironhermes}/skills/productivity/canvas/scripts/canvas_api.py\""
        );
        assert!(!has_bare_ironhermes_home_reference(&rewritten));

        // Idempotent on a second pass over the already-wrapped output.
        assert_eq!(rewrite_stale_paths(&rewritten), None);
    }

    /// R8 must self-heal a bare reference left behind by a PRIOR run — i.e.
    /// one where `R4_RE` no longer matches because the rename already
    /// happened (the word is already `IRONHERMES_HOME`, not `HERMES_HOME`),
    /// which is exactly the shape a run with this fix applied encounters
    /// against the operator's already-partially-migrated real tree.
    #[test]
    fn skills_migrate_r8_self_heals_bare_reference_r4_no_longer_matches() {
        let already_renamed_but_bare =
            "Data file: $IRONHERMES_HOME/skills/productivity/memento-flashcards/data/cards.json";
        assert!(
            !R4_RE.is_match(already_renamed_but_bare),
            "precondition: this line must not contain the word HERMES_HOME at all"
        );
        let rewritten =
            rewrite_stale_paths(already_renamed_but_bare).expect("rewrite must change something");
        assert!(!has_bare_ironhermes_home_reference(&rewritten));
        assert_eq!(
            rewritten,
            "Data file: ${IRONHERMES_HOME:-$HOME/.ironhermes}/skills/productivity/memento-flashcards/data/cards.json"
        );
    }

    #[test]
    fn skills_migrate_never_emits_bare_variable_reference() {
        let combined = format!(
            "{BUCKET_B_ORIGINAL}\n{BUCKET_A_ORIGINAL}\n{BUCKET_C_ORIGINAL}\n{BUCKET_D_ORIGINAL}\n"
        );
        let rewritten = rewrite_stale_paths(&combined).expect("rewrite must change something");
        assert!(
            !has_bare_ironhermes_home_reference(&rewritten),
            "rewritten output must never contain a bare $IRONHERMES_HOME reference: {rewritten}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn skills_migrate_refuses_path_escaping_root_via_symlink() {
        let dir = seed_fixture_tree();
        let root = dir.path();

        let sibling_dir = tempfile::tempdir().expect("sibling tempdir");
        let sibling_file = sibling_dir.path().join("outside.txt");
        std::fs::write(&sibling_file, BUCKET_A_ORIGINAL).expect("write sibling file");

        let link_path = root.join("escape-link.txt");
        std::os::unix::fs::symlink(&sibling_file, &link_path).expect("create symlink");

        let report = migrate_tree(root, BackupMode::None).expect("migrate_tree");
        let skipped = report.actions.iter().any(|a| {
            matches!(a, MigrationAction::SkippedOutsideRoot { path } if path == &link_path)
        });
        assert!(
            skipped,
            "the symlink entry must be reported as SkippedOutsideRoot: {:?}",
            report.actions
        );

        let sibling_bytes = std::fs::read_to_string(&sibling_file).unwrap();
        assert_eq!(
            sibling_bytes, BUCKET_A_ORIGINAL,
            "the escape target must be unmodified"
        );
    }

    #[test]
    fn skills_migrate_backup_mirror_written_only_for_rewritten_files() {
        let dir = seed_fixture_tree();
        let root = dir.path();

        let skill_md_path = root.join("category/skillname/SKILL.md");
        let skill_md_before = std::fs::read_to_string(&skill_md_path).unwrap();

        let clean_path = root.join("category/skillname/scripts/clean.txt");
        let clean_before = std::fs::read_to_string(&clean_path).unwrap();

        let report = migrate_tree(root, BackupMode::Mirror).expect("migrate_tree");
        let backup_root = report.backup_root.clone().expect("backup root must exist");

        let backup_skill_md = backup_root.join("category/skillname/SKILL.md");
        let backup_contents = std::fs::read_to_string(&backup_skill_md).unwrap();
        assert_eq!(
            backup_contents, skill_md_before,
            "the backup copy must hold the PRE-migration bytes"
        );

        let backup_clean = backup_root.join("category/skillname/scripts/clean.txt");
        assert!(
            !backup_clean.exists(),
            "an unchanged file must not be backed up"
        );

        let clean_after = std::fs::read_to_string(&clean_path).unwrap();
        assert_eq!(clean_after, clean_before, "unchanged file must be untouched on disk");
    }

    #[test]
    fn skills_migrate_no_backup_mode_writes_no_mirror() {
        let dir = seed_fixture_tree();
        let root = dir.path();

        let report = migrate_tree(root, BackupMode::None).expect("migrate_tree");
        assert!(report.rewritten_count() > 0);
        assert!(report.backup_root.is_none());

        let backups = backup_dirs_for(root);
        assert!(
            backups.is_empty(),
            "BackupMode::None must create no backup directory"
        );

        assert_no_temp_files(root);
    }

    #[test]
    fn skills_migrate_missing_root_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing_root = dir.path().join("does-not-exist");

        let report = migrate_tree(&missing_root, BackupMode::Mirror).expect("must be Ok");
        assert!(report.actions.is_empty());
        assert!(report.backup_root.is_none());
    }
}
