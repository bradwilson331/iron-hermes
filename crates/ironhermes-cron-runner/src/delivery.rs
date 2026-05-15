use std::path::PathBuf;
use std::sync::Arc;
use ironhermes_core::Config;
use ironhermes_cron::{
    format_delivery_message, is_silent, CronJob, DeliveryTarget,
    TgSendApi, // Relocated from ironhermes-gateway in Task 1 step (a)
};
use tracing::{error, warn};

// ---------------------------------------------------------------------------
// wrap_for_delivery
// ---------------------------------------------------------------------------

/// Apply the wrap_response header/footer and `format_delivery_message`
/// truncation to a raw output body.
///
/// When `config.cron.wrap_response = true` (the default):
///   - Prepends `Cronjob Response: {job.name}\n(job_id: {job.id})\n---\n`
///   - Appends `\n\nTo stop or manage this job, send me a new message...`
///
/// Then delegates to `format_delivery_message` for final truncation.
fn wrap_for_delivery(job: &CronJob, config: &Config, body: &str) -> String {
    let wrapped = if config.cron.wrap_response {
        format!(
            "Cronjob Response: {}\n(job_id: {})\n---\n{}\n\nTo stop or manage this job, send me a new message...",
            job.name, job.id, body
        )
    } else {
        body.to_string()
    };
    format_delivery_message(&job.name, &wrapped)
}

// ---------------------------------------------------------------------------
// extract_media_paths
// ---------------------------------------------------------------------------

/// Strip `MEDIA:` lines from an output body, returning:
/// - The cleaned body (with MEDIA lines removed)
/// - The list of media `PathBuf`s extracted
///
/// Lines matching `^MEDIA:\s*(\S+)$` are consumed; all other lines are kept.
/// Routing to `send_voice`/`send_image_file`/`send_video` is deferred to Plan 07.
fn extract_media_paths(body: &str) -> (String, Vec<PathBuf>) {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("MEDIA:") {
            let p = rest.trim();
            if !p.is_empty() {
                paths.push(PathBuf::from(p));
            }
        } else {
            kept.push(line);
        }
    }
    (kept.join("\n"), paths)
}

// ---------------------------------------------------------------------------
// dispatch_all_targets
// ---------------------------------------------------------------------------

