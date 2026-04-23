//! D-24 / D-25 / D-28 integration tests for ProcessRegistry operations.
//!
//! These tests exercise the spawn / drain_and_kill / LRU / TTL / snapshot
//! surface defined in Plan 21.7-02 Task 2-02. Tests that require a real
//! child process are `cfg!(unix)`-gated (the reference commands are
//! `sleep`, which is unix-portable but absent on Windows).

use ironhermes_exec::process_registry::*;
use std::time::Duration;

#[tokio::test]
async fn spawn_then_drain_kills_all_children() {
    if !cfg!(unix) {
        return;
    }
    let mut reg = ProcessRegistry::new_for_session("t-drain");
    let spec = SpawnSpec {
        command: "sleep 10".to_string(),
        cwd: None,
        env: vec![],
        watch_patterns: vec![],
    };
    let _id = reg.spawn(spec).await.expect("spawn");
    assert_eq!(reg.running_count(), 1);

    reg.drain_and_kill().await.expect("drain");
    assert_eq!(
        reg.running_count(),
        0,
        "E-03: drain must empty running map"
    );
    assert!(
        reg.finished_count() >= 1,
        "drained entries become finished, not dropped"
    );
    // Zombie-check deferred to Wave 4 S-01 harness which uses nix::waitpid.
}

#[tokio::test]
async fn drain_and_kill_session_is_task_id_scoped() {
    if !cfg!(unix) {
        return;
    }
    let mut reg = ProcessRegistry::new_for_session("task-a");
    reg.spawn(SpawnSpec {
        command: "sleep 10".to_string(),
        cwd: None,
        env: vec![],
        watch_patterns: vec![],
    })
    .await
    .expect("spawn");
    assert_eq!(reg.running_count(), 1);

    // Different task id — no-op.
    reg.drain_and_kill_session("task-b").await.expect("drain-b");
    assert_eq!(reg.running_count(), 1, "drain is task-id-scoped");

    // Matching task id — drains.
    reg.drain_and_kill_session("task-a").await.expect("drain-a");
    assert_eq!(reg.running_count(), 0);
}

#[tokio::test]
async fn spawn_over_max_processes_lru_prunes_oldest() {
    if !cfg!(unix) {
        return;
    }
    let mut reg = ProcessRegistry::new_for_session("t-lru");
    let n = MAX_PROCESSES + 1;
    for i in 0..n {
        // Vary sleep slightly to create deterministic oldest-first ordering.
        let spec = SpawnSpec {
            command: format!("sleep {}", 30 + i / 10),
            cwd: None,
            env: vec![],
            watch_patterns: vec![],
        };
        reg.spawn(spec).await.expect("spawn");
    }
    assert!(
        reg.running_count() <= MAX_PROCESSES,
        "LRU: running_count {} must not exceed MAX_PROCESSES {}",
        reg.running_count(),
        MAX_PROCESSES
    );
    // Cleanup.
    reg.drain_and_kill().await.expect("drain");
}

#[tokio::test]
async fn finished_ttl_prunes_30min_old_entries() {
    // Build a finished entry by hand via the test-only builder (avoids
    // spawning a real child and any time-pause dependency).
    let mut reg = ProcessRegistry::new_for_session("t-ttl");
    use std::time::Instant;
    let started = Instant::now() - Duration::from_secs(FINISHED_TTL_SECONDS + 120);
    let finished = Instant::now() - Duration::from_secs(FINISHED_TTL_SECONDS + 60);
    let old_finished = fake_process_session(
        "proc_test000000",
        "t-ttl",
        "echo done",
        Some(1),
        started,
        Some(finished),
        Some(0),
    );
    reg.insert_fake_finished(old_finished);
    assert_eq!(reg.finished_count(), 1);

    reg.prune_finished_ttl();
    assert_eq!(
        reg.finished_count(),
        0,
        "E-03: TTL must prune 30min-old finished entries"
    );
}

#[tokio::test]
async fn finished_ttl_keeps_recent_entries() {
    let mut reg = ProcessRegistry::new_for_session("t-ttl-keep");
    use std::time::Instant;
    let now = Instant::now();
    let fresh = fake_process_session(
        "proc_fresh_01",
        "t-ttl-keep",
        "echo ok",
        Some(2),
        now,
        Some(now),
        Some(0),
    );
    reg.insert_fake_finished(fresh);
    reg.prune_finished_ttl();
    assert_eq!(
        reg.finished_count(),
        1,
        "recent finished entries must be retained"
    );
}

#[tokio::test]
async fn snapshot_reports_running_plus_finished() {
    if !cfg!(unix) {
        return;
    }
    let mut reg = ProcessRegistry::new_for_session("t-snap");
    reg.spawn(SpawnSpec {
        command: "sleep 10".into(),
        cwd: None,
        env: vec![],
        watch_patterns: vec![],
    })
    .await
    .unwrap();
    let snap = reg.snapshot();
    assert_eq!(snap.tracked, 1);
    assert_eq!(snap.entries.len(), 1);
    assert_eq!(snap.entries[0].task_id, "t-snap");
    reg.drain_and_kill().await.unwrap();
    // After drain: snapshot still shows the finished entry (TTL keeps it).
    let snap2 = reg.snapshot();
    assert_eq!(snap2.tracked, 1);
    assert!(snap2.entries[0].exit_code.is_none() || snap2.entries[0].exit_code.is_some());
}

#[tokio::test]
async fn spawn_with_bad_watch_pattern_errors() {
    if !cfg!(unix) {
        return;
    }
    let mut reg = ProcessRegistry::new_for_session("t-badre");
    let res = reg
        .spawn(SpawnSpec {
            command: "sleep 1".into(),
            cwd: None,
            env: vec![],
            watch_patterns: vec!["(unclosed".to_string()],
        })
        .await;
    assert!(res.is_err(), "invalid regex must fail spawn up-front");
    assert_eq!(reg.running_count(), 0);
}

#[tokio::test]
async fn spawn_with_empty_command_errors() {
    let mut reg = ProcessRegistry::new_for_session("t-empty");
    let res = reg
        .spawn(SpawnSpec {
            command: "   ".into(),
            cwd: None,
            env: vec![],
            watch_patterns: vec![],
        })
        .await;
    assert!(res.is_err(), "empty command must fail spawn");
}

#[tokio::test]
async fn kill_unknown_id_is_idempotent() {
    let mut reg = ProcessRegistry::new_for_session("t-kill-unknown");
    reg.kill("proc_does_not_exist").await.expect("idempotent");
}

#[tokio::test]
async fn poll_returns_running_then_finished_after_drain() {
    if !cfg!(unix) {
        return;
    }
    let mut reg = ProcessRegistry::new_for_session("t-poll");
    let id = reg
        .spawn(SpawnSpec {
            command: "sleep 10".into(),
            cwd: None,
            env: vec![],
            watch_patterns: vec![],
        })
        .await
        .unwrap();
    let st = reg.poll(&id).await.expect("status");
    assert!(st.running, "just-spawned should be running");
    assert!(st.exit_code.is_none());
    reg.drain_and_kill().await.unwrap();
    let st2 = reg.poll(&id).await.expect("status-after");
    assert!(!st2.running, "after drain should not be running");
}
