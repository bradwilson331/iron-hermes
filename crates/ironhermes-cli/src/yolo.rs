//! Process-default DEFCON level resolution + --yolo banner emission
//! (Phase 57 D-08 / research OQ7; originally Phase 21.7 D-11/D-12/D-14).
//!
//! Public API re-exported by `crate::lib` so tests and main binaries can
//! both drive the same code path via `ironhermes_cli::{resolve_session_default,
//! maybe_print_yolo_banner, print_yolo_banner_to_stderr}`.
//!
//! Design rules:
//! - [`resolve_session_default`] is pure: it picks the level that seeds every
//!   NEW session in this process (`main.rs` hands the result to
//!   `session_defcon().set_process_default`). The `--yolo` flag is level 5; a
//!   kanban worker's `IRONHERMES_DEFCON_LEVEL` env can only TIGHTEN that or the
//!   profile's own level (the numeric minimum), never loosen it.
//! - Gateway NEVER reads a per-request yolo flag (INV-21.7-05); the process
//!   default is its only input, and `main.rs` passes no CLI flag for it.
//! - `maybe_print_yolo_banner` takes a generic `Write` so it can be
//!   unit-tested against `Vec<u8>` (ISS-07 — replaces assert_cmd stub).
//! - Banner is bold red; the text includes the three unskippable stops
//!   (iteration budget, fatal error, user interrupt — G-01/G-04/G-09).

use std::io::Write;

use ironhermes_core::config::DefconLevel;

/// The environment variable a kanban dispatcher sets on a worker (research OQ7): the
/// dispatcher's session level, which the worker may only match or tighten.
pub const WORKER_LEVEL_ENV: &str = "IRONHERMES_DEFCON_LEVEL";

/// The level that seeds every new session in this process (Phase 57 D-08, OQ7).
///
/// - the base is `config_level` (the profile's effective level), or level 5 when
///   `cli_yolo_flag` is set (`--yolo`, top-level or `chat --yolo`, D-08);
/// - a valid `worker_env` (`1`-`5`, surrounding whitespace tolerated) can only make the
///   result STRICTER: the result is the numeric minimum of the base and the env level,
///   so a worker can never be looser than its profile or its dispatcher (T-57-55);
/// - an empty value is "unset"; any other value is ignored and a warning is returned.
///
/// Pure: no env access, no I/O. The caller prints the warning.
pub fn resolve_session_default(
    config_level: DefconLevel,
    cli_yolo_flag: bool,
    worker_env: Option<&str>,
) -> (DefconLevel, Option<String>) {
    let base = if cli_yolo_flag {
        DefconLevel::FIVE
    } else {
        config_level
    };
    let Some(raw) = worker_env.map(str::trim).filter(|v| !v.is_empty()) else {
        return (base, None);
    };
    match raw
        .parse::<u8>()
        .ok()
        .and_then(|n| DefconLevel::try_from(n).ok())
    {
        Some(env_level) => (base.min(env_level), None),
        None => (
            base,
            Some(format!(
                "{WORKER_LEVEL_ENV}={raw} ignored: it must be a whole number from 1 to 5; \
                 using DEFCON {}",
                base.level()
            )),
        ),
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

    fn lvl(n: u8) -> DefconLevel {
        DefconLevel::try_from(n).expect("1-5")
    }

    /// D-08 / OQ7: no flag and no env -> the config level; `--yolo` -> 5; a valid env
    /// level only ever tightens; an invalid env value is ignored with a warning.
    #[test]
    fn resolve_session_default_rules() {
        assert_eq!(resolve_session_default(lvl(3), false, None), (lvl(3), None));
        assert_eq!(resolve_session_default(lvl(2), false, None), (lvl(2), None));
        assert_eq!(
            resolve_session_default(lvl(1), true, None),
            (lvl(5), None),
            "--yolo is level 5 whatever the config says (D-08)"
        );
        assert_eq!(
            resolve_session_default(lvl(3), false, Some("1")),
            (lvl(1), None),
            "env 1 with config 3 tightens to 1"
        );
        assert_eq!(
            resolve_session_default(lvl(2), false, Some("5")),
            (lvl(2), None),
            "env 5 with config 2 stays 2: the env only tightens"
        );
        assert_eq!(
            resolve_session_default(lvl(2), true, Some("1")),
            (lvl(1), None),
            "an env level tightens a --yolo process too"
        );
        assert_eq!(
            resolve_session_default(lvl(3), false, Some(" 2 ")),
            (lvl(2), None),
            "surrounding whitespace is tolerated"
        );
        assert_eq!(
            resolve_session_default(lvl(3), false, Some("")),
            (lvl(3), None),
            "an empty value is 'unset', not an error"
        );
        for bad in ["9", "0", "x", "-1", "3.5", "high"] {
            let (level, warning) = resolve_session_default(lvl(3), false, Some(bad));
            assert_eq!(level, lvl(3), "invalid {bad:?} is ignored");
            let warning = warning.unwrap_or_else(|| panic!("invalid {bad:?} must warn"));
            assert!(
                warning.contains("IRONHERMES_DEFCON_LEVEL") && warning.contains(bad),
                "the warning names the variable and the value: {warning}"
            );
        }
    }

    /// OQ7: for every config level, flag and env level, the env result is exactly
    /// `min(without-env result, env level)`, so an env-provided level can never produce
    /// a level numerically higher than the non-env result.
    #[test]
    fn worker_env_level_only_tightens() {
        for config in 1..=5u8 {
            for flag in [false, true] {
                let (baseline, _) = resolve_session_default(lvl(config), flag, None);
                for env in 1..=5u8 {
                    let (with_env, warning) =
                        resolve_session_default(lvl(config), flag, Some(&env.to_string()));
                    assert!(warning.is_none(), "valid env {env} must not warn");
                    assert!(
                        with_env <= baseline,
                        "env {env} loosened config {config} flag {flag}: {with_env:?} > {baseline:?}"
                    );
                    assert_eq!(
                        with_env,
                        baseline.min(lvl(env)),
                        "env {env}, config {config}, flag {flag}: the stricter of the two"
                    );
                }
            }
        }
    }
}
