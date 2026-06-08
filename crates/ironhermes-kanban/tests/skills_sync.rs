//! Integration tests for skills_bundle sync helpers (Plan 07, Task 2).
//!
//! Tests verify:
//! - First-run creates both SKILL.md files with bundled content
//! - Second-run with force=false preserves user edits (D-30)
//! - force=true overwrites user edits
//! - restore_bundled_kanban_skill() writes bundled content for a single skill
//! - restore_bundled_kanban_skill() errors on unknown skill names
//! - Parent directories are created automatically

use ironhermes_kanban::{
    KANBAN_ORCHESTRATOR_SKILL_MD, KANBAN_WORKER_SKILL_MD, KanbanError, SyncAction,
    restore_bundled_kanban_skill, sync_bundled_kanban_skills,
};
use tempfile::TempDir;

fn temp_skills_root() -> TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

#[test]
fn first_run_writes_both_skills() {
    let tmp = temp_skills_root();
    let root = tmp.path();

    let report =
        sync_bundled_kanban_skills(root, false).expect("sync_bundled_kanban_skills should succeed");

    let worker_path = root.join("kanban-worker").join("SKILL.md");
    let orchestrator_path = root.join("kanban-orchestrator").join("SKILL.md");

    assert!(
        worker_path.exists(),
        "kanban-worker/SKILL.md should exist after first sync"
    );
    assert!(
        orchestrator_path.exists(),
        "kanban-orchestrator/SKILL.md should exist after first sync"
    );

    let worker_content = std::fs::read_to_string(&worker_path).expect("read worker SKILL.md");
    let orchestrator_content =
        std::fs::read_to_string(&orchestrator_path).expect("read orchestrator SKILL.md");

    assert!(
        !worker_content.is_empty(),
        "kanban-worker/SKILL.md should be non-empty"
    );
    assert!(
        !orchestrator_content.is_empty(),
        "kanban-orchestrator/SKILL.md should be non-empty"
    );

    assert_eq!(report.worker_action, SyncAction::Wrote);
    assert_eq!(report.orchestrator_action, SyncAction::Wrote);
}

#[test]
fn second_run_preserves_user_edits() {
    let tmp = temp_skills_root();
    let root = tmp.path();

    // First sync — creates the files.
    sync_bundled_kanban_skills(root, false).expect("first sync should succeed");

    // User edits the worker skill.
    let worker_path = root.join("kanban-worker").join("SKILL.md");
    std::fs::write(&worker_path, "USER EDIT").expect("write user edit");

    // Second sync with force=false — should preserve the user edit.
    let report = sync_bundled_kanban_skills(root, false).expect("second sync should succeed");

    let content = std::fs::read_to_string(&worker_path).expect("read after second sync");
    assert_eq!(
        content, "USER EDIT",
        "user edit must be preserved when force=false"
    );
    assert_eq!(
        report.worker_action,
        SyncAction::Skipped,
        "worker action must be Skipped when file exists and force=false"
    );
}

#[test]
fn force_overwrites_user_edits() {
    let tmp = temp_skills_root();
    let root = tmp.path();

    // First sync — creates the files.
    sync_bundled_kanban_skills(root, false).expect("first sync");

    // User edits the worker skill.
    let worker_path = root.join("kanban-worker").join("SKILL.md");
    std::fs::write(&worker_path, "USER EDIT").expect("write user edit");

    // Force sync — must overwrite.
    let report = sync_bundled_kanban_skills(root, true).expect("force sync should succeed");

    let content = std::fs::read_to_string(&worker_path).expect("read after force sync");
    assert_eq!(
        content, KANBAN_WORKER_SKILL_MD,
        "force sync must restore bundled content"
    );
    assert_ne!(
        content, "USER EDIT",
        "user edit must be overwritten when force=true"
    );
    assert_eq!(
        report.worker_action,
        SyncAction::Restored,
        "worker action must be Restored when force=true and file existed"
    );
}

