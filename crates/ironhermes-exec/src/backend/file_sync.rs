//! `FileTransport` / `FileSyncManager` — shared remote file-sync contract for
//! remote backends (SSH tar-over-SSH this plan; §5.2/§7 of
//! `docs/EXEC-BACKENDS-ARCHITECTURE.md`).
//!
//! Bind-mount backends (Docker, Local) see the host filesystem live and need
//! no file sync at all (§3 of the porting doc) — only remote backends (SSH
//! this phase; Modal/Daytona are out of scope per D-01) implement this
//! trait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Shared file-transport contract a remote backend (SSH — this plan)
/// implements to push/pull files in bulk between the host and the target
/// environment. Every method operates on a *batch* — SSH's impl streams a
/// single `tar` over the ControlMaster socket (§5.2) instead of N
/// individual round-trips.
#[async_trait::async_trait]
pub trait FileTransport: Send + Sync {
    /// Push a batch of `(local_path, remote_path)` pairs into the target
    /// environment in one transport call.
    async fn bulk_upload(&self, files: &[(PathBuf, String)]) -> anyhow::Result<()>;

    /// Pull the target environment's tracked files back to `dest` on the
    /// host, used by [`FileSyncManager::sync_back`] on cleanup.
    async fn bulk_download(&self, dest: &Path) -> anyhow::Result<()>;

    /// Remove the named remote paths (deletion detection — §7).
    async fn delete(&self, paths: &[String]) -> anyhow::Result<()>;
}

/// Default rate-limit window (§7): `sync()` calls inside this window of the
/// last successful sync are skipped, so a tight command loop doesn't push a
/// tar-over-ssh stream before every single command.
pub const DEFAULT_SYNC_RATE_LIMIT: Duration = Duration::from_secs(5);

/// What `FileSyncManager` last observed about a tracked local file: its
/// `(mtime, size)` fingerprint (§7's change-detection key) plus the remote
/// path it was pushed to, so a later removal can be reported back to
/// [`FileTransport::delete`] using the correct remote-side name.
struct Tracked {
    mtime: SystemTime,
    size: u64,
    remote_path: String,
}

/// Backend-agnostic `(mtime, size)` change-tracking file-sync manager (§7).
/// Generic over `T: FileTransport` so SSH (this plan) — and, in principle,
/// any future remote backend — reuse the identical diff/rate-limit logic and
/// only supply their own transport.
pub struct FileSyncManager<T: FileTransport> {
    transport: T,
    /// `local_path -> Tracked` as observed at the last successful sync.
    known: HashMap<PathBuf, Tracked>,
    rate_limit: Duration,
    last_sync: Option<Instant>,
}

impl<T: FileTransport> FileSyncManager<T> {
    /// Construct with the default 5s rate-limit window (§7).
    pub fn new(transport: T) -> Self {
        Self::with_rate_limit(transport, DEFAULT_SYNC_RATE_LIMIT)
    }

    /// Construct with an explicit rate-limit window — used by tests to
    /// disable rate-limiting (`Duration::ZERO`) so back-to-back `sync()`
    /// calls both actually execute the diff/transport logic under test.
    pub fn with_rate_limit(transport: T, rate_limit: Duration) -> Self {
        Self {
            transport,
            known: HashMap::new(),
            rate_limit,
            last_sync: None,
        }
    }

    /// Diff `files` (the caller-supplied current set of tracked
    /// `(local_path, remote_path)` pairs) against the last-known
    /// `(mtime, size)` map: upload any new/changed file via
    /// [`FileTransport::bulk_upload`], and delete any previously-known file
    /// that is absent from `files` via [`FileTransport::delete`]. Skips
    /// entirely (returns `Ok(())` without touching the transport) if called
    /// again inside `rate_limit` of the last successful sync.
    pub async fn sync(&mut self, files: &[(PathBuf, String)]) -> anyhow::Result<()> {
        if let Some(last) = self.last_sync
            && last.elapsed() < self.rate_limit
        {
            return Ok(());
        }

        let mut current: std::collections::HashSet<&PathBuf> = std::collections::HashSet::new();
        let mut changed: Vec<(PathBuf, String)> = Vec::new();

        for (local_path, remote_path) in files {
            current.insert(local_path);

            let metadata = match std::fs::metadata(local_path) {
                Ok(m) => m,
                // Vanished between enumeration and sync — nothing to push;
                // a later sync's removal detection handles it once the
                // caller stops including it in `files`.
                Err(_) => continue,
            };
            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let size = metadata.len();

            let unchanged = self
                .known
                .get(local_path)
                .is_some_and(|tracked| tracked.mtime == mtime && tracked.size == size);

            if !unchanged {
                changed.push((local_path.clone(), remote_path.clone()));
            }

            self.known.insert(
                local_path.clone(),
                Tracked {
                    mtime,
                    size,
                    remote_path: remote_path.clone(),
                },
            );
        }

        let removed: Vec<(PathBuf, String)> = self
            .known
            .iter()
            .filter(|(path, _)| !current.contains(path))
            .map(|(path, tracked)| (path.clone(), tracked.remote_path.clone()))
            .collect();

        if !changed.is_empty() {
            self.transport.bulk_upload(&changed).await?;
        }

        if !removed.is_empty() {
            let removed_remote: Vec<String> = removed.iter().map(|(_, r)| r.clone()).collect();
            self.transport.delete(&removed_remote).await?;
            for (path, _) in &removed {
                self.known.remove(path);
            }
        }

        self.last_sync = Some(Instant::now());
        Ok(())
    }

