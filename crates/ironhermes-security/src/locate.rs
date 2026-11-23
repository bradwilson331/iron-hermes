//! Locating the `ironhermes-tirith` sidecar binary.
//!
//! Order: a sibling of the running executable (as invoked, then with symlinks
//! resolved), then the `IRONHERMES_TIRITH_BIN` override, then a `PATH` search. The sibling comes first on purpose: kanban
//! workers run with a scrubbed environment (`worker_spawn.rs`), so the lookup
//! that works there must not need any environment at all. A plain `cargo build`
//! puts both binaries in the same `target/<profile>/` directory.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// File stem of the sidecar binary.
pub const SIDECAR_BIN_NAME: &str = "ironhermes-tirith";

/// Optional operator override naming the sidecar binary.
pub const SIDECAR_ENV_OVERRIDE: &str = "IRONHERMES_TIRITH_BIN";

fn exe_name() -> String {
    format!("{SIDECAR_BIN_NAME}{}", std::env::consts::EXE_SUFFIX)
}

/// Pure lookup used by [`locate_sidecar_binary`]; every input is a parameter so a
/// test can drive it over a temp-dir layout.
pub fn locate_from(
    current_exe: Option<&Path>,
    env_override: Option<&OsStr>,
    path_var: Option<&OsStr>,
) -> Option<PathBuf> {
    let name = exe_name();

    // The directory the executable was invoked from, then the directory of its real
    // file. On macOS `current_exe()` returns a symlink's own path, so an install that
    // links `~/.local/bin/ironhermes` to `target/debug/ironhermes` needs the second.
    let resolved = current_exe.and_then(|exe| std::fs::canonicalize(exe).ok());
    for exe in [current_exe, resolved.as_deref()].into_iter().flatten() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(&name);
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }

    if let Some(over) = env_override.filter(|v| !v.is_empty()) {
        let over = PathBuf::from(over);
        if over.is_file() {
            return Some(over);
        }
    }

    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(path_var) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join(&name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Locate the sidecar using the real process environment.
pub fn locate_sidecar_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let over = std::env::var_os(SIDECAR_ENV_OVERRIDE);
    let path = std::env::var_os("PATH");
    locate_from(exe.as_deref(), over.as_deref(), path.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, b"#!/bin/sh\n").unwrap();
        p
    }

    #[test]
    fn locate_prefers_sibling_then_env_then_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let name = exe_name();

        let bin_dir = root.join("bin");
        let exe = touch(&bin_dir, "ironhermes");
        let sibling = touch(&bin_dir, &name);
        let env_bin = touch(&root.join("env"), "custom-tirith");
        let path_dir = root.join("pathdir");
        let on_path = touch(&path_dir, &name);
        let path_var = std::env::join_paths([&path_dir]).unwrap();

        // Sibling wins over both the override and PATH.
        assert_eq!(
            locate_from(Some(&exe), Some(env_bin.as_os_str()), Some(&path_var)),
            Some(sibling.clone())
        );

        // No sibling: the override wins over PATH.
        fs::remove_file(&sibling).unwrap();
        assert_eq!(
            locate_from(Some(&exe), Some(env_bin.as_os_str()), Some(&path_var)),
            Some(env_bin.clone())
        );

        // An override naming a missing file falls through to PATH.
        let missing = root.join("env").join("does-not-exist");
        assert_eq!(
            locate_from(Some(&exe), Some(missing.as_os_str()), Some(&path_var)),
            Some(on_path.clone())
        );

        // An empty override is ignored.
        assert_eq!(
            locate_from(Some(&exe), Some(OsStr::new("")), Some(&path_var)),
            Some(on_path.clone())
        );

        // Nothing anywhere.
        fs::remove_file(&on_path).unwrap();
        assert_eq!(
            locate_from(Some(&exe), Some(missing.as_os_str()), Some(&path_var)),
            None
        );
        assert_eq!(locate_from(None, None, None), None);
    }

    /// `~/.local/bin/ironhermes` symlinked to `target/debug/ironhermes`: on macOS
    /// `current_exe()` returns the symlink path, so the sidecar sitting next to the
    /// real binary must still be found. The symlink's own directory wins when it also
    /// holds a sidecar, and the override and PATH stay later in the order.
    #[cfg(unix)]
    #[test]
    fn locate_follows_a_symlinked_executable_to_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let name = exe_name();

        let real_dir = root.join("target").join("debug");
        let real_exe = touch(&real_dir, "ironhermes");
        let real_sidecar = touch(&real_dir, &name);
        let link_dir = root.join("local").join("bin");
        fs::create_dir_all(&link_dir).unwrap();
        let link = link_dir.join("ironhermes");
        std::os::unix::fs::symlink(&real_exe, &link).unwrap();
        let env_bin = touch(&root.join("env"), "custom-tirith");

        // No sidecar next to the symlink: the one next to the target is found, ahead
        // of the override.
        let found = locate_from(Some(&link), Some(env_bin.as_os_str()), None)
            .expect("sidecar next to the symlink target");
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(&real_sidecar).unwrap()
        );

        // A sidecar next to the symlink itself still wins.
        let link_sidecar = touch(&link_dir, &name);
        assert_eq!(
            locate_from(Some(&link), Some(env_bin.as_os_str()), None),
            Some(link_sidecar)
        );

        // A dangling symlink resolves nowhere and falls through to the override.
        fs::remove_file(link_dir.join(&name)).unwrap();
        fs::remove_file(&real_exe).unwrap();
        assert_eq!(
            locate_from(Some(&link), Some(env_bin.as_os_str()), None),
            Some(env_bin)
        );
    }
}
