//! Phase 22.4.2.1 Plan 02 — regression test for cron -> Telegram delivery.
//!
//! Originally tested `dispatch_delivery` (gateway-local single-target function,
//! deleted in Plan 32.1-07). Now tests `dispatch_all_targets` from
//! `ironhermes-cron-runner`, which owns delivery dispatch as of Plan 32.1-07.
//!
//! The behavioral contracts are identical — no assertions changed.
//! No live HTTP — pure trait-object dispatch.

use std::sync::{Arc, Mutex};

use ironhermes_core::Config;
use ironhermes_cron::{DeliveryTarget, TgSendApi};
use ironhermes_cron::job::{CronJob, JobState, RepeatConfig, ScheduleParsed};
use ironhermes_cron_runner::dispatch_all_targets;
use chrono::Utc;

struct FakeTgClient {
    pub calls: Arc<Mutex<Vec<(String, String, Option<String>)>>>, // (chat_id, content, thread_id)
    pub fail: bool,
}

#[async_trait::async_trait]
impl TgSendApi for FakeTgClient {
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push((
            chat_id.to_string(),
            content.to_string(),
            thread_id.map(|s| s.to_string()),
        ));
        if self.fail {
            Err(anyhow::anyhow!("fake send failure"))
        } else {
            Ok(())
        }
    }

    async fn send_voice(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
    async fn send_image_file(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
    async fn send_video(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
    async fn send_document(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
}

fn fake_tg(fail: bool) -> Arc<FakeTgClient> {
    Arc::new(FakeTgClient {
        calls: Arc::new(Mutex::new(vec![])),
        fail,
    })
}

fn make_job(deliver: &str) -> CronJob {
    CronJob {
        id: "test-job".to_string(),
        name: "test-job".to_string(),
        prompt: "test".to_string(),
        skills: vec![],
        schedule: ScheduleParsed::Interval {
            minutes: 60,
            display: "every 60m".to_string(),
        },
        schedule_display: "every 60m".to_string(),
        repeat: RepeatConfig::default(),
        enabled: true,
        state: JobState::Scheduled,
        paused_at: None,
        paused_reason: None,
        deliver: deliver.to_string(),
        origin: None,
        created_at: Utc::now(),
        next_run_at: None,
        last_run_at: None,
        last_status: None,
        last_error: None,
        model: None,
        provider: None,
        base_url: None,
        script: None,
        no_agent: false,
        context_from: None,
        enabled_toolsets: None,
        workdir: None,
        last_delivery_error: None,
    }
}

fn config_no_wrap() -> Config {
    let mut c = Config::default();
    c.cron.wrap_response = false;
    c
}

/// A telegram DeliveryTarget should invoke send_message with the correct chat_id + content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telegram_target_invokes_send_message() {
    let fake = fake_tg(false);
    let target = DeliveryTarget {
        platform: "telegram".to_string(),
        chat_id: "12345".to_string(),
        thread_id: None,
    };
    let job = make_job("telegram:12345");
    let config = config_no_wrap();
    let tg_client: Arc<dyn TgSendApi> = fake.clone();
    let errors = dispatch_all_targets(
        vec![target],
        "hello world",
        &job,
        &config,
        Some(&tg_client),
    ).await;
    assert!(errors.is_empty(), "expected no errors: {:?}", errors);
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "expected exactly one send_message call");
    assert_eq!(calls[0].0, "12345", "chat_id must match target.chat_id");
    assert!(calls[0].1.contains("hello world"), "content must contain job output");
    assert_eq!(calls[0].2, None, "thread_id must be None");
}

/// An empty targets list must NOT invoke send_message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_targets_does_not_call_tg() {
    let fake = fake_tg(false);
    let job = make_job("local");
    let config = Config::default();
    let tg_client: Arc<dyn TgSendApi> = fake.clone();
    let errors = dispatch_all_targets(
        vec![],
        "some output",
        &job,
        &config,
        Some(&tg_client),
    ).await;
    assert!(errors.is_empty());
    let calls = fake.calls.lock().unwrap();
    assert!(
        calls.is_empty(),
        "empty targets must not invoke send_message"
    );
}

/// A TG send failure must be non-fatal: dispatch_all_targets returns an error string,
/// the function does not panic, and the error is accumulated (D-09 non-fatal contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tg_send_failure_is_non_fatal() {
    let fake = fake_tg(true); // fail=true → send_message returns Err
    let target = DeliveryTarget {
        platform: "telegram".to_string(),
        chat_id: "99999".to_string(),
        thread_id: None,
    };
    let job = make_job("telegram:99999");
    let config = config_no_wrap();
    let tg_client: Arc<dyn TgSendApi> = fake.clone();
    // Must return without panic — D-09: delivery failure is non-fatal
    let errors = dispatch_all_targets(
        vec![target],
        "error output",
        &job,
        &config,
        Some(&tg_client),
    ).await;
    // Error should be accumulated
    assert_eq!(errors.len(), 1, "expected 1 accumulated error");
    assert!(errors[0].contains("99999"), "error should mention chat_id: {}", errors[0]);
}
