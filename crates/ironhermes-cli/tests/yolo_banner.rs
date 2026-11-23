//! E-02 / E-10 — yolo banner assertions via the public lib surface.
//!
//! ISS-07: drop the assert_cmd stub; unit-test `maybe_print_yolo_banner`
//! directly. The behavioral E-02 UAT row in VALIDATION.md remains
//! operator-manual (no mock provider exists yet).

use ironhermes_cli::{maybe_print_yolo_banner, resolve_session_default};
use ironhermes_core::config::DefconLevel;

fn lvl(n: u8) -> DefconLevel {
    DefconLevel::try_from(n).expect("1-5")
}

/// Phase 57 D-08: through the lib re-export, `--yolo` wins over a config level, a
/// config level stays as configured without the flag, and a worker env level of 1
/// tightens even a `--yolo` process to 1.
#[test]
fn resolve_session_default_precedence_matrix() {
    assert_eq!(
        resolve_session_default(lvl(2), true, None).0,
        lvl(5),
        "--yolo wins over a config level"
    );
    assert_eq!(
        resolve_session_default(lvl(2), false, None).0,
        lvl(2),
        "config level 2 stays 2 without the flag"
    );
    assert_eq!(
        resolve_session_default(lvl(3), false, None).0,
        lvl(3),
        "nothing set -> the config default"
    );
    assert_eq!(
        resolve_session_default(lvl(3), true, Some("1")).0,
        lvl(1),
        "an env level of 1 tightens a --yolo process to 1"
    );
}

#[test]
fn maybe_print_yolo_banner_writes_nothing_when_disabled() {
    // Force colored off for deterministic bytes.
    colored::control::set_override(false);
    let mut buf: Vec<u8> = Vec::new();
    maybe_print_yolo_banner(false, &mut buf).expect("write");
    assert!(
        buf.is_empty(),
        "disabled -> no bytes written. Got: {:?}",
        String::from_utf8_lossy(&buf)
    );
}

#[test]
fn maybe_print_yolo_banner_writes_bold_red_banner_when_enabled() {
    // Colored on to mimic production bytes.
    colored::control::set_override(true);
    let mut buf: Vec<u8> = Vec::new();
    maybe_print_yolo_banner(true, &mut buf).expect("write");
    let out = String::from_utf8_lossy(&buf);

    // Core string markers (insensitive to ANSI re-layout):
    assert!(
        out.contains("--yolo enabled:"),
        "banner must contain '--yolo enabled:'. Got: {:?}",
        out
    );
    assert!(
        out.contains("dangerous-command approvals are bypassed"),
        "banner must mention approval bypass. Got: {:?}",
        out
    );
    assert!(
        out.contains("Iteration budget"),
        "banner must mention iteration-budget unskippable stop. Got: {:?}",
        out
    );
    // Bold + red ANSI sequences: CSI 1 (bold) and 31 (red) both present.
    assert!(
        out.contains("\x1b[1"),
        "expected bold ANSI escape. Got: {:?}",
        out
    );
    assert!(
        out.contains("\x1b[31") || out.contains(";31"),
        "expected red ANSI escape. Got: {:?}",
        out
    );
}

#[test]
fn maybe_print_yolo_banner_idempotent_when_called_twice_in_isolation() {
    // Important detail: callers are expected to invoke this ONCE per session.
    // This test confirms the function itself is idempotent on the writer.
    colored::control::set_override(false);
    let mut buf: Vec<u8> = Vec::new();
    maybe_print_yolo_banner(true, &mut buf).unwrap();
    let first_len = buf.len();
    maybe_print_yolo_banner(true, &mut buf).unwrap();
    // Two sequential writes doubles the content; the per-session one-shot
    // is a SITE discipline (enforced by run_chat / run_single / run_gateway
    // calling exactly once), not a function-internal latch.
    assert_eq!(
        buf.len(),
        first_len * 2,
        "maybe_print_yolo_banner does not internally de-duplicate; \
         single-call discipline is caller-side"
    );
}
