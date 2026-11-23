//! Tier 2 on-disk layout (D-11): everything under `<home>/tier2/`.
//!
//! Paths derive from the profile home only, never from model input (T-58.2-02), and never name
//! `<home>/memory.db` or `~/.ironhermes/memory/`.

use std::path::{Path, PathBuf};

use ironhermes_core::tier2::TIER2_DIRNAME;

#[derive(Debug, Clone)]
pub struct Tier2Paths {
    pub root: PathBuf,
    pub db: PathBuf,
    pub index: PathBuf,
    pub live: PathBuf,
    pub bank: PathBuf,
    pub people: PathBuf,
    pub groups: PathBuf,
    pub dreams: PathBuf,
    pub forget: PathBuf,
    /// D-26 embedding model cache.
    pub models: PathBuf,
}

impl Tier2Paths {
    /// Pure: no I/O.
    pub fn from_home(home: &Path) -> Self {
        let root = home.join(TIER2_DIRNAME);
        Self {
            db: root.join("tier2.db"),
            index: root.join("index.usearch"),
            live: root.join("live"),
            bank: root.join("bank"),
            people: root.join("people"),
            groups: root.join("groups"),
            dreams: root.join("dreams"),
            forget: root.join("forget"),
            models: root.join("models"),
            root,
        }
    }

    /// Create the root and the seven subdirectories (mode 0700 on unix).
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        for d in [
            &self.root,
            &self.live,
            &self.bank,
            &self.people,
            &self.groups,
            &self.dreams,
            &self.forget,
            &self.models,
        ] {
            b.create(d)?;
        }
        Ok(())
    }
}

/// Take a blocking exclusive lock on `path` (created if missing) until the returned handle
/// drops. Separate handles conflict even inside one process, so it serialises threads as well as
/// processes. Never truncates or writes, so a planted symlink's target is not modified (T-58.2-52).
pub(crate) fn lock_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    let f = open_lock_file(path)?;
    f.lock()?;
    Ok(f)
}

/// Non-blocking twin of `lock_exclusive`: `None` when another holder has it. For callers that run
/// on an async thread and must never wait.
pub(crate) fn try_lock_exclusive(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let f = open_lock_file(path)?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

fn open_lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn all(p: &Tier2Paths) -> Vec<&Path> {
        vec![
            &p.root, &p.db, &p.index, &p.live, &p.bank, &p.people, &p.groups, &p.dreams, &p.forget,
            &p.models,
        ]
        .into_iter()
        .map(|p| p.as_path())
        .collect()
    }

    #[test]
    fn paths_layout_under_home() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        let p = Tier2Paths::from_home(h);
        assert_eq!(p.root, h.join("tier2"));
        assert_eq!(p.db, h.join("tier2/tier2.db"));
        assert_eq!(p.index, h.join("tier2/index.usearch"));
        for (got, name) in [
            (&p.live, "live"),
            (&p.bank, "bank"),
            (&p.people, "people"),
            (&p.groups, "groups"),
            (&p.dreams, "dreams"),
            (&p.forget, "forget"),
            (&p.models, "models"),
        ] {
            assert_eq!(got, &h.join("tier2").join(name));
        }
        p.ensure_dirs().unwrap();
        assert!(p.root.is_dir());
        for d in [
            &p.live, &p.bank, &p.people, &p.groups, &p.dreams, &p.forget, &p.models,
        ] {
            assert!(d.is_dir(), "{} missing", d.display());
        }
    }

    #[test]
    fn lock_exclusive_serializes_holders() {
        use std::time::Duration;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub/x.lock");
        let first = lock_exclusive(&path).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let p2 = path.clone();
        let t = std::thread::spawn(move || {
            let _second = lock_exclusive(&p2).unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        drop(first);
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        t.join().unwrap();
    }

    #[test]
    fn paths_two_homes_isolated() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (pa, pb) = (
            Tier2Paths::from_home(a.path()),
            Tier2Paths::from_home(b.path()),
        );
        for x in all(&pa) {
            assert!(
                !x.starts_with(b.path()),
                "{} leaks into home B",
                x.display()
            );
        }
        for x in all(&pb) {
            assert!(
                !x.starts_with(a.path()),
                "{} leaks into home A",
                x.display()
            );
        }
    }

    #[test]
    fn paths_never_memory_db() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Tier2Paths::from_home(tmp.path());
        for x in all(&p) {
            assert_ne!(x, tmp.path().join("memory.db"));
            assert!(
                !x.to_string_lossy().contains("/memory/"),
                "{} lives under a memory/ dir",
                x.display()
            );
        }
    }
}
