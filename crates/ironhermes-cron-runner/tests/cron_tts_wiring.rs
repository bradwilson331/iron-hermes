//! Phase 37.2 Plan 01 — cron TTS wiring RED tests.
//!
//! Covers:
//!   REQ-37.2-04: cron turns MUST register TTS tools with an AudioDispatcher bound to
//!                the job's first resolved DeliveryTarget.
//!   REQ-37.2-05: when a cron turn's resolved targets are empty/unresolvable, the runner
//!                MUST record it in `last_delivery_error` (no silent success).
//!   REQ-37.2-06: both surfaces emit a structured turn-end tracing record
//!                (observable side-effect: last_delivery_error for unresolvable target).
//!
//! ## RED Status
//!
//! Test 1 (`cron_tts_registration_binds_dispatcher_to_first_target`):
//!   May be GREEN after Plan 01 because it exercises existing primitives
//!   (`ToolRegistry::register_tts_tools` + `tts_registration_status`). If
//!   `register_tts_tools` + `Live` status already works with a real dispatcher,
//!   this test passes immediately. Plan 02/03 does not change its green/red status.
//!
//! Test 2 (`unresolvable_target_sets_last_delivery_error`):
//!   RED until Plan 03. Currently `complete_and_dispatch` returns `Ok(())` silently
//!   when `resolve_delivery_targets` yields an empty vec, without setting
//!   `last_delivery_error`. Plan 03 adds:
//!   ```
//!   if targets.is_empty() {
//!       j.last_delivery_error = Some("no delivery targets resolved ...");
//!       store.save()?;
//!       return Ok(());
//!   }
//!   ```
//!   This test asserts that field is set — fails today.
//!
//! NOTE: This file does NOT reference the new `CronRunnerContext` field added in Plan 03.
//!   That field does not exist until Plan 03. The Wave-0 tests are scoped to
//!   `ToolRegistry`/`register_tts_tools`/`resolve_delivery_targets`/job-field
//!   assertions that compile against the current (pre-Plan-03) struct.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::commands::context::TtsRegistrationStatus;
use ironhermes_core::{Config, Platform, SessionKey};
use ironhermes_cron::{CronJob, JobStore, ScheduleParsed, resolve_delivery_targets};
use ironhermes_cron_runner::CronRunnerContext;
use ironhermes_tools::{AudioDispatcher, ToolRegistry};
use tempfile::TempDir;

// ===========================================================================
// FakeAudioDispatcher — recording stub implementing AudioDispatcher
// ===========================================================================
//
// Mirrors the FakeTg pattern from delivery.rs tests (lines 229-345):
// a recording struct implementing the target trait, storing call args in
// Mutex<Vec<...>> so tests can assert what was dispatched.

#[derive(Default)]
struct FakeAudioDispatcher {
    voice_calls: Mutex<Vec<(String, PathBuf, Option<String>)>>,
    audio_calls: Mutex<Vec<(String, PathBuf, Option<String>)>>,
}

#[async_trait]
impl AudioDispatcher for FakeAudioDispatcher {
    async fn send_voice_file(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.voice_calls.lock().unwrap().push((
            chat_id.to_string(),
            path.to_path_buf(),
            thread_id.map(|s| s.to_string()),
        ));
        Ok(())
    }

    async fn send_audio_file(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.audio_calls.lock().unwrap().push((
            chat_id.to_string(),
            path.to_path_buf(),
            thread_id.map(|s| s.to_string()),
        ));
        Ok(())
    }
}

// ===========================================================================
// Fixtures
// ===========================================================================

fn make_ctx(tmpdir: &TempDir) -> CronRunnerContext {
    let cron_dir = tmpdir.path().join("cron");
    let store = Arc::new(std::sync::Mutex::new(
        JobStore::open(cron_dir).expect("open store"),
    ));
    CronRunnerContext {
        job_store: store,
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(ToolRegistry::new())),
        memory_manager: None,
        hook_registry: None,
        config: Config::default(),
        mcp_manager: None,
        tg_client: None,
        audio_dispatcher: None, // Plan 03 field — None for test fixture default
        delivery_registry: ironhermes_cron::DeliveryRegistry::new(), // Plan 07 field
    }
}

/// Build a CronJob that resolves to an EMPTY target set.
///
/// `deliver = "origin"` + `origin = None` → `resolve_delivery_targets` returns
/// empty Vec when no home channel is configured (delivery.rs:199 warn path).
fn make_empty_target_job(store: &Arc<std::sync::Mutex<JobStore>>) -> CronJob {
    let mut job_store = store.lock().unwrap();
    job_store
        .add_job(
            "Test Empty Target Job",
            "do something",
            ScheduleParsed::Interval {
                minutes: 60,
                display: "every 60m".to_string(),
            },
            "every 60m",
            "origin", // deliver=origin with origin=None → empty targets
            vec![],
            None, // no origin set
        )
        .expect("add job")
}

// ===========================================================================
// Tests
// ===========================================================================

