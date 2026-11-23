//! Append-only live log (`<home>/tier2/live/YYYY-MM-DD.md`): the citation target of every
//! consolidated claim (`memory://live/YYYY-MM-DD.md#L<n>`).

use std::io::Write;
use std::path::Path;

use anyhow::Context;
use chrono::NaiveDate;

/// `memory://live/YYYY-MM-DD.md#L<line>`.
pub fn live_url(date: NaiveDate, line: usize) -> String {
    format!("memory://live/{date}.md#L{line}")
}

/// The one line format every live-log writer uses: `- [<claim id>] <text>`, whitespace collapsed
/// so the line never holds a newline (Tier 1 entries can span lines). The later purge (D-32)
/// retracts lines by the `[claim:<id>]` marker.
pub fn claim_line(claim_id: &str, text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("- [{claim_id}] {text}")
}

/// What every line citing a purged claim becomes (D-32). One line stays one line, so the `#L<n>`
/// citations of every other claim keep pointing at their own text.
pub const RETRACTED_LINE: &str = "<!-- retracted -->";

/// D-32: replace every line of every day file that cites one of `claim_ids` (`[<claim id>]`, the
/// [`claim_line`] shape shared by consolidation lines, D-29 moves and D-31 notes) with
/// [`RETRACTED_LINE`]. Holds the append lock across all reads and rewrites (tmp + rename), so no
/// concurrent append is lost. Returns the number of lines replaced. Blocking.
///
/// `pub(crate)`: only the human-run purge may rewrite the log (D-28).
pub(crate) fn retract_claim_lines(live_dir: &Path, claim_ids: &[String]) -> anyhow::Result<usize> {
    if claim_ids.is_empty() {
        return Ok(0);
    }
    let marks: Vec<String> = claim_ids.iter().map(|id| format!("[{id}]")).collect();
    let lock_path = live_dir.join(LIVE_LOCK_FILE);
    let _lock = crate::paths::lock_exclusive(&lock_path)
        .with_context(|| format!("locking {}", lock_path.display()))?;
    let mut replaced = 0;
    for entry in
        std::fs::read_dir(live_dir).with_context(|| format!("reading {}", live_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut hit = 0;
        let mut out: Vec<&str> = Vec::new();
        for l in raw.lines() {
            if marks.iter().any(|m| l.contains(m.as_str())) {
                hit += 1;
                out.push(RETRACTED_LINE);
            } else {
                out.push(l);
            }
        }
        if hit == 0 {
            continue;
        }
        let mut text = out.join("\n");
        if raw.ends_with('\n') {
            text.push('\n');
        }
        // Durable: a short file here would destroy every other claim's line (WR-04).
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        crate::render::write_atomic(live_dir, &name, &text)?;
        replaced += hit;
    }
    Ok(replaced)
}

/// Lock file (inside the live dir) every append holds from the line count to the fsync.
pub const LIVE_LOCK_FILE: &str = ".append.lock";

/// Append `lines` to the day's file (created with a `# Live memory YYYY-MM-DD` header on line 1)
/// and fsync. Returns the 1-based line number of each appended line. Lines must not contain
/// newlines (callers normalise whitespace).
///
/// Every live-log writer goes through here, and the cross-process lock held for the whole call is
/// why the returned line numbers are exact across processes. Call it only from a blocking thread;
/// the lock is not re-entrant, so a caller must never already hold it.
pub fn append_lines(
    live_dir: &Path,
    date: NaiveDate,
    lines: &[String],
) -> anyhow::Result<Vec<usize>> {
    let lock_path = live_dir.join(LIVE_LOCK_FILE);
    let _lock = crate::paths::lock_exclusive(&lock_path)
        .with_context(|| format!("locking {}", lock_path.display()))?;
    let path = live_dir.join(format!("{date}.md"));
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut out = String::new();
    let mut n = existing.lines().count();
    if existing.is_empty() {
        out.push_str(&format!("# Live memory {date}\n"));
        n = 1;
    } else if !existing.ends_with('\n') {
        out.push('\n');
    }
    let mut numbers = Vec::with_capacity(lines.len());
    for l in lines {
        n += 1;
        numbers.push(n);
        out.push_str(l);
        out.push('\n');
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    f.write_all(out.as_bytes())?;
    f.sync_all()?;
    Ok(numbers)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WR-04: the rewrite keeps the day file's mode and leaves no temp file.
    #[cfg(unix)]
    #[test]
    fn retract_keeps_mode_and_leaves_no_tmp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let id = "claim:0123456789ab".to_string();
        let file = dir.path().join("2026-01-01.md");
        std::fs::write(
            &file,
            format!("# h\n- [{id}] x\n- [claim:aaaaaaaaaaaa] y\n"),
        )
        .unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(retract_claim_lines(dir.path(), &[id]).unwrap(), 1);
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(names.is_empty(), "{names:?}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# h\n<!-- retracted -->\n- [claim:aaaaaaaaaaaa] y\n"
        );
    }

    /// T-58.2-79: the rewrite waits for the append lock, so an in-flight append is never lost.
    #[test]
    fn retract_waits_for_the_append_lock() {
        let dir = tempfile::tempdir().unwrap();
        let id = "claim:0123456789ab".to_string();
        let file = dir.path().join("2026-01-01.md");
        std::fs::write(&file, format!("# h\n- [{id}] x\n")).unwrap();
        let held = crate::paths::lock_exclusive(&dir.path().join(LIVE_LOCK_FILE)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let p = dir.path().to_path_buf();
        let t = std::thread::spawn(move || tx.send(retract_claim_lines(&p, &[id]).unwrap()));
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "retraction ran while the append lock was held"
        );
        drop(held);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(1));
        t.join().unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# h\n<!-- retracted -->\n"
        );
    }
}
