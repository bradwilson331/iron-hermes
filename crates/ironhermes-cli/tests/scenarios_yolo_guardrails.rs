//! S-05 / S-06 / S-07 / S-08 / S-17 / S-18 — yolo + guardrails scenarios
//! (AI-SPEC §5).
//!
//! Layering:
//!   S-05 — budget exhaustion latches (BudgetHandle atomic-level property)
//!   S-06 — gateway + main.rs never read `request.yolo` (static-grep D-12)
//!   S-07 — --yolo path must not `catch_unwind` (G-09 halt surface)
//!   S-08 — no `stdin().read` in agent/delegate_task source (G-08)
//!   S-17 — marker: integration proof lives in Plan 05 E-12 test
//!   S-18 — insta snapshots of StatusReport (default, --all, --deep, yolo)

use ironhermes_cli::status_cmd::StatusReport;

// ---------------------------------------------------------------------------
// S-05: budget latches at 0 — any subsequent consume must return None.
// This is the atomic-level property that prevents an extra tool_use from
// leaking past Stop100. The provider-streamed "extra tool_use after budget"
// end-to-end proof lives in Plan 05's E-12 advisory injection test.
// ---------------------------------------------------------------------------
#[test]
fn s05_budget_latches_at_zero_no_extra_consume_succeeds() {
    use ironhermes_agent::budget::BudgetHandle;

    let b = BudgetHandle::new(3);
    // consume returns Some(remaining_after_decrement): 2, 1, 0.
    assert_eq!(b.consume(), Some(2), "consume #1");
    assert_eq!(b.consume(), Some(1), "consume #2");
    assert_eq!(b.consume(), Some(0), "consume #3 hits floor");
    // Further consume MUST return None (Stop100 latched).
    assert_eq!(
        b.consume(),
        None,
        "S-05 / G-01: budget must return None once exhausted"
    );
    for i in 0..10 {
        assert_eq!(
            b.consume(),
            None,
            "S-05 / G-01: consume #{} after exhaustion must remain None \
             (extra tool_use streamed by provider must never execute)",
            i + 4
        );
    }
    assert_eq!(
        b.remaining(),
        0,
        "remaining must stay at 0 after None-latch"
    );
}

// ---------------------------------------------------------------------------
// S-06: gateway + main.rs must NOT read a per-request yolo field. Mirrors
// the runtime `INV-21.7-05` invariant at the scenario-test layer so eval
// auditors can grep by `S-06` and find a concrete assertion.
// ---------------------------------------------------------------------------
#[test]
fn s06_gateway_and_cli_do_not_read_per_request_yolo() {
    const CLI_MAIN: &str = include_str!("../src/main.rs");
    const GW_HANDLER: &str = include_str!("../../ironhermes-gateway/src/handler.rs");

    for (label, src) in [("main.rs", CLI_MAIN), ("handler.rs", GW_HANDLER)] {
        assert!(
            !src.contains("request.yolo"),
            "S-06 / D-12: {} must NOT read `request.yolo`",
            label
        );
        assert!(
            !src.contains("req.yolo"),
            "S-06 / D-12: {} must NOT read `req.yolo`",
            label
        );
    }
}

// ---------------------------------------------------------------------------
// S-07: the --yolo path must NOT suppress fatal panics by wrapping in
// `catch_unwind`. G-09 is the unskippable halt surface — yolo bypasses
// approvals, never crashes. If main.rs co-locates `catch_unwind` and `yolo`
// in the same file it could indicate an anti-pattern; this is a heuristic
// static-grep (the runtime proof lives in the agent loop).
// ---------------------------------------------------------------------------
#[test]
fn s07_yolo_path_does_not_catch_unwind_panics() {
    const CLI_MAIN: &str = include_str!("../src/main.rs");
    let has_catch_unwind = CLI_MAIN.contains("catch_unwind");
    let has_yolo = CLI_MAIN.contains("yolo");
    assert!(
        !(has_catch_unwind && has_yolo),
        "S-07 / G-09: main.rs must not co-locate `catch_unwind` with `yolo` \
         — fatal-error halt is unskippable under --yolo"
    );
}

