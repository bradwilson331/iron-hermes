//! Phase 51 Plan 12 Task 1 (CR-01 / T16) — workspace-wide recurrence tripwire.
//!
//! `crates/ironhermes-vault/Cargo.toml` once declared an `[[example]]` target
//! (`seed_profile_secret`, committed in `541afc5d5`) whose source file was never
//! `git add`-ed. Cargo resolves explicitly-declared targets at manifest-parse time —
//! BEFORE feature resolution — so `required-features` does nothing to defer that
//! resolution: every cargo command against the workspace, `cargo metadata` included (and
//! therefore all of CI), failed to parse the manifest on a clean checkout, while passing
//! on every working tree that happened to still have the untracked file on disk.
//!
//! This test makes that class of bug fail loudly instead of silently: for every declared
//! `[[example]]`/`[[bin]]`/`[[test]]`/`[[bench]]` target in every workspace member
//! manifest, resolve its source path (the explicit `path` key when present, otherwise the
//! conventional `<kind-dir>/<name>.rs`) and assert `git ls-files` reports that path as
//! tracked. Member manifests are discovered from `git ls-files '*/Cargo.toml' Cargo.toml`
//! rather than a hardcoded list, so a new crate is covered automatically the day it lands.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `CARGO_MANIFEST_DIR` for this crate is `<root>/crates/ironhermes-vault` — two levels up
/// reaches the workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ironhermes-vault's manifest dir must have a parent (crates/)")
        .parent()
        .expect("crates/ must have a parent (the workspace root)")
        .to_path_buf()
}

/// Every `Cargo.toml` git tracks under `root` — the workspace root's own manifest plus
/// every member/sub-crate manifest. Discovered from `git`, not a hardcoded member list.
fn tracked_manifests(root: &Path) -> Vec<PathBuf> {
    let out = Command::new("git")
        .args(["ls-files", "*/Cargo.toml", "Cargo.toml"])
        .current_dir(root)
        .output()
        .expect("run git ls-files for manifest discovery");
    assert!(
        out.status.success(),
        "git ls-files (manifest discovery) failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| root.join(l))
        .collect()
}

/// Is `relative_path` (relative to `root`) tracked in the git index? This is the exact
/// ground truth `cargo metadata` sees on a checkout built from the index alone — a file
/// present on disk but absent from the index is invisible to a fresh clone.
fn is_tracked(root: &Path, relative_path: &str) -> bool {
    let out = Command::new("git")
        .args(["ls-files", "--error-unmatch", relative_path])
        .current_dir(root)
        .output()
        .expect("run git ls-files --error-unmatch");
    out.status.success()
}

/// The four target kinds cargo resolves eagerly at manifest-parse time (before feature
/// gating), and the conventional source directory for each when no explicit `path` key is
/// given (`Cargo.toml`'s own target auto-discovery convention).
const TARGET_KINDS: &[(&str, &str)] = &[
    ("example", "examples"),
    ("bin", "src/bin"),
    ("test", "tests"),
    ("bench", "benches"),
];

#[test]
fn every_declared_cargo_target_source_is_git_tracked() {
    let root = workspace_root();
    let manifests = tracked_manifests(&root);
    assert!(
        !manifests.is_empty(),
        "git ls-files found no Cargo.toml files under {} — the discovery command itself is \
         broken, which silently disables this entire tripwire",
        root.display()
    );

    let mut checked = 0usize;
    for manifest_path in &manifests {
        let manifest_dir = manifest_path
            .parent()
            .expect("a Cargo.toml path always has a parent directory");
        let manifest_rel = manifest_path
            .strip_prefix(&root)
            .expect("manifest path must be under the workspace root")
            .to_string_lossy()
            .to_string();
        let contents = std::fs::read_to_string(manifest_path)
            .unwrap_or_else(|e| panic!("read {manifest_rel}: {e}"));
        let doc: toml::Value = contents
            .parse()
            .unwrap_or_else(|e| panic!("parse {manifest_rel} as TOML: {e}"));

        for (kind, kind_dir) in TARGET_KINDS {
            let Some(targets) = doc.get(*kind).and_then(|v| v.as_array()) else {
                continue;
            };
            for target in targets {
                let name = target
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| {
                        panic!("{manifest_rel}: a [[{kind}]] target is missing its `name` key")
                    });
                let source_abs = match target.get("path").and_then(|v| v.as_str()) {
                    Some(explicit) => manifest_dir.join(explicit),
                    None => manifest_dir.join(kind_dir).join(format!("{name}.rs")),
                };
                let source_rel = source_abs
                    .strip_prefix(&root)
                    .expect("target source path must be under the workspace root")
                    .to_string_lossy()
                    .to_string();

                checked += 1;
                assert!(
                    is_tracked(&root, &source_rel),
                    "{manifest_rel}: [[{kind}]] target {name:?} resolves to {source_rel}, \
                     which `git ls-files` does not report as tracked. Cargo resolves \
                     explicitly-declared targets at manifest-parse time — BEFORE feature \
                     resolution — so an untracked source here breaks every cargo command \
                     on a fresh checkout (including `cargo metadata`, and therefore all of \
                     CI), not just a build of this specific target."
                );
            }
        }
    }

    assert!(
        checked > 0,
        "no explicitly-declared [[example]]/[[bin]]/[[test]]/[[bench]] targets were found \
         across {} manifests — this test's own coverage may have silently regressed",
        manifests.len()
    );
}
