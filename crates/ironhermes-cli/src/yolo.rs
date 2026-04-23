//! --yolo flag resolution + banner emission (D-11/D-12/D-14).
//!
//! Public API re-exported by `crate::lib` so tests and main binaries can
//! both drive the same code path via `ironhermes_cli::{resolve_yolo,
//! maybe_print_yolo_banner, print_yolo_banner_to_stderr}`.
//!
//! Design rules (Phase 21.7 Plan 08):
//! - Gateway NEVER reads a per-request yolo flag (INV-21.7-05); the
//!   precedence resolver is config-only from the gateway path (CLI flag
//!   is always `false` for the gateway subcommand).
//! - `maybe_print_yolo_banner` takes a generic `Write` so it can be
//!   unit-tested against `Vec<u8>` (ISS-07 — replaces assert_cmd stub).
//! - Banner is bold red; the text includes the three unskippable stops
//!   (iteration budget, fatal error, user interrupt — G-01/G-04/G-09).

use std::io::Write;

/// Resolve the effective yolo state from the CLI flag + config (D-12).
///
/// Precedence: CLI flag > config > disabled. The second tuple element
/// identifies the winning source as a `&'static str` suitable for
/// banner/log annotations: `"flag" | "config" | "disabled"`.
pub fn resolve_yolo(flag: bool, config_yolo: bool) -> (bool, &'static str) {
    if flag {
        (true, "flag")
    } else if config_yolo {
        (true, "config")
    } else {
        (false, "disabled")
    }
}

/// Write the yolo-enabled banner when `enabled == true`.
///
/// When `enabled == false`, writes nothing and returns `Ok(())`. Takes a
/// generic `Write` so tests can drive it with a `Vec<u8>` buffer (ISS-07).
///
/// The banner reminds the operator of the three unskippable stops so
/// there is no misunderstanding that the iteration budget, fatal errors,
/// and user interrupt still halt execution.
///
/// ANSI is emitted directly via raw CSI sequences rather than going
/// through the `colored` crate's global-override machinery — the unit
/// test needs deterministic bytes in a `Vec<u8>` buffer regardless of
/// `colored::control::set_override` races between parallel tests.
/// `\x1b[1;31m` opens bold+red, `\x1b[0m` resets.
pub fn maybe_print_yolo_banner<W: Write>(enabled: bool, out: &mut W) -> std::io::Result<()> {
    if !enabled {
        return Ok(());
    }
    // CSI 1 = bold, CSI 31 = red; combine as "1;31" in one SGR escape.
    // This is portable across xterm-class terminals and matches what
    // colored::Colorize would emit for `.bold().red()` on a single span.
    writeln!(
        out,
        "\x1b[1;31m--yolo enabled:\x1b[0m \x1b[31mall dangerous-command \
         approvals are bypassed. Iteration budget, fatal error, and user \
         interrupt are the only stops.\x1b[0m"
    )
}

/// Convenience wrapper: same semantics as `maybe_print_yolo_banner`
/// but always writes to stderr. Use ONCE per session at run-start.
pub fn print_yolo_banner_to_stderr(enabled: bool) {
    // Ignore write errors — stderr failing is not a hard-stop.
    let _ = maybe_print_yolo_banner(enabled, &mut std::io::stderr().lock());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_yolo_disabled_by_default() {
        assert_eq!(resolve_yolo(false, false), (false, "disabled"));
    }

    #[test]
    fn resolve_yolo_flag_wins_over_config() {
        assert_eq!(resolve_yolo(true, true), (true, "flag"));
    }

    #[test]
    fn resolve_yolo_config_enables_when_flag_is_false() {
        assert_eq!(resolve_yolo(false, true), (true, "config"));
    }
}