    /// Pull the remote environment's tracked files back to `dest` (§7's
    /// `sync_back()`, used by `cleanup()`).
    pub async fn sync_back(&self, dest: &Path) -> anyhow::Result<()> {
        self.transport.bulk_download(dest).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type UploadCalls = Arc<Mutex<Vec<Vec<(PathBuf, String)>>>>;
    type DeleteCalls = Arc<Mutex<Vec<Vec<String>>>>;

    /// Records every `FileTransport` call it receives (via a shared `Arc`
    /// handle kept by the test) instead of touching any real network or
    /// filesystem — proves `FileSyncManager`'s diff/rate-limit logic without
    /// a live SSH host (project constraint: no live host in this plan).
    #[derive(Clone, Default)]
    struct FakeTransport {
        uploads: UploadCalls,
        deletes: DeleteCalls,
    }

    #[async_trait::async_trait]
    impl FileTransport for FakeTransport {
        async fn bulk_upload(&self, files: &[(PathBuf, String)]) -> anyhow::Result<()> {
            self.uploads.lock().expect("uploads mutex poisoned").push(files.to_vec());
            Ok(())
        }

        async fn bulk_download(&self, _dest: &Path) -> anyhow::Result<()> {
            Ok(())
        }

        async fn delete(&self, paths: &[String]) -> anyhow::Result<()> {
            self.deletes.lock().expect("deletes mutex poisoned").push(paths.to_vec());
            Ok(())
        }
    }

    #[tokio::test]
    async fn unchanged_file_is_not_reuploaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("a.txt");
        std::fs::write(&file_path, "hello").expect("write file");

        let transport = FakeTransport::default();
        let handle = transport.clone();
        let mut mgr = FileSyncManager::with_rate_limit(transport, Duration::ZERO);

        let files = vec![(file_path.clone(), "remote/a.txt".to_string())];
        mgr.sync(&files).await.expect("first sync");
        mgr.sync(&files).await.expect("second sync — file unchanged");

        let uploads = handle.uploads.lock().expect("uploads mutex poisoned");
        assert_eq!(
            uploads.len(),
            1,
            "an unchanged (mtime,size) file must not be re-uploaded on a second sync: {uploads:?}"
        );
        assert_eq!(uploads[0], files);
    }

    #[tokio::test]
    async fn changed_file_is_reuploaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("a.txt");
        std::fs::write(&file_path, "v1").expect("write file");

        let transport = FakeTransport::default();
        let handle = transport.clone();
        let mut mgr = FileSyncManager::with_rate_limit(transport, Duration::ZERO);

        let files = vec![(file_path.clone(), "remote/a.txt".to_string())];
        mgr.sync(&files).await.expect("first sync");

        // Change size (and therefore the (mtime,size) fingerprint) so the
        // second sync must treat this as changed regardless of filesystem
        // mtime-resolution granularity.
        std::fs::write(&file_path, "v2-is-longer-than-v1").expect("rewrite file");
        mgr.sync(&files).await.expect("second sync — file changed");

        let uploads = handle.uploads.lock().expect("uploads mutex poisoned");
        assert_eq!(
            uploads.len(),
            2,
            "a changed (mtime,size) file must be re-uploaded: {uploads:?}"
        );
    }

    #[tokio::test]
    async fn removed_file_triggers_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("a.txt");
        std::fs::write(&file_path, "hello").expect("write file");

        let transport = FakeTransport::default();
        let handle = transport.clone();
        let mut mgr = FileSyncManager::with_rate_limit(transport, Duration::ZERO);

        let files = vec![(file_path.clone(), "remote/a.txt".to_string())];
        mgr.sync(&files).await.expect("first sync uploads a.txt");

        // Second sync's tracked-file set no longer includes a.txt — this
        // must be detected as a removal and trigger FileTransport::delete
        // with the file's remote path.
        mgr.sync(&[]).await.expect("second sync — a.txt removed");

        let deletes = handle.deletes.lock().expect("deletes mutex poisoned");
        assert_eq!(deletes.len(), 1, "a removed file must trigger exactly one delete call");
        assert_eq!(deletes[0], vec!["remote/a.txt".to_string()]);
    }

    #[tokio::test]
    async fn sync_respects_rate_limit_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("a.txt");
        std::fs::write(&file_path, "v1").expect("write file");

        let transport = FakeTransport::default();
        let handle = transport.clone();
        // Default constructor uses the real 5s rate-limit window.
        let mut mgr = FileSyncManager::new(transport);

        let files = vec![(file_path.clone(), "remote/a.txt".to_string())];
        mgr.sync(&files).await.expect("first sync");

        // Mutate the file and sync again immediately — still inside the 5s
        // window, so this must be skipped even though the file actually
        // changed on disk.
        std::fs::write(&file_path, "v2-longer-than-v1").expect("rewrite file");
        mgr.sync(&files).await.expect("second sync — rate-limited");

        let uploads = handle.uploads.lock().expect("uploads mutex poisoned");
        assert_eq!(
            uploads.len(),
            1,
            "a sync() call inside the rate-limit window must be a no-op: {uploads:?}"
        );
    }

    #[tokio::test]
    async fn sync_back_delegates_to_bulk_download() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transport = FakeTransport::default();
        let mgr = FileSyncManager::new(transport);

        // FakeTransport::bulk_download is a no-op Ok(()); this just proves
        // the manager wires sync_back() straight through without erroring.
        mgr.sync_back(dir.path()).await.expect("sync_back should delegate cleanly");
    }
}
