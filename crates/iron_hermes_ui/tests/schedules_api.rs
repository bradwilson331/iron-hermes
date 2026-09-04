//! Phase 46.9 Plan 04 — Source-string shape locks + data-layer round-trip
//! test for `schedules_api.rs`.
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot `use iron_hermes_ui::...` (see
//! tests/kanban_write_fns.rs, tests/provider_config_api.rs for the same
//! constraint). The functional CRUD/validation/`is_valid` tests for
//! `schedules_api.rs`'s internals live as `#[cfg(test)]` unit tests inside
//! the file itself (`cargo test -p iron_hermes_ui --lib schedules_api`).
//! This file locks the module's *shape* via source-string assertions plus a
//! CRUD + run-now data-layer round trip exercised directly against
//! `ironhermes_cron::JobStore`, which — unlike `iron_hermes_ui` — is a
//! normal importable lib crate.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

// ---------------------------------------------------------------------------
// Shape locks (source-string assertions)
// ---------------------------------------------------------------------------

#[test]
fn schedules_api_declares_the_six_server_fns() {
    let src = read("src/server/schedules_api.rs");
    for name in [
        "get_schedules",
        "create_schedule",
        "update_schedule",
        "delete_schedule",
        "set_schedule_enabled",
        "run_schedule_now",
    ] {
        assert!(
            src.contains(&format!("fn {}", name)),
            "schedules_api.rs must declare `fn {}`",
            name,
        );
    }
    assert!(
        src.matches("#[server]").count() >= 6,
        "schedules_api.rs must have at least 6 #[server] fns"
    );
}

#[test]
fn schedules_api_module_is_registered() {
    let src = read("src/server/mod.rs");
    assert!(
        src.contains("pub mod schedules_api;"),
        "server/mod.rs must contain `pub mod schedules_api;`"
    );
}

/// D-06: schedule strings are validated exclusively through
/// `ironhermes_cron::parse_schedule` — no hand-rolled/second parser.
#[test]
fn schedules_api_validates_via_parse_schedule() {
    let src = read("src/server/schedules_api.rs");
    assert!(
        src.matches("parse_schedule").count() >= 1,
        "schedules_api.rs must call parse_schedule to validate every schedule string"
    );
}

/// Mirrors `patch_task_status`'s spawn_blocking store-mutate shape
/// (kanban_api.rs:473-497) — every write opens the JobStore inside
/// `tokio::task::spawn_blocking`.
#[test]
fn schedules_api_uses_spawn_blocking() {
    let src = read("src/server/schedules_api.rs");
    assert!(
        src.contains("spawn_blocking"),
        "schedules_api.rs must wrap JobStore I/O in tokio::task::spawn_blocking"
    );
}

/// D-06 prohibition: CRUD logic is ported from ironhermes-cli/src/cron.rs
/// directly into schedules_api.rs — it must never shell out to the CLI
/// binary.
#[test]
fn schedules_api_never_shells_out_to_the_cli() {
    let src = read("src/server/schedules_api.rs");
    assert_eq!(
        src.matches("Command::new").count(),
        0,
        "schedules_api.rs must never shell out via Command::new — CRUD logic \
         is ported into #[server] fns, not invoked as a subprocess"
    );
}

/// Rule 2 addition: prompt-injection/security scanning parity with the CLI's
/// `cmd_create`/`cmd_edit` (`ironhermes-cli/src/cron.rs`), which both call
/// `scan_cron_prompt` before persisting. The web surface schedules arbitrary
/// agent prompts to run unattended and must not drop this safeguard.
#[test]
fn schedules_api_scans_prompt_for_injection() {
    let src = read("src/server/schedules_api.rs");
    assert!(
        src.contains("scan_cron_prompt"),
        "schedules_api.rs must call ironhermes_cron::scan_cron_prompt before \
         persisting a create/update — CLI parity (ironhermes-cli/src/cron.rs)"
    );
}

/// Phase 49.5 WR-01: the blueprint create path is a second surface that
/// persists an agent prompt, and slot values substitute verbatim into
/// `prompt_template`. It must carry the same `scan_cron_prompt` safeguard the
/// manual create/edit path above does — otherwise a crafted slot value is
/// written durably to jobs.json and only caught at tick time.
#[test]
fn blueprints_api_scans_prompt_for_injection() {
    let src = read("src/server/blueprints_api.rs");
    assert!(
        src.contains("scan_cron_prompt"),
        "blueprints_api.rs must call ironhermes_cron::scan_cron_prompt before \
         persisting a blueprint-filled job — parity with schedules_api.rs"
    );
}

/// The `iron_hermes_ui` Cargo.toml references the new native-only
/// `ironhermes-cron` path dependency.
#[test]
fn cargo_toml_references_ironhermes_cron_dep() {
    let src = read("Cargo.toml");
    assert!(
        src.contains("ironhermes-cron = { path = \"../ironhermes-cron\" }"),
        "Cargo.toml must add the native-only ironhermes-cron path dependency"
    );
}

/// Gap 2 (D-06 / T-46.9-20): all five write fns must check
/// `web_config_write_enabled` before any `JobStore` mutation — one
/// occurrence per write fn, minimum.
#[test]
fn schedules_api_write_fns_gate_on_web_config_write_enabled() {
    let src = read("src/server/schedules_api.rs");
    assert!(
        src.matches("web_config_write_enabled").count() >= 5,
        "schedules_api.rs must gate each of the five write fns \
         (create_schedule, update_schedule, delete_schedule, \
         set_schedule_enabled, run_schedule_now) on \
         security.web_config_write_enabled — found {} occurrences",
        src.matches("web_config_write_enabled").count(),
    );
    assert!(
        src.contains("Config writes are disabled"),
        "schedules_api.rs must return the shared gate-closed error copy"
    );
}

/// Gap 2 (D-06): the read path (`get_schedules`) must remain ungated — only
/// the five write fns fail closed. Approximated by checking that the gate
/// text does not appear between the `get_schedules` fn signature and the
/// doc comment introducing the next fn (`create_schedule`) — using the
/// doc-comment text (not the `#[server]`/`pub async fn` boundary) as the end
/// marker so `create_schedule`'s own gate-explaining doc comment is
/// correctly excluded from the slice.
#[test]
fn get_schedules_is_not_gated() {
    let src = read("src/server/schedules_api.rs");
    let start = src
        .find("pub async fn get_schedules")
        .expect("get_schedules fn must exist");
    let end = src
        .find("Create a new scheduled job")
        .expect("create_schedule's doc comment must exist");
    assert!(
        end > start,
        "create_schedule must be declared after get_schedules"
    );
    let get_schedules_body = &src[start..end];
    assert!(
        !get_schedules_body.contains("web_config_write_enabled"),
        "get_schedules (a read path) must not be gated on web_config_write_enabled"
    );
}

// ---------------------------------------------------------------------------
// Data-layer round trip (against ironhermes_cron::JobStore directly)
// ---------------------------------------------------------------------------

/// CRUD + run-now round trip: create -> list -> toggle -> run-now -> delete,
/// exercised directly against a temp-dir-backed JobStore (mirrors the
/// `schedules_api.rs`-internal `_in_store` helpers this integration test
/// cannot import, since `iron_hermes_ui` has no `src/lib.rs`).
#[test]
fn crud_and_run_now_round_trip_against_job_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = ironhermes_cron::JobStore::open(dir.path().join("cron")).expect("open store");

    let schedule = ironhermes_cron::parse_schedule("every 30m").expect("parse");
    let job = store
        .add_job(
            "integration-job",
            "do the thing",
            schedule,
            "every 30m",
            "local",
            vec![],
            None,
        )
        .expect("add_job");

    assert_eq!(store.list_jobs().len(), 1);

    store.toggle_job(&job.id, false).expect("toggle off");
    assert!(!store.get_job(&job.id).unwrap().enabled);
    store.toggle_job(&job.id, true).expect("toggle on");
    assert!(store.get_job(&job.id).unwrap().enabled);

    store.trigger_job(&job.id).expect("run now");
    let triggered = store.get_job(&job.id).unwrap();
    assert!(
        triggered.next_run_at.is_some(),
        "trigger_job must set next_run_at"
    );

    store.remove_job(&job.id).expect("delete");
    assert!(store.list_jobs().is_empty());
}

