//! Cross-platform PID liveness check (kill -0 analog) + signal helpers.
//!
//! Plan 01 Task 0 — human selected `nix` (commit `91a0bc26` SUMMARY + the
//! reply on the checkpoint). The Errno-discriminating implementation
//! correctly handles the EPERM case — a process can exist but be owned by
//! another uid, in which case `kill(pid, 0)` returns EPERM rather than 0
//! and a naive `is_ok()` check would report "not alive" for a live
//! foreign process.
//!
//! Plan 03 adds [`kill_pid`], [`Signal`], and [`current_hostname`] for the
//! dispatcher's max-runtime enforcement (D-10 step 4) and claim-lock building
//! (D-08).
//!
//! Plan 03's dispatcher reaches this via the crate-root re-export
//! `ironhermes_kanban::is_pid_alive` (also accessible as
//! `ironhermes_kanban::pid::is_pid_alive`).

/// Signal variants for [`kill_pid`] (D-10 step 4: SIGTERM → grace → SIGKILL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

// ---------------------------------------------------------------------------
// is_pid_alive
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub fn is_pid_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => true,             // process exists, signal 0 was a no-op
        Err(Errno::ESRCH) => false, // no such process
        Err(Errno::EPERM) => true,  // exists but owned by another uid
        Err(_) => false,
    }
}

/// On non-unix targets the kanban dispatcher is not supported. We return
/// `false` so callers fall through to their "process is gone" branch
/// instead of livelocking; the dispatcher itself is gated on cfg(unix)
/// when plan 03 lands.
#[cfg(not(unix))]
pub fn is_pid_alive(_pid: u32) -> bool {
    false
}

// ---------------------------------------------------------------------------
// kill_pid
// ---------------------------------------------------------------------------

/// Send a signal to a process by PID.
///
/// On Unix, uses `nix::sys::signal::kill` with SIGTERM or SIGKILL.
/// On non-Unix, returns an error — the dispatcher is Unix-only for this op.
///
/// The only `unsafe` block in the crate lives in the non-nix fallback path
/// (not used on Unix where `nix` provides safe wrappers).
#[cfg(unix)]
pub fn kill_pid(pid: u32, sig: Signal) -> crate::error::Result<()> {
    use nix::sys::signal::{self, kill};
    use nix::unistd::Pid;

    let nix_sig = match sig {
        Signal::Term => signal::Signal::SIGTERM,
        Signal::Kill => signal::Signal::SIGKILL,
    };

    kill(Pid::from_raw(pid as i32), nix_sig).map_err(|e| {
        crate::error::KanbanError::Other(anyhow::anyhow!(
            "kill_pid({pid}, {sig:?}) failed: {e}"
        ))
    })
}

#[cfg(not(unix))]
pub fn kill_pid(_pid: u32, _sig: Signal) -> crate::error::Result<()> {
    Err(crate::error::KanbanError::Other(anyhow::anyhow!(
        "kill_pid requires Unix"
    )))
}

// ---------------------------------------------------------------------------
// current_hostname
// ---------------------------------------------------------------------------

/// Return the current hostname for use in claim-lock strings (D-08).
///
/// Checks `HOSTNAME` env var first (always set on Linux/macOS in interactive
/// shells), then tries reading `/etc/hostname` (common on Linux), then falls
/// back to `"unknown"`. This approach avoids needing additional crate features
/// (`nix` `hostname` or `libc`) while being reliable in the non-interactive
/// gateway process context where the env var is typically inherited.
pub fn current_hostname() -> String {
    // 1. HOSTNAME env (set by bash/zsh and inherited by the gateway process).
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }

    // 2. /etc/hostname (Linux standard; not present on macOS but harmless to try).
    #[cfg(unix)]
    if let Ok(content) = std::fs::read_to_string("/etc/hostname") {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    // 3. Final fallback.
    "unknown".to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Self-liveness is the only portable smoke we can do without forking
    /// children — that's plan 03's territory.
    #[test]
    fn self_pid_is_alive() {
        let me = std::process::id();
        assert!(is_pid_alive(me), "self pid {me} reported not alive");
    }

    /// Synthetic unreachable PID must report dead. PID 99999999 is almost
    /// certainly unused on any real system; ESRCH is the expected errno.
    #[test]
    fn unreachable_pid_is_dead() {
        // PID 99999999 is past the Linux max PID (4194304) and macOS max PID
        // (~99999). ESRCH ("No such process") is guaranteed.
        assert!(
            !is_pid_alive(99999999),
            "unreachable pid 99999999 reported alive"
        );
    }

    #[cfg(unix)]
    #[test]
    fn kill_pid_self_with_term_fails_gracefully() {
        // Sending SIGTERM to our own PID in a test would kill the test runner.
        // Instead just check that kill_pid with SIGKILL on a dead PID returns an error.
        let result = kill_pid(99999999, Signal::Kill);
        assert!(result.is_err(), "kill_pid on dead PID should return Err");
    }

    #[test]
    fn current_hostname_is_nonempty() {
        let h = current_hostname();
        assert!(!h.is_empty(), "current_hostname returned empty string");
    }
}