/// Dispatch the cron job output to all resolved delivery targets.
///
/// Returns a `Vec<String>` of per-target error strings. An empty vec means
/// all targets succeeded (or there were no targets).
///
/// Defensive behaviours:
/// - Returns immediately on `[SILENT]` output (even if upstream missed the gate)
/// - Accumulates per-target errors rather than aborting on first failure
/// - Returns `"telegram:<id>: no adapter available"` when `tg_client` is `None`
/// - Returns `"<platform>:<id>: unsupported platform"` for non-Telegram targets
pub async fn dispatch_all_targets(
    targets: Vec<DeliveryTarget>,
    output: &str,
    job: &CronJob,
    config: &Config,
    tg_client: Option<&Arc<dyn TgSendApi>>,
) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    // Defensive silent gate
    if is_silent(output) {
        return errors;
    }

    for target in &targets {
        // Extract MEDIA: tags (TODO Plan 07: route by extension via
        // send_voice/send_image_file/send_video). For now, body minus
        // MEDIA: lines is sent as text; media paths are logged.
        let (body_no_media, media_paths) = extract_media_paths(output);

        let payload = wrap_for_delivery(job, config, &body_no_media);

        match target.platform.as_str() {
            "telegram" => {
                let Some(tg) = tg_client else {
                    errors.push(format!(
                        "telegram:{}: no adapter available",
                        target.chat_id
                    ));
                    continue;
                };
                match tg
                    .send_message(&target.chat_id, &payload, target.thread_id.as_deref())
                    .await
                {
                    Ok(_) => {
                        if !media_paths.is_empty() {
                            warn!(
                                media_count = media_paths.len(),
                                "MEDIA: paths extracted but routing to \
                                 send_voice/send_image_file/send_video \
                                 is deferred (Plan 07). Paths logged only."
                            );
                        }
                    }
                    Err(e) => {
                        error!(
                            job_id = %job.id,
                            chat_id = %target.chat_id,
                            "telegram delivery failed: {}",
                            e
                        );
                        errors.push(format!("telegram:{}: {}", target.chat_id, e));
                    }
                }
            }
            other => {
                errors.push(format!("{}:{}: unsupported platform", other, target.chat_id));
            }
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_cron::job::{JobOrigin, JobState, RepeatConfig, ScheduleParsed};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use chrono::Utc;

    // ---------------------------------------------------------------------------
    // FakeTg — testable TgSendApi implementation
    // ---------------------------------------------------------------------------

    #[derive(Default)]
    struct FakeTg {
        /// Recorded (chat_id, content, thread_id) calls
        calls: Mutex<Vec<(String, String, Option<String>)>>,
        /// chat_ids whose send_message should return Err
        fail_on: HashSet<String>,
    }

    #[async_trait]
    impl TgSendApi for FakeTg {
        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            if self.fail_on.contains(chat_id) {
                return Err(anyhow::anyhow!("simulated failure for {}", chat_id));
            }
            self.calls
                .lock()
                .unwrap()
                .push((chat_id.to_string(), content.to_string(), thread_id.map(|s| s.to_string())));
            Ok(())
        }
    }

    impl FakeTg {
        fn with_fail_on(chat_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                fail_on: chat_ids.into_iter().map(|s| s.into()).collect(),
            }
        }

        fn recorded_calls(&self) -> Vec<(String, String, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    // ---------------------------------------------------------------------------
    // Job fixture
    // ---------------------------------------------------------------------------

    fn make_job(deliver: &str) -> CronJob {
        CronJob {
            id: "job-1".to_string(),
            name: "Test Job".to_string(),
            prompt: "do something".to_string(),
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

    fn make_job_with_origin(deliver: &str) -> CronJob {
        let mut job = make_job(deliver);
        job.origin = Some(JobOrigin {
            platform: "telegram".to_string(),
            chat_id: "12345".to_string(),
            chat_name: None,
            thread_id: None,
        });
        job
    }

    fn config_with_wrap(wrap: bool) -> Config {
        let mut c = Config::default();
        c.cron.wrap_response = wrap;
        c
    }

    fn make_target(platform: &str, chat_id: &str) -> DeliveryTarget {
        DeliveryTarget {
            platform: platform.to_string(),
            chat_id: chat_id.to_string(),
            thread_id: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: empty targets — no work, no errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test1_empty_targets_returns_no_errors() {
        let job = make_job("local");
        let config = Config::default();
        let errors = dispatch_all_targets(vec![], "output", &job, &config, None).await;
        assert!(errors.is_empty(), "expected no errors for empty targets");
    }

    // -----------------------------------------------------------------------
    // Test 2: single target success with wrap_response=true (default)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test2_single_target_success_wrap_response_true() {
        let job = make_job("telegram:42");
        let config = config_with_wrap(true);
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];

        let errors = dispatch_all_targets(
            targets,
            "out",
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "expected no errors: {:?}", errors);
        let calls = tg.recorded_calls();
        assert_eq!(calls.len(), 1);
        let (chat_id, payload, thread_id) = &calls[0];
        assert_eq!(chat_id, "42");
        assert!(
            payload.contains("Cronjob Response: Test Job"),
            "wrap header missing in: {payload}"
        );
        assert_eq!(*thread_id, None);
    }

    // -----------------------------------------------------------------------
    // Test 3: wrap_response=false — payload equals format_delivery_message directly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test3_wrap_response_false_no_header_no_footer() {
        let job = make_job("telegram:42");
        let config = config_with_wrap(false);
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];

        let errors = dispatch_all_targets(
            targets,
            "out",
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty());
        let calls = tg.recorded_calls();
        assert_eq!(calls.len(), 1);
        let expected = format_delivery_message(&job.name, "out");
        assert_eq!(calls[0].1, expected);
        assert!(
            !calls[0].1.contains("Cronjob Response:"),
            "should not have wrap header when wrap_response=false"
        );
        assert!(
            !calls[0].1.contains("To stop or manage this job"),
            "should not have wrap footer when wrap_response=false"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: multi-target with partial failure
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test4_multi_target_partial_failure() {
        let job = make_job("telegram:42,discord:99");
        let config = Config::default();
        // discord (chat_id "99") fails, telegram ("42") succeeds
        let tg = Arc::new(FakeTg::with_fail_on(["99"]));
        let targets = vec![
            make_target("telegram", "42"),
            make_target("discord", "99"),
        ];

        let errors = dispatch_all_targets(
            targets,
            "output",
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        // Discord is unsupported — produces "unsupported platform" error, not the fail_on error
        // (FakeTg is called for telegram only — discord arm hits the "other" match arm)
        assert_eq!(errors.len(), 1, "expected exactly 1 error: {:?}", errors);
        assert!(
            errors[0].contains("discord") && errors[0].contains("unsupported platform"),
            "error should be unsupported platform for discord: {}",
            errors[0]
        );
        // Telegram still received its call
        let calls = tg.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "42");
    }

    // -----------------------------------------------------------------------
    // Test 5: multi-target both succeed
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test5_multi_target_both_telegram_succeed() {
        let job = make_job("telegram:42,telegram:99");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![
            make_target("telegram", "42"),
            make_target("telegram", "99"),
        ];

        let errors = dispatch_all_targets(
            targets,
            "output",
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "expected no errors: {:?}", errors);
        let calls = tg.recorded_calls();
        assert_eq!(calls.len(), 2);
        let ids: Vec<&str> = calls.iter().map(|(id, _, _)| id.as_str()).collect();
        assert!(ids.contains(&"42"));
        assert!(ids.contains(&"99"));
    }

    // -----------------------------------------------------------------------
    // Test 6: tg_client=None for telegram target — "no adapter available"
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test6_tg_client_none_returns_no_adapter_error() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let targets = vec![make_target("telegram", "42")];

        let errors = dispatch_all_targets(targets, "output", &job, &config, None).await;

        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("telegram:42") && errors[0].contains("no adapter available"),
            "unexpected error string: {}",
            errors[0]
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: unsupported platform — "unsupported platform"
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test7_unsupported_platform_error() {
        let job = make_job("reddit:abc");
        let config = Config::default();
        let targets = vec![make_target("reddit", "abc")];

        let errors = dispatch_all_targets(targets, "output", &job, &config, None).await;

        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("reddit:abc") && errors[0].contains("unsupported platform"),
            "unexpected error string: {}",
            errors[0]
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: MEDIA tag extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test8_extract_media_paths() {
        let body = "MEDIA: /tmp/file.png\nrest of message\nmore content";
        let (cleaned, paths) = extract_media_paths(body);
        assert_eq!(paths.len(), 1, "expected 1 media path");
        assert_eq!(paths[0], PathBuf::from("/tmp/file.png"));
        assert!(
            !cleaned.contains("MEDIA:"),
            "cleaned body should not contain MEDIA: line"
        );
        assert!(
            cleaned.contains("rest of message"),
            "cleaned body should contain non-MEDIA content"
        );
    }

    #[tokio::test]
    async fn test8b_dispatch_strips_media_from_body() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];
        let output = "MEDIA: /tmp/img.jpg\nactual message content";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let calls = tg.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert!(
            !calls[0].1.contains("MEDIA:"),
            "payload must not contain MEDIA: line"
        );
        assert!(
            calls[0].1.contains("actual message content"),
            "payload must contain non-MEDIA content"
        );
    }

    // -----------------------------------------------------------------------
    // Test 9: [SILENT] output — no calls, no errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test9_silent_output_suppresses_delivery() {
        let job = make_job_with_origin("origin");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "12345")];
        let output = "[SILENT] this is silent output";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "silent output must produce no errors");
        assert!(
            tg.recorded_calls().is_empty(),
            "send_message must not be called for silent output"
        );
    }

    // -----------------------------------------------------------------------
    // Test: telegram target with send failure — error accumulated
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_telegram_send_failure_accumulated() {
        let job = make_job("telegram:fail-me");
        let config = Config::default();
        let tg = Arc::new(FakeTg::with_fail_on(["fail-me"]));
        let targets = vec![make_target("telegram", "fail-me")];

        let errors = dispatch_all_targets(
            targets,
            "output",
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("telegram:fail-me"),
            "error should include platform:chat_id prefix: {}",
            errors[0]
        );
    }
}