/// Malformed-row backstop (store layer): a job whose stored cron expression
/// is unparseable garbage must still load and list without panicking. The
/// `is_valid=false` rendering assertion is the `schedules_api.rs`-internal
/// unit test `malformed_cron_row_is_invalid` (this crate has no lib.rs, so
/// that private-fn-level assertion cannot live here — mirrors
/// `tests/provider_config_api.rs`'s identical split for its own partial
/// backstop).
///
/// `JobStore::add_job` validates the cron expression eagerly (via
/// `compute_next_run` -> `Schedule::from_str`), so a malformed `Cron` row
/// can only exist on disk via a hand-edited/externally-written `jobs.json`
/// (structurally valid JSON, semantically bad cron string) — write that raw
/// JSON directly, mirroring `ironhermes-cron/src/store.rs`'s own
/// `reload_picks_up_external_mutations` test fixture shape.
#[test]
fn malformed_cron_row_loads_and_lists_without_panicking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cron_dir = dir.path().join("cron");
    std::fs::create_dir_all(&cron_dir).expect("create cron dir");
    let raw = serde_json::json!([{
        "id": "corrupted-cron-id",
        "name": "corrupted-cron",
        "prompt": "do something",
        "skills": [],
        "schedule": { "kind": "cron", "expr": "not a valid cron", "display": "not a valid cron" },
        "schedule_display": "not a valid cron",
        "repeat": { "times": null, "completed": 0 },
        "enabled": true,
        "state": "scheduled",
        "paused_at": null,
        "paused_reason": null,
        "deliver": "local",
        "origin": null,
        "created_at": "2026-01-01T00:00:00Z",
        "next_run_at": null,
        "last_run_at": null,
        "last_status": null,
        "last_error": null
    }]);
    fs::write(cron_dir.join("jobs.json"), raw.to_string()).expect("write jobs.json");

    // Must not panic.
    let store = ironhermes_cron::JobStore::open(cron_dir).expect("open store with malformed row");
    let jobs = store.list_jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "corrupted-cron");
}
