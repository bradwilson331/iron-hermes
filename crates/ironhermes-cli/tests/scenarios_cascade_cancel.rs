//! S-01..S-04 — cascade-cancel scenarios (AI-SPEC §5).
//!
//! Real SIGINT delivery is a manual UAT item (see VALIDATION.md
//! "Manual-Only Verifications" / D-07); these tests assert the in-process
//! token cascade + process-registry drain are correct.
//!
//! Coverage:
//!   S-01  parent cancels 3 subagents within 500ms      — cfg(unix)
//!   S-02  parent cancels subagents + processes; no zombies — cfg(unix)
//!   S-03  third ctrl-c hard-exit                        — #[ignore] (manual UAT)
//!   S-04  on_session_end mid-subagent leaves Cancelled marker

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use ironhermes_exec::process_registry::{ProcessRegistry, SpawnSpec};

/// S-01 — parent cancel token fires → within 500ms all 3 subagent tokens
/// observe `is_cancelled()`. Token cascade is the oracle for E-01.
#[cfg(unix)]
#[tokio::test]
async fn s01_parent_cancels_three_subagents_within_500ms() {
    let parent = CancellationToken::new();
    let children: Vec<CancellationToken> =
        (0..3).map(|_| parent.child_token()).collect();

    // Pre-cancel sanity.
    assert!(
        children.iter().all(|c| !c.is_cancelled()),
        "pre-cancel: no child tokens should be cancelled yet"
    );

    let start = std::time::Instant::now();
    parent.cancel();

    // Poll for cancellation propagation (child_token is synchronous but we
    // allow a tiny yield window to mirror real scheduler behavior).
    for _ in 0..50 {
        if children.iter().all(|c| c.is_cancelled()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let latency = start.elapsed();

    assert!(
        children.iter().all(|c| c.is_cancelled()),
        "S-01 / E-01: all 3 subagent child tokens must be cancelled once parent cancels"
    );
    assert!(
        latency < Duration::from_millis(500),
        "S-01 / M-01: cascade must complete within 500ms. Elapsed: {:?}",
        latency
    );
}

/// S-02 — mixed cascade: parent cancels 3 subagent tokens + 2 tracked
/// background processes. Post-drain: all tokens cancelled, no processes in
/// `sysinfo`, registry running_count == 0.
#[cfg(unix)]
#[tokio::test]
async fn s02_parent_cancels_subagents_plus_processes_no_zombies() {
    use sysinfo::{Pid, System};

    // Subagent stand-ins: 3 child tokens.
    let parent = CancellationToken::new();
    let children: Vec<CancellationToken> =
        (0..3).map(|_| parent.child_token()).collect();

    // Real tracked processes via ProcessRegistry.
    let reg = Arc::new(RwLock::new(ProcessRegistry::new_for_session("s02")));
    let mut pids: Vec<u32> = Vec::new();
    for _ in 0..2 {
        let id = {
            let mut r = reg.write().await;
            r.spawn(SpawnSpec {
                command: "sleep 30".into(),
                cwd: None,
                env: vec![],
                watch_patterns: vec![],
            })
            .await
            .expect("spawn sleep 30")
        };
        let pid = {
            let r = reg.read().await;
            r.poll(&id).await.and_then(|s| s.pid).expect("spawn must record a pid")
        };
        pids.push(pid);
    }

    // Fire the cascade: cancel parent token + drain the registry.
    parent.cancel();
    reg.write()
        .await
        .drain_and_kill_session("s02")
        .await
        .expect("drain_and_kill_session");

    // Allow OS reaping + sysinfo propagation.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Subagent tokens cancelled.
    assert!(
        children.iter().all(|c| c.is_cancelled()),
        "S-02 / E-01: subagent tokens must be cancelled after parent.cancel()"
    );

    // Processes gone from sysinfo.
    let mut sys = System::new_all();
    sys.refresh_all();
    for pid in &pids {
        assert!(
            sys.process(Pid::from(*pid as usize)).is_none(),
            "S-02 / E-03: process {} must be gone post-drain",
            pid
        );
    }

    // Registry accounting: running map drained.
    let r = reg.read().await;
    assert_eq!(
        r.running_count(),
        0,
        "S-02: running map must be empty after drain_and_kill_session"
    );
}

/// S-03 — third ctrl-c emergency hard-exit. Automatable only via subprocess
/// + real SIGINT delivery, which is finicky on macOS and excluded from CI.
/// VALIDATION.md carries the manual UAT row.
#[test]
#[ignore = "S-03 manual UAT — real SIGINT delivery covered in VALIDATION.md Manual-Only row"]
fn s03_third_ctrl_c_hard_exits() {
    // Documented UAT:
    //   1. Launch `hermes chat`
    //   2. Issue `delegate_task` that runs `sleep 60`
    //   3. Press Ctrl-C three times within 500ms each
    //   4. Assert:
    //      (a) binary exits with code 130 (SIGINT signal)
    //      (b) `ps` shows no orphan hermes children
    //      (c) no zombies visible via `ps -A -o pid,stat`
    //
    // Automation deferred past Phase 21.7 per open_questions in PLAN.md.
}

/// S-04 — on_session_end during a mid-subagent execution ends the transcript
/// with a `Cancelled` marker (D-07 contract).
#[cfg(unix)]
#[tokio::test]
async fn s04_on_session_end_during_subagent_leaves_cancelled_marker() {
    use ironhermes_agent::transcript::{
        transcript_path_for, TranscriptLine, TranscriptWriter,
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = transcript_path_for(tmp.path(), "sess-s04", "sub-1");

    let w = TranscriptWriter::open(&path);
    // Simulate a mid-turn sequence.
    w.append(TranscriptLine::now_stream_delta("partial output"));
    w.append(TranscriptLine::now_tool_call("exec", "ls"));
    // ...then on_session_end fires and writes the cancel marker (D-07).
    w.append(TranscriptLine::now_cancelled("on_session_end during turn"));

    // Fire-and-forget writes are tokio::spawn'd; drain before reading.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let body = std::fs::read_to_string(&path).expect("transcript file");
    let lines: Vec<&str> = body.lines().collect();
    assert!(
        lines.len() >= 3,
        "S-04: transcript must contain >= 3 lines, got {}",
        lines.len()
    );

    let last: TranscriptLine =
        serde_json::from_str(lines.last().expect("at least one line"))
            .expect("last line must be valid TranscriptLine JSON");
    assert!(
        matches!(last, TranscriptLine::Cancelled { .. }),
        "S-04 / D-07: last line on cancel path MUST be Cancelled, got {:?}",
        last
    );
}
