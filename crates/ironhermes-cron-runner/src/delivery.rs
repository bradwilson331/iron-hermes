use std::path::{Path, PathBuf};
use std::sync::Arc;
use ironhermes_core::Config;
use ironhermes_cron::{
    format_delivery_message, is_silent, CronJob, DeliveryTarget,
    TgSendApi, // Relocated from ironhermes-gateway in Task 1 step (a)
};
use tracing::error;

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
// MEDIA routing (Plan 32.1-07 Task 3 — closes Plan 06 deferral)
// ---------------------------------------------------------------------------

/// Classification of a media file by extension for routing to the
/// correct Telegram send method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Image,
    Video,
    Voice,
    Document,
}

/// Classify a media path by its file extension (case-insensitive).
/// Unknown or missing extensions fall through to `Document`.
fn classify_media_path(path: &Path) -> MediaKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => MediaKind::Image,
        "mp4" | "mov" | "webm" | "mkv" => MediaKind::Video,
        "ogg" | "opus" | "mp3" | "wav" | "m4a" => MediaKind::Voice,
        _ => MediaKind::Document,
    }
}

/// Route each extracted media path to the appropriate `TgSendApi` method.
///
/// Text body must already have been sent via `send_message` before calling
/// this (caption-style ordering: text first, then attachments).
///
/// Returns a `Vec<String>` of per-path error strings; empty means success.
async fn route_media_payload(
    target: &DeliveryTarget,
    paths: &[PathBuf],
    tg: &Arc<dyn TgSendApi>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for path in paths {
        let result = match classify_media_path(path) {
            MediaKind::Image => {
                tg.send_image_file(&target.chat_id, path, target.thread_id.as_deref())
                    .await
            }
            MediaKind::Video => {
                tg.send_video(&target.chat_id, path, target.thread_id.as_deref())
                    .await
            }
            MediaKind::Voice => {
                tg.send_voice(&target.chat_id, path, target.thread_id.as_deref())
                    .await
            }
            MediaKind::Document => {
                tg.send_document(&target.chat_id, path, target.thread_id.as_deref())
                    .await
            }
        };
        if let Err(e) = result {
            errors.push(format!(
                "{}:{}:media:{}: {}",
                target.platform,
                target.chat_id,
                path.display(),
                e
            ));
        }
    }
    errors
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
        // Strip MEDIA: tags from the text body and collect their paths;
        // the paths are dispatched via route_media_payload after the text
        // send below (caption-style ordering).
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
                        // Route media after text body (caption-style ordering — Test 9)
                        if !media_paths.is_empty() {
                            let media_errors =
                                route_media_payload(target, &media_paths, tg).await;
                            errors.extend(media_errors);
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
        /// Recorded (chat_id, content, thread_id) send_message calls
        calls: Mutex<Vec<(String, String, Option<String>)>>,
        /// Recorded (chat_id, path, thread_id) send_image_file calls
        images: Mutex<Vec<(String, PathBuf, Option<String>)>>,
        /// Recorded (chat_id, path, thread_id) send_video calls
        videos: Mutex<Vec<(String, PathBuf, Option<String>)>>,
        /// Recorded (chat_id, path, thread_id) send_voice calls
        voices: Mutex<Vec<(String, PathBuf, Option<String>)>>,
        /// Recorded (chat_id, path, thread_id) send_document calls
        documents: Mutex<Vec<(String, PathBuf, Option<String>)>>,
        /// chat_ids whose send_message should return Err
        fail_on: HashSet<String>,
        /// paths whose media send_* should return Err
        fail_media: HashSet<PathBuf>,
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

        async fn send_voice(
            &self,
            chat_id: &str,
            path: &std::path::Path,
            thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            if self.fail_media.contains(path) {
                return Err(anyhow::anyhow!("simulated media failure for {}", path.display()));
            }
            self.voices.lock().unwrap().push((
                chat_id.to_string(),
                path.to_path_buf(),
                thread_id.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn send_image_file(
            &self,
            chat_id: &str,
            path: &std::path::Path,
            thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            if self.fail_media.contains(path) {
                return Err(anyhow::anyhow!("simulated media failure for {}", path.display()));
            }
            self.images.lock().unwrap().push((
                chat_id.to_string(),
                path.to_path_buf(),
                thread_id.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn send_video(
            &self,
            chat_id: &str,
            path: &std::path::Path,
            thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            if self.fail_media.contains(path) {
                return Err(anyhow::anyhow!("simulated media failure for {}", path.display()));
            }
            self.videos.lock().unwrap().push((
                chat_id.to_string(),
                path.to_path_buf(),
                thread_id.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn send_document(
            &self,
            chat_id: &str,
            path: &std::path::Path,
            thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            if self.fail_media.contains(path) {
                return Err(anyhow::anyhow!("simulated media failure for {}", path.display()));
            }
            self.documents.lock().unwrap().push((
                chat_id.to_string(),
                path.to_path_buf(),
                thread_id.map(|s| s.to_string()),
            ));
            Ok(())
        }
    }

    impl FakeTg {
        fn with_fail_on(chat_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                images: Mutex::new(vec![]),
                videos: Mutex::new(vec![]),
                voices: Mutex::new(vec![]),
                documents: Mutex::new(vec![]),
                fail_on: chat_ids.into_iter().map(|s| s.into()).collect(),
                fail_media: HashSet::new(),
            }
        }

        fn with_fail_media(paths: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                images: Mutex::new(vec![]),
                videos: Mutex::new(vec![]),
                voices: Mutex::new(vec![]),
                documents: Mutex::new(vec![]),
                fail_on: HashSet::new(),
                fail_media: paths.into_iter().map(|p| p.into()).collect(),
            }
        }

        fn recorded_calls(&self) -> Vec<(String, String, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }

        fn recorded_images(&self) -> Vec<(String, PathBuf, Option<String>)> {
            self.images.lock().unwrap().clone()
        }

        fn recorded_videos(&self) -> Vec<(String, PathBuf, Option<String>)> {
            self.videos.lock().unwrap().clone()
        }

        fn recorded_voices(&self) -> Vec<(String, PathBuf, Option<String>)> {
            self.voices.lock().unwrap().clone()
        }

        fn recorded_documents(&self) -> Vec<(String, PathBuf, Option<String>)> {
            self.documents.lock().unwrap().clone()
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
    // Plan 32.1-07 Task 3: MEDIA routing behavior tests
    // -----------------------------------------------------------------------

    // Test M1: no media — only send_message, no media methods called
    #[tokio::test]
    async fn testm1_no_media_only_send_message() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];

        let errors = dispatch_all_targets(
            targets,
            "no media here",
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        assert_eq!(tg.recorded_calls().len(), 1, "exactly one send_message");
        assert!(tg.recorded_images().is_empty(), "no image calls");
        assert!(tg.recorded_videos().is_empty(), "no video calls");
        assert!(tg.recorded_voices().is_empty(), "no voice calls");
        assert!(tg.recorded_documents().is_empty(), "no document calls");
    }

    // Test M2: image routing — png
    #[tokio::test]
    async fn testm2_image_routing_png() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];
        let output = "caption text\nMEDIA: /tmp/a.png";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        assert_eq!(tg.recorded_calls().len(), 1, "one send_message");
        let images = tg.recorded_images();
        assert_eq!(images.len(), 1, "one send_image_file call");
        assert_eq!(images[0].1, PathBuf::from("/tmp/a.png"));
        assert!(tg.recorded_videos().is_empty());
        assert!(tg.recorded_voices().is_empty());
        assert!(tg.recorded_documents().is_empty());
    }

    // Test M3: image routing — jpg, jpeg, gif, webp
    #[tokio::test]
    async fn testm3_image_routing_all_extensions() {
        for ext in &["jpg", "jpeg", "gif", "webp"] {
            let job = make_job("telegram:42");
            let config = Config::default();
            let tg = Arc::new(FakeTg::default());
            let targets = vec![make_target("telegram", "42")];
            let output = format!("MEDIA: /tmp/file.{}", ext);

            let errors = dispatch_all_targets(
                targets,
                &output,
                &job,
                &config,
                Some(&(tg.clone() as Arc<dyn TgSendApi>)),
            )
            .await;

            assert!(errors.is_empty(), "unexpected errors for .{}: {:?}", ext, errors);
            assert_eq!(tg.recorded_images().len(), 1, ".{} must route to send_image_file", ext);
            assert!(tg.recorded_videos().is_empty(), ".{} must not route to send_video", ext);
            assert!(tg.recorded_voices().is_empty(), ".{} must not route to send_voice", ext);
            assert!(tg.recorded_documents().is_empty(), ".{} must not route to send_document", ext);
        }
    }

    // Test M4: video routing — mp4, mov, webm, mkv
    #[tokio::test]
    async fn testm4_video_routing_all_extensions() {
        for ext in &["mp4", "mov", "webm", "mkv"] {
            let job = make_job("telegram:42");
            let config = Config::default();
            let tg = Arc::new(FakeTg::default());
            let targets = vec![make_target("telegram", "42")];
            let output = format!("MEDIA: /tmp/file.{}", ext);

            let errors = dispatch_all_targets(
                targets,
                &output,
                &job,
                &config,
                Some(&(tg.clone() as Arc<dyn TgSendApi>)),
            )
            .await;

            assert!(errors.is_empty(), "unexpected errors for .{}: {:?}", ext, errors);
            assert_eq!(tg.recorded_videos().len(), 1, ".{} must route to send_video", ext);
            assert!(tg.recorded_images().is_empty(), ".{} must not route to send_image_file", ext);
            assert!(tg.recorded_voices().is_empty(), ".{} must not route to send_voice", ext);
            assert!(tg.recorded_documents().is_empty(), ".{} must not route to send_document", ext);
        }
    }

    // Test M5: voice routing — ogg, opus, mp3, wav, m4a
    #[tokio::test]
    async fn testm5_voice_routing_all_extensions() {
        for ext in &["ogg", "opus", "mp3", "wav", "m4a"] {
            let job = make_job("telegram:42");
            let config = Config::default();
            let tg = Arc::new(FakeTg::default());
            let targets = vec![make_target("telegram", "42")];
            let output = format!("MEDIA: /tmp/file.{}", ext);

            let errors = dispatch_all_targets(
                targets,
                &output,
                &job,
                &config,
                Some(&(tg.clone() as Arc<dyn TgSendApi>)),
            )
            .await;

            assert!(errors.is_empty(), "unexpected errors for .{}: {:?}", ext, errors);
            assert_eq!(tg.recorded_voices().len(), 1, ".{} must route to send_voice", ext);
            assert!(tg.recorded_images().is_empty(), ".{} must not route to send_image_file", ext);
            assert!(tg.recorded_videos().is_empty(), ".{} must not route to send_video", ext);
            assert!(tg.recorded_documents().is_empty(), ".{} must not route to send_document", ext);
        }
    }

    // Test M6: document fallback — unknown extension
    #[tokio::test]
    async fn testm6_document_fallback_unknown_extension() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];
        let output = "MEDIA: /tmp/blob.xyz";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let docs = tg.recorded_documents();
        assert_eq!(docs.len(), 1, "unknown extension must route to send_document");
        assert_eq!(docs[0].1, PathBuf::from("/tmp/blob.xyz"));
        assert!(tg.recorded_images().is_empty());
        assert!(tg.recorded_videos().is_empty());
        assert!(tg.recorded_voices().is_empty());
    }

    // Test M7: document fallback — no extension
    #[tokio::test]
    async fn testm7_document_fallback_no_extension() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];
        let output = "MEDIA: /tmp/no_ext_file";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let docs = tg.recorded_documents();
        assert_eq!(docs.len(), 1, "no extension must route to send_document");
        assert_eq!(docs[0].1, PathBuf::from("/tmp/no_ext_file"));
    }

    // Test M8: multiple MEDIA lines, mixed extensions — all three dispatched in order
    #[tokio::test]
    async fn testm8_multiple_media_lines_mixed_extensions() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];
        let output = "header\nMEDIA: /tmp/a.png\nMEDIA: /tmp/b.mp4\nMEDIA: /tmp/c.xyz\nfooter";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        assert_eq!(tg.recorded_images().len(), 1, "one image");
        assert_eq!(tg.recorded_videos().len(), 1, "one video");
        assert_eq!(tg.recorded_documents().len(), 1, "one document");
        assert!(tg.recorded_voices().is_empty(), "no voices");
        // text body has all MEDIA lines stripped
        let calls = tg.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].1.contains("MEDIA:"), "body must not contain MEDIA: lines");
        assert!(calls[0].1.contains("header"), "body must contain non-MEDIA content");
        assert!(calls[0].1.contains("footer"), "body must contain non-MEDIA content");
    }

    // Test M9: text-first ordering — send_message called before send_image_file
    #[tokio::test]
    async fn testm9_text_first_ordering() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Use call-order counter
        struct OrderedFake {
            counter: Arc<AtomicUsize>,
            message_order: Mutex<Option<usize>>,
            image_order: Mutex<Option<usize>>,
        }

        #[async_trait]
        impl TgSendApi for OrderedFake {
            async fn send_message(&self, _: &str, _: &str, _: Option<&str>) -> anyhow::Result<()> {
                let n = self.counter.fetch_add(1, Ordering::SeqCst);
                *self.message_order.lock().unwrap() = Some(n);
                Ok(())
            }
            async fn send_voice(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
            async fn send_image_file(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> {
                let n = self.counter.fetch_add(1, Ordering::SeqCst);
                *self.image_order.lock().unwrap() = Some(n);
                Ok(())
            }
            async fn send_video(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
            async fn send_document(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let fake = Arc::new(OrderedFake {
            counter: counter.clone(),
            message_order: Mutex::new(None),
            image_order: Mutex::new(None),
        });

        let job = make_job("telegram:42");
        let config = Config::default();
        let targets = vec![make_target("telegram", "42")];
        let tg: Arc<dyn TgSendApi> = fake.clone();

        let errors = dispatch_all_targets(
            targets,
            "caption text\nMEDIA: /tmp/a.png",
            &job,
            &config,
            Some(&tg),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let msg_ord = fake.message_order.lock().unwrap().expect("send_message not called");
        let img_ord = fake.image_order.lock().unwrap().expect("send_image_file not called");
        assert!(
            msg_ord < img_ord,
            "send_message (order {}) must be called BEFORE send_image_file (order {})",
            msg_ord,
            img_ord
        );
    }

    // Test M10: per-target media error accumulated
    #[tokio::test]
    async fn testm10_media_error_accumulated() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::with_fail_media([PathBuf::from("/tmp/a.png")]));
        let targets = vec![make_target("telegram", "42")];
        let output = "caption\nMEDIA: /tmp/a.png";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert_eq!(errors.len(), 1, "expected 1 media error: {:?}", errors);
        assert!(
            errors[0].contains("telegram:42:media:/tmp/a.png"),
            "error should include platform:chat_id:media:path: {}",
            errors[0]
        );
        // text send_message succeeded — no text error
        assert_eq!(tg.recorded_calls().len(), 1, "send_message must still have been called");
    }

    // Test M11: no adapter for media — existing "no adapter available" error preserved
    #[tokio::test]
    async fn testm11_no_adapter_for_media_uses_existing_error() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let targets = vec![make_target("telegram", "42")];
        let output = "MEDIA: /tmp/a.png\nsome text";

        let errors = dispatch_all_targets(targets, output, &job, &config, None).await;

        assert_eq!(errors.len(), 1, "expected 1 error: {:?}", errors);
        assert!(
            errors[0].contains("telegram:42") && errors[0].contains("no adapter available"),
            "error should be 'no adapter available': {}",
            errors[0]
        );
    }

    // Test M12: case-insensitive extension — .PNG routes to send_image_file
    #[tokio::test]
    async fn testm12_case_insensitive_extension() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];
        let output = "MEDIA: /tmp/IMG.PNG";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        assert_eq!(tg.recorded_images().len(), 1, ".PNG must route to send_image_file");
    }

    // Test M13: relative path (no leading slash) — passed verbatim to adapter
    #[tokio::test]
    async fn testm13_relative_path_passed_verbatim() {
        let job = make_job("telegram:42");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "42")];
        let output = "MEDIA: blob.png";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        let images = tg.recorded_images();
        assert_eq!(images.len(), 1, "relative .png must route to send_image_file");
        assert_eq!(images[0].1, PathBuf::from("blob.png"), "path must be passed verbatim");
    }

    // Test M14: [SILENT] gate respected — no MEDIA extraction, no media sends
    #[tokio::test]
    async fn testm14_silent_gate_suppresses_media() {
        let job = make_job_with_origin("origin");
        let config = Config::default();
        let tg = Arc::new(FakeTg::default());
        let targets = vec![make_target("telegram", "12345")];
        let output = "[SILENT] MEDIA: /tmp/a.png";

        let errors = dispatch_all_targets(
            targets,
            output,
            &job,
            &config,
            Some(&(tg.clone() as Arc<dyn TgSendApi>)),
        )
        .await;

        assert!(errors.is_empty(), "silent output must produce no errors");
        assert!(tg.recorded_calls().is_empty(), "no send_message for silent output");
        assert!(tg.recorded_images().is_empty(), "no send_image_file for silent output");
        assert!(tg.recorded_videos().is_empty(), "no send_video for silent output");
        assert!(tg.recorded_voices().is_empty(), "no send_voice for silent output");
        assert!(tg.recorded_documents().is_empty(), "no send_document for silent output");
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
