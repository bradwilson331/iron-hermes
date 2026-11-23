//! D-20 replay: the sanitized `clawdi-fork-parity-daily` cron prompt through the
//! REAL `ironhermes-tirith` sidecar's paste op (Phase 57 plan 06).
//!
//! Calibration record. The cron scanner (`ironhermes_cron::scan_cron_prompt_detailed`)
//! passes this excerpt -- the real job prompt text matches no `CRON_THREAT_PATTERNS`
//! entry. The sidecar's paste op, however, reports one HIGH finding on it:
//! `AnalysisIncomplete` ("Nested executable body could not be parsed completely").
//! It is produced by quote characters in benign prose (a `"DEGRADED — web-only mode"`
//! phrase together with a `"Fork Parity — <today's date>"` title), not by anything
//! executable. Any combined cron verdict must therefore NOT treat a lone paste-op
//! `AnalysisIncomplete` as a block: it would refuse a legitimate scheduled job.
//!
//! The asserted rule ids are exactly what the pinned tirith-core 0.4.2 returns.
//! If a tirith-core bump changes them, re-record the result deliberately.

use std::path::PathBuf;

use ironhermes_security::{SidecarClient, SidecarOptions};
use ironhermes_security_proto::{SubjectKind, WireSeverity};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ironhermes-cron/tests/fixtures/clawdi_prompt_sanitized.txt"
);

#[tokio::test]
async fn clawdi_sidecar_replay() {
    let text = std::fs::read_to_string(FIXTURE).expect("sanitized clawdi fixture must exist");
    let home = tempfile::tempdir().unwrap();
    let mut opts = SidecarOptions::for_home(home.path());
    opts.binary = Some(PathBuf::from(env!("CARGO_BIN_EXE_ironhermes-tirith")));
    let client = SidecarClient::spawn(opts);

    let findings = client
        .check(SubjectKind::Paste, &text, None)
        .await
        .expect("sidecar must answer the paste check");

    let got: Vec<(&str, WireSeverity)> =
        findings.iter().map(|f| (f.rule.as_str(), f.sev)).collect();
    assert_eq!(
        got,
        vec![("AnalysisIncomplete", WireSeverity::High)],
        "recorded paste-op result for the sanitized clawdi prompt, got {findings:?}"
    );
}