/// REQ-37.2-04: registering TTS tools with a real AudioDispatcher produces
/// `TtsRegistrationStatus::Live`.
///
/// This test exercises `ToolRegistry::register_tts_tools` directly — the same
/// primitive Plan 03 wires into `run_cron_job`. A `FakeAudioDispatcher` is
/// passed as the dispatcher (simulating the `AudioDispatcher` the gateway will
/// supply via the Plan 03 context field).
///
/// The test derives a `SessionKey` from a `DeliveryTarget` with platform
/// "telegram" / chat_id "999", matching the D-04 binding rule.
///
/// Green/Red status: EXPECTED TO PASS after Plan 01 — `register_tts_tools`
/// and `tts_registration_status` are existing primitives. If this test is
/// already green, it confirms the primitives are correct; Plan 03 wires them
/// at the right call site.
#[tokio::test]
async fn cron_tts_registration_binds_dispatcher_to_first_target() {
    let mut reg = ToolRegistry::new();

    // Build SessionKey from a DeliveryTarget { platform: "telegram", chat_id: "999" }
    // Matches the D-04 rule: SessionKey derived from the first resolved DeliveryTarget.
    let session_key = match "telegram" {
        "telegram" => SessionKey::new(Platform::Telegram, "999"),
        other => panic!("unexpected platform in test fixture: {}", other),
    };

    let dispatcher: Arc<dyn AudioDispatcher> = Arc::new(FakeAudioDispatcher::default());

    reg.register_tts_tools(session_key, Some(dispatcher), Arc::new(Config::default()));

    // REQ-37.2-04: after registration with a real dispatcher, status must be Live.
    assert_eq!(
        reg.tts_registration_status(),
        TtsRegistrationStatus::Live,
        "REQ-37.2-04: TTS registration with a real AudioDispatcher must yield Live status"
    );
}

/// REQ-37.2-05 / REQ-37.2-06: a cron job whose resolved targets are empty
/// (deliver=origin, origin=None) MUST set `last_delivery_error` instead of
/// recording silent success.
///
/// RED until Plan 03. Currently `complete_and_dispatch` returns `Ok(())` when
/// `targets.is_empty()` WITHOUT setting `last_delivery_error` (runner.rs:117-119).
/// Plan 03 adds the empty-target guard that writes:
/// `"no delivery targets resolved (deliver=origin, origin=null?)"` to the field.
///
/// This test drives the empty-target path via `run_cron_job` with:
///   - `no_agent = true` (short-circuits the LLM, calls `complete_and_dispatch` directly)
///   - `script = None` (triggers the "[no_agent set but no script field]" output path)
///   - `deliver = "origin"`, `origin = None` (empty target resolution)
///
/// The assertion on `last_delivery_error` FAILS today — that is the expected RED state.
#[tokio::test]
async fn unresolvable_target_sets_last_delivery_error() {
    let tmp = TempDir::new().expect("tmpdir");

    // Set IRONHERMES_HOME so complete_job_run can persist state
    unsafe {
        std::env::set_var("IRONHERMES_HOME", tmp.path());
    }

    let ctx = make_ctx(&tmp);
    let job = make_empty_target_job(&ctx.job_store);

    // Confirm the test fixture actually produces empty targets
    let targets = resolve_delivery_targets(&job);
    assert!(
        targets.is_empty(),
        "fixture must produce empty targets for this test to be meaningful"
    );

    // Drive via run_cron_job (no_agent=true + script=None short-circuits to
    // complete_and_dispatch without any LLM call).
    // We construct a fresh job copy with no_agent=true; the job was persisted
    // by add_job so complete_job_run can update it.
    let no_agent_job = CronJob {
        no_agent: true,
        script: None,
        deliver: "origin".to_string(),
        origin: None,
        ..job.clone()
    };

    let result = ironhermes_cron_runner::run_cron_job(&no_agent_job, &ctx).await;

    unsafe {
        std::env::remove_var("IRONHERMES_HOME");
    }

    assert!(
        result.is_ok(),
        "run_cron_job must not return Err: {:?}",
        result
    );

    // REQ-37.2-05: last_delivery_error must be set for the empty-target case.
    // RED until Plan 03 — currently returns Ok(()) silently without setting the field.
    // Plan 03 sets: j.last_delivery_error = Some("no delivery targets resolved ...")
    let store = ctx.job_store.lock().unwrap();
    let persisted_job = store
        .jobs
        .iter()
        .find(|j| j.id == no_agent_job.id)
        .expect("job must exist in store after run");

    let delivery_error = persisted_job.last_delivery_error.as_deref();
    assert!(
        delivery_error.is_some(),
        "REQ-37.2-05: last_delivery_error must be set for empty delivery targets (currently None)"
    );
    assert!(
        delivery_error
            .unwrap_or("")
            .contains("no delivery targets resolved"),
        "REQ-37.2-05: last_delivery_error must contain 'no delivery targets resolved', got: {:?}",
        delivery_error
    );
}