// ---------------------------------------------------------------------------
// S-08: no `stdin().read` appears in agent or delegate_task source trees.
// G-08 requires non-interactive paths to short-circuit via `io_gate::
// can_prompt(...)` rather than blocking on stdin.
// ---------------------------------------------------------------------------
#[test]
fn s08_no_stdin_read_in_agent_or_delegate_task_sources() {
    use walkdir::WalkDir;

    // Walk from the workspace root (CARGO_MANIFEST_DIR is .../crates/
    // ironhermes-cli; go up two levels).
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");

    let agent_src = workspace.join("crates/ironhermes-agent/src");
    let delegate_task = workspace.join("crates/ironhermes-tools/src/delegate_task.rs");

    // Agent src tree.
    for entry in WalkDir::new(&agent_src).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(entry.path())
            .unwrap_or_else(|e| panic!("read {:?}: {}", entry.path(), e));
        assert!(
            !src.contains("stdin().read"),
            "S-08 / G-08: stdin().read MUST NOT appear in \
             crates/ironhermes-agent/*. Found in {:?}",
            entry.path()
        );
    }

    // delegate_task.rs — child subagent entry must never read stdin.
    let src = std::fs::read_to_string(&delegate_task)
        .unwrap_or_else(|e| panic!("read {:?}: {}", delegate_task, e));
    assert!(
        !src.contains("stdin().read"),
        "S-08 / G-08: delegate_task.rs must never read stdin"
    );
}

// ---------------------------------------------------------------------------
// S-17 — marker. Integration proof lives in Plan 05's
// `budget_pressure_advisory_injection.rs` (recording-provider test that
// drives real tier crossings and asserts advisory injection).
// ---------------------------------------------------------------------------
#[test]
fn s17_is_covered_by_plan_05_e12_budget_pressure_advisory_injection() {
    // Sentinel: if Plan 05's test is deleted or weakened, this marker
    // remains and the phase SUMMARY must be updated.
    let _ = "S-17 covered by crates/ironhermes-agent/tests/budget_pressure_advisory_injection.rs";
}

// ---------------------------------------------------------------------------
// S-18 — four insta snapshots of StatusReport output.
// Locks the v1 JSON schema across default / --all / --deep / yolo-on.
// ---------------------------------------------------------------------------
#[test]
fn s18_status_report_snapshot_default() {
    let snap = StatusReport::fixture();
    let json = serde_json::to_string_pretty(&snap).expect("serialize");
    insta::assert_snapshot!("s18_default_json", json);
}

#[test]
fn s18_status_report_snapshot_with_all() {
    use ironhermes_cli::status_cmd::McpServer;

    let mut snap = StatusReport::fixture();
    // Populate per_server to reflect --all.
    snap.mcp.per_server = Some(vec![
        McpServer {
            name: "fs".into(),
            connected: true,
            tool_count: 4,
            reachable: None,
        },
        McpServer {
            name: "github".into(),
            connected: true,
            tool_count: 11,
            reachable: None,
        },
    ]);
    let json = serde_json::to_string_pretty(&snap).expect("serialize");
    insta::assert_snapshot!("s18_all_json", json);
}

#[test]
fn s18_status_report_snapshot_with_deep_healthy() {
    let mut snap = StatusReport::fixture();
    snap.provider.healthy = Some(true);
    snap.memory.state_db_healthy = Some(true);
    let json = serde_json::to_string_pretty(&snap).expect("serialize");
    insta::assert_snapshot!("s18_deep_healthy_json", json);
}

#[test]
fn s18_status_report_snapshot_yolo_on() {
    use ironhermes_cli::status_cmd::YoloStatus;

    let mut snap = StatusReport::fixture();
    snap.yolo = YoloStatus {
        enabled: true,
        source: "flag".into(),
    };
    let json = serde_json::to_string_pretty(&snap).expect("serialize");
    insta::assert_snapshot!("s18_yolo_on_json", json);
}