#[test]
fn restore_helper_writes_bundled() {
    let tmp = temp_skills_root();
    let root = tmp.path();

    restore_bundled_kanban_skill(root, "kanban-worker")
        .expect("restore_bundled_kanban_skill should succeed for kanban-worker");

    let worker_path = root.join("kanban-worker").join("SKILL.md");
    assert!(
        worker_path.exists(),
        "kanban-worker/SKILL.md should exist after restore"
    );

    let content = std::fs::read_to_string(&worker_path).expect("read restored SKILL.md");
    assert_eq!(
        content, KANBAN_WORKER_SKILL_MD,
        "restored content must match bundled bytes exactly"
    );
}

#[test]
fn restore_unknown_skill_errors() {
    let tmp = temp_skills_root();
    let root = tmp.path();

    let result = restore_bundled_kanban_skill(root, "unknown-skill");
    assert!(
        result.is_err(),
        "restore_bundled_kanban_skill should error for unknown skill"
    );

    match result.unwrap_err() {
        KanbanError::UnknownSkill(name) => {
            assert_eq!(name, "unknown-skill");
        }
        other => panic!("expected KanbanError::UnknownSkill, got: {:?}", other),
    }
}

#[test]
fn sync_creates_parent_dirs() {
    let tmp = temp_skills_root();
    // Pass the root directly — kanban-worker/ and kanban-orchestrator/ don't exist yet.
    let root = tmp.path();

    assert!(
        !root.join("kanban-worker").exists(),
        "kanban-worker subdir should not exist before sync"
    );
    assert!(
        !root.join("kanban-orchestrator").exists(),
        "kanban-orchestrator subdir should not exist before sync"
    );

    sync_bundled_kanban_skills(root, false).expect("sync should create parent dirs");

    assert!(
        root.join("kanban-worker").is_dir(),
        "kanban-worker/ subdir should be created by sync"
    );
    assert!(
        root.join("kanban-orchestrator").is_dir(),
        "kanban-orchestrator/ subdir should be created by sync"
    );
}

#[test]
fn sync_idempotent_on_second_call() {
    let tmp = temp_skills_root();
    let root = tmp.path();

    // First sync.
    sync_bundled_kanban_skills(root, false).expect("first sync");

    let worker_content_before =
        std::fs::read_to_string(root.join("kanban-worker").join("SKILL.md"))
            .expect("read worker before second sync");

    // Second sync — should be a no-op.
    let report = sync_bundled_kanban_skills(root, false).expect("second sync");

    let worker_content_after = std::fs::read_to_string(root.join("kanban-worker").join("SKILL.md"))
        .expect("read worker after second sync");

    assert_eq!(
        worker_content_before, worker_content_after,
        "second sync must not change file content (idempotent)"
    );
    assert_eq!(
        report.worker_action,
        SyncAction::Skipped,
        "second sync must report Skipped (no-op)"
    );
    assert_eq!(
        report.orchestrator_action,
        SyncAction::Skipped,
        "second sync must report Skipped (no-op)"
    );
}

#[test]
fn bundled_content_matches_include_str_source() {
    // Byte-for-byte check: the synced file must equal the include_str! constant.
    let tmp = temp_skills_root();
    let root = tmp.path();

    sync_bundled_kanban_skills(root, false).expect("sync");

    let worker_on_disk =
        std::fs::read(root.join("kanban-worker").join("SKILL.md")).expect("read worker bytes");
    let orchestrator_on_disk = std::fs::read(root.join("kanban-orchestrator").join("SKILL.md"))
        .expect("read orchestrator bytes");

    assert_eq!(
        worker_on_disk.as_slice(),
        KANBAN_WORKER_SKILL_MD.as_bytes(),
        "synced kanban-worker SKILL.md must match bundled bytes exactly"
    );
    assert_eq!(
        orchestrator_on_disk.as_slice(),
        KANBAN_ORCHESTRATOR_SKILL_MD.as_bytes(),
        "synced kanban-orchestrator SKILL.md must match bundled bytes exactly"
    );
}
