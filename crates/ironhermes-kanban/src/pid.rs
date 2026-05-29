//! Cross-platform PID liveness check (kill -0 analog).
//!
//! Plan 01 Task 0 — human selected `nix` (commit `91a0bc26` SUMMARY + the
//! reply on the checkpoint). The Errno-discriminating implementation
//! correctly handles the EPERM case — a process can exist but be owned by
//! another uid, in which case `kill(pid, 0)` returns EPERM rather than 0
//! and a naive `is_ok()` check would report "not alive" for a live
//! foreign process.
//!
//! Plan 03's dispatcher reaches this via the crate-root re-export
//! `ironhermes_kanban::is_pid_alive` (also accessible as
//! `ironhermes_kanban::pid::is_pid_alive`).

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
}
