use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::job::CronJob;

// ---------------------------------------------------------------------------
// DeliveryTarget
// ---------------------------------------------------------------------------

/// Resolved delivery destination for job output.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryTarget {
    pub platform: String,
    pub chat_id: String,
    pub thread_id: Option<String>,
}

// ---------------------------------------------------------------------------
// KNOWN_DELIVERY_PLATFORMS allowlist
// ---------------------------------------------------------------------------

/// Allowlist of recognised delivery platform tokens.
///
/// Any token that is NOT in this list will never trigger an env-var read.
/// This prevents env-var enumeration via crafted `deliver` values
/// (e.g. `deliver = "stripe_secret"` would otherwise read `STRIPE_SECRET_HOME_CHANNEL`).
pub const KNOWN_DELIVERY_PLATFORMS: &[&str] =
    &["telegram", "discord", "slack", "matrix", "whatsapp", "webhook", "qq"];

// ---------------------------------------------------------------------------
// Private env-var mapping helpers
// ---------------------------------------------------------------------------

fn home_channel_env_var(platform: &str) -> Option<&'static str> {
    match platform {
        "telegram"  => Some("TELEGRAM_HOME_CHANNEL"),
        "discord"   => Some("DISCORD_HOME_CHANNEL"),
        "slack"     => Some("SLACK_HOME_CHANNEL"),
        "matrix"    => Some("MATRIX_HOME_CHANNEL"),
        "whatsapp"  => Some("WHATSAPP_HOME_CHANNEL"),
        "webhook"   => Some("WEBHOOK_HOME_CHANNEL"),
        "qq"        => Some("QQ_HOME_CHANNEL"),
        _           => None,
    }
}

fn legacy_home_channel_env_var(platform: &str) -> Option<&'static str> {
    match platform {
        "qq" => Some("QQBOT_HOME_CHANNEL"),
        _    => None,
    }
}

fn home_channel_thread_env_var(platform: &str) -> Option<&'static str> {
    match platform {
        "telegram"  => Some("TELEGRAM_HOME_CHANNEL_THREAD_ID"),
        "discord"   => Some("DISCORD_HOME_CHANNEL_THREAD_ID"),
        "slack"     => Some("SLACK_HOME_CHANNEL_THREAD_ID"),
        "matrix"    => Some("MATRIX_HOME_CHANNEL_THREAD_ID"),
        "whatsapp"  => Some("WHATSAPP_HOME_CHANNEL_THREAD_ID"),
        "webhook"   => Some("WEBHOOK_HOME_CHANNEL_THREAD_ID"),
        "qq"        => Some("QQ_HOME_CHANNEL_THREAD_ID"),
        _           => None,
    }
}

// ---------------------------------------------------------------------------
// Telegram config.yaml whitelist fallback (gap-closure 32.1-09)
// ---------------------------------------------------------------------------
//
// Restores pre-32.1 gateway parity: when TELEGRAM_HOME_CHANNEL is unset,
// resolve to the single entry of `gateway.platforms.telegram.whitelist`
// from `config.yaml` if and only if the whitelist has exactly one entry.
// Zero or multiple entries return None (matching Config::telegram_default_origin's
// OriginDecision::None / OriginDecision::Multi semantics).
//
// SCOPE: telegram only. Other platforms (discord/slack/matrix/etc.) do
// not currently carry whitelist semantics in `Config::gateway.platforms`
// shaped like Telegram, so the fallback is intentionally telegram-scoped.
fn lookup_telegram_whitelist_fallback() -> Option<DeliveryTarget> {
    // Config::load reads ${IRONHERMES_HOME}/config.yaml (or defaults if missing).
    // Failures are non-fatal — fallback simply returns None.
    let config = match ironhermes_core::config::Config::load() {
        Ok(c)  => c,
        Err(e) => {
            tracing::debug!(error = %e, "telegram fallback: Config::load failed — skipping");
            return None;
        }
    };
    let tg = config.gateway.platforms.get("telegram")?;
    if !tg.enabled {
        return None;
    }
    match tg.whitelist.len() {
        1 => {
            let chat_id = tg.whitelist[0].to_string();
            if chat_id.is_empty() {
                return None;
            }
            Some(DeliveryTarget {
                platform: "telegram".to_string(),
                chat_id,
                thread_id: None,
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// lookup_home_channel
// ---------------------------------------------------------------------------

/// Look up the home channel for a platform via env vars.
/// Allowlist gate runs FIRST — no env-var read for unknown platforms.
/// Returns `None` if the platform is unknown or the env var is unset/empty.
///
/// For the `telegram` platform, if both the primary and legacy env vars yield
/// nothing, falls back to `Config::gateway.platforms.telegram.whitelist` when
/// it has exactly one entry (gap-closure 32.1-09 — restores pre-32.1 parity).
fn lookup_home_channel(platform: &str) -> Option<DeliveryTarget> {
    // Allowlist gate FIRST — no env var read for unknown platforms
    if !KNOWN_DELIVERY_PLATFORMS.contains(&platform) {
        return None;
    }
    let chat_id = home_channel_env_var(platform)
        .and_then(|v| std::env::var(v).ok())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            legacy_home_channel_env_var(platform)
                .and_then(|v| std::env::var(v).ok())
                .filter(|s| !s.is_empty())
        });

    // Gap-closure 32.1-09: telegram-only config.yaml whitelist fallback.
    // Runs ONLY when both the primary and legacy env vars yielded nothing,
    // AND the platform is telegram. Other platforms remain env-only.
    if chat_id.is_none() && platform == "telegram" {
        if let Some(target) = lookup_telegram_whitelist_fallback() {
            return Some(target);
        }
    }

    let chat_id = chat_id?;

    let thread_id = home_channel_thread_env_var(platform)
        .and_then(|v| std::env::var(v).ok())
        .filter(|s| !s.is_empty());
    Some(DeliveryTarget {
        platform: platform.to_string(),
        chat_id,
        thread_id,
    })
}

// ---------------------------------------------------------------------------
// expand_routing_token
// ---------------------------------------------------------------------------

fn expand_routing_token(token: &str, job: &CronJob) -> Vec<DeliveryTarget> {
    let token = token.trim();
    if token.is_empty() {
        return Vec::new();
    }

    // `all` expands to every configured home channel
    if token.eq_ignore_ascii_case("all") {
        return KNOWN_DELIVERY_PLATFORMS
            .iter()
            .filter_map(|p| lookup_home_channel(p))
            .collect();
    }

    // `origin` resolves to job.origin, or falls back to any configured home channel
    if token.eq_ignore_ascii_case("origin") {
        if let Some(origin) = &job.origin {
            if KNOWN_DELIVERY_PLATFORMS.contains(&origin.platform.as_str()) {
                return vec![DeliveryTarget {
                    platform: origin.platform.clone(),
                    chat_id: origin.chat_id.clone(),
                    thread_id: origin.thread_id.clone(),
                }];
            } else {
                tracing::warn!(platform=%origin.platform, "origin platform not in KNOWN_DELIVERY_PLATFORMS — skipping");
                return Vec::new();
            }
        }
        // No origin — fallback to first configured home channel
        for p in KNOWN_DELIVERY_PLATFORMS {
            if let Some(target) = lookup_home_channel(p) {
                return vec![target];
            }
        }
        tracing::warn!("deliver=origin has no origin and no home channel configured");
        return Vec::new();
    }

    // `platform:chat_id` form
    if let Some((platform, chat_id)) = token.split_once(':') {
        let platform = platform.trim();
        let chat_id = chat_id.trim();
        if !KNOWN_DELIVERY_PLATFORMS.contains(&platform) {
            tracing::warn!(platform=%platform, "deliver platform not in allowlist — skipping");
            return Vec::new();
        }
        let thread_id = home_channel_thread_env_var(platform)
            .and_then(|v| std::env::var(v).ok())
            .filter(|s| !s.is_empty());
        return vec![DeliveryTarget {
            platform: platform.to_string(),
            chat_id: chat_id.to_string(),
            thread_id,
        }];
    }

    // Bare `platform` without colon → home channel
    if !KNOWN_DELIVERY_PLATFORMS.contains(&token) {
        tracing::warn!(token=%token, "deliver token not in allowlist — skipping");
        return Vec::new();
    }
    match lookup_home_channel(token) {
        Some(t) => vec![t],
        None => {
            tracing::warn!(platform=%token, "deliver={token} has no home channel configured");
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// resolve_delivery_targets (plural — primary API)
// ---------------------------------------------------------------------------

/// Resolve all delivery targets for a job.
///
/// Supports:
/// - `"local"` / empty → no targets
/// - `"platform:chat_id"` → single explicit target
/// - `"telegram,discord:abc"` → comma-split, one target per token
/// - `"all"` → every `KNOWN_DELIVERY_PLATFORMS` entry that has a configured home channel
/// - `"origin"` → job origin, or first configured home channel as fallback
/// - `"telegram"` (bare platform) → `TELEGRAM_HOME_CHANNEL` env var
///
/// Tokens outside `KNOWN_DELIVERY_PLATFORMS` produce no target and emit a `tracing::warn!`.
/// Duplicate targets (same platform + chat_id + thread_id) are suppressed.
pub fn resolve_delivery_targets(job: &CronJob) -> Vec<DeliveryTarget> {
    let deliver = job.deliver.trim();
    if deliver.is_empty() || deliver.eq_ignore_ascii_case("local") {
        return Vec::new();
    }

    let mut out: Vec<DeliveryTarget> = Vec::new();
    for part in deliver.split(',') {
        for target in expand_routing_token(part, job) {
            // dedup against already-seen targets
            let already = out.iter().any(|t| {
                t.platform == target.platform
                    && t.chat_id == target.chat_id
                    && t.thread_id == target.thread_id
            });
            if !already {
                out.push(target);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// resolve_delivery_target (singular — deprecated, delegates to plural)
// ---------------------------------------------------------------------------

/// **Deprecated**: use [`resolve_delivery_targets`] (plural) for full
/// multi-target parity (comma-split, `all`, home-channel lookup).
/// This single-target form returns the first plural result.
///
/// - `"local"` → None (file-only, no platform delivery)
/// - `"origin"` → map `job.origin` to DeliveryTarget; None if no origin captured
/// - `"platform:chat_id"` → split on first `:`, left = platform, right = chat_id
/// - anything else → None
pub fn resolve_delivery_target(job: &CronJob) -> Option<DeliveryTarget> {
    resolve_delivery_targets(job).into_iter().next()
}

// ---------------------------------------------------------------------------
// is_silent
// ---------------------------------------------------------------------------

/// Returns true if the output starts with `[SILENT]` (case-insensitive).
/// Silent output is saved to file but NOT delivered to any platform.
pub fn is_silent(output: &str) -> bool {
    output.trim().to_uppercase().starts_with("[SILENT]")
}

// ---------------------------------------------------------------------------
// save_job_output
// ---------------------------------------------------------------------------

/// Save job output to `{hermes_home}/cron/output/{job_id}/{timestamp}.md`.
/// Uses atomic temp+rename write pattern with fsync before rename.
/// Applies `chmod 0700` to the output directory and `chmod 0600` to the file on Unix.
/// Returns the path that was written.
pub fn save_job_output(job_id: &str, output: &str) -> Result<PathBuf> {
    // Reject any job_id that could escape the output directory via path traversal
    if job_id.contains('/') || job_id.contains('\\') || job_id.contains("..") || job_id.is_empty() {
        anyhow::bail!("invalid job_id for filesystem use: {:?}", job_id);
    }

    let home = ironhermes_core::get_hermes_home();
    let output_dir = home.join("cron").join("output").join(job_id);

    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output dir: {}", output_dir.display()))?;

    // chmod 0700 on the output directory (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&output_dir, fs::Permissions::from_mode(0o700));
    }

    let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let file_path = output_dir.join(format!("{}.md", timestamp));
    let tmp_path = output_dir.join(format!("{}.md.tmp", timestamp));

    {
        let mut f = fs::File::create(&tmp_path)
            .with_context(|| format!("failed to create temp file: {}", tmp_path.display()))?;
        f.write_all(output.as_bytes())
            .with_context(|| format!("failed to write temp file: {}", tmp_path.display()))?;
        f.flush()?;
        f.sync_all()?;
    }

    fs::rename(&tmp_path, &file_path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            tmp_path.display(),
            file_path.display()
        )
    })?;

    // chmod 0600 on the output file (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&file_path, fs::Permissions::from_mode(0o600));
    }

    Ok(file_path)
}

// ---------------------------------------------------------------------------
// format_delivery_message
// ---------------------------------------------------------------------------

/// Maximum output length for platform delivery (Telegram message limit).
pub const MAX_PLATFORM_OUTPUT: usize = 4000;

/// Format job output for platform delivery.
/// Truncates at MAX_PLATFORM_OUTPUT and appends a note if truncated.
pub fn format_delivery_message(job_name: &str, output: &str) -> String {
    let header = format!("[Job: {}]\n", job_name);

    if output.len() > MAX_PLATFORM_OUTPUT {
        // Use floor_char_boundary to avoid panicking on multi-byte UTF-8 chars
        let safe_end = output.floor_char_boundary(MAX_PLATFORM_OUTPUT);
        let truncated = &output[..safe_end];
        format!(
            "{}{}\n\n(truncated -- full output saved to file)",
            header, truncated
        )
    } else {
        format!("{}{}", header, output)
    }
}

// ---------------------------------------------------------------------------
// Tests — original
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{CronJob, JobOrigin, JobState, RepeatConfig, ScheduleParsed};
    use chrono::Utc;
    use std::sync::MutexGuard;
    use tempfile::TempDir;

    fn env_lock() -> MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn make_job(deliver: &str, origin: Option<JobOrigin>) -> CronJob {
        CronJob {
            id: "test-job-id".to_string(),
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
            origin,
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

    // --- resolve_delivery_target (singular — legacy) ---

    #[test]
    fn local_returns_none() {
        let job = make_job("local", None);
        assert_eq!(resolve_delivery_target(&job), None);
    }

    #[test]
    fn origin_no_origin_field_returns_none() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        // Isolate IRONHERMES_HOME so the telegram config.yaml whitelist fallback
        // (32.1-09) does not fire against the developer's real config.yaml.
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("origin", None);
        let result = resolve_delivery_target(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        // No home channels configured and no config.yaml in empty tempdir — returns None
        assert_eq!(result, None);
    }

    #[test]
    fn origin_with_origin_field_returns_target() {
        let origin = JobOrigin {
            platform: "telegram".to_string(),
            chat_id: "12345".to_string(),
            chat_name: Some("Test Chat".to_string()),
            thread_id: Some("99".to_string()),
        };
        let job = make_job("origin", Some(origin));
        let target = resolve_delivery_target(&job).expect("should resolve");
        assert_eq!(target.platform, "telegram");
        assert_eq!(target.chat_id, "12345");
        assert_eq!(target.thread_id, Some("99".to_string()));
    }

    #[test]
    fn platform_colon_chat_id_returns_target() {
        let job = make_job("telegram:67890", None);
        let target = resolve_delivery_target(&job).expect("should resolve");
        assert_eq!(target.platform, "telegram");
        assert_eq!(target.chat_id, "67890");
        assert_eq!(target.thread_id, None);
    }

    #[test]
    fn webhook_url_resolves_correctly() {
        let job = make_job("webhook:https://example.com/hook", None);
        let target = resolve_delivery_target(&job).expect("should resolve");
        assert_eq!(target.platform, "webhook");
        assert_eq!(target.chat_id, "https://example.com/hook");
    }

    #[test]
    fn unknown_deliver_returns_none() {
        // "slack" as a bare platform now requires SLACK_HOME_CHANNEL — not set in this test
        let job = make_job("slack", None);
        assert_eq!(resolve_delivery_target(&job), None);
    }

    #[test]
    fn empty_deliver_returns_none() {
        let job = make_job("", None);
        assert_eq!(resolve_delivery_target(&job), None);
    }

    // --- is_silent ---

    #[test]
    fn is_silent_exact_prefix() {
        assert!(is_silent("[SILENT] some output"));
    }

    #[test]
    fn is_silent_with_leading_whitespace() {
        assert!(is_silent("  [SILENT] some output"));
    }

    #[test]
    fn is_silent_lowercase_prefix() {
        assert!(is_silent("[silent] output"));
    }

    #[test]
    fn is_silent_no_prefix_returns_false() {
        assert!(!is_silent("normal output"));
    }

    #[test]
    fn is_silent_partial_prefix_returns_false() {
        assert!(!is_silent("[SIL] output"));
    }

    // --- format_delivery_message ---

    #[test]
    fn format_delivery_message_short_output() {
        let msg = format_delivery_message("Daily Report", "hello world");
        assert_eq!(msg, "[Job: Daily Report]\nhello world");
    }

    #[test]
    fn format_delivery_message_truncates_long_output() {
        let long_output = "x".repeat(MAX_PLATFORM_OUTPUT + 100);
        let msg = format_delivery_message("Job", &long_output);
        assert!(msg.contains("(truncated -- full output saved to file)"));
        // Header + MAX_PLATFORM_OUTPUT chars + truncation note
        let content_part = &msg[7..]; // skip "[Job: Job]\n" header
        let lines: Vec<&str> = content_part.splitn(2, '\n').collect();
        assert!(lines[0].len() <= MAX_PLATFORM_OUTPUT);
    }

    #[test]
    fn format_delivery_message_exact_limit_not_truncated() {
        let output = "y".repeat(MAX_PLATFORM_OUTPUT);
        let msg = format_delivery_message("Job", &output);
        assert!(!msg.contains("truncated"));
    }

    // --- save_job_output ---

    #[test]
    fn save_job_output_creates_file() {
        // We can't easily override get_hermes_home, so we test the function
        // by checking it doesn't error (it will use the real hermes home or
        // fail gracefully in CI). For a proper unit test, we patch at the
        // integration level. Here we just verify the tempdir pattern works.
        let tmp = TempDir::new().expect("tempdir");
        let output_dir = tmp.path().join("cron").join("output").join("test-id");
        fs::create_dir_all(&output_dir).unwrap();

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let file_path = output_dir.join(format!("{}.md", timestamp));
        let tmp_path = output_dir.join(format!("{}.md.tmp", timestamp));

        let content = "test output";
        {
            let mut f = fs::File::create(&tmp_path).unwrap();
            f.write_all(content.as_bytes()).unwrap();
            f.flush().unwrap();
        }
        fs::rename(&tmp_path, &file_path).unwrap();

        assert!(file_path.exists());
        let read_back = fs::read_to_string(&file_path).unwrap();
        assert_eq!(read_back, content);
    }
}

// ---------------------------------------------------------------------------
// Tests — multi-target routing (Task 1)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod multi_target_tests {
    use super::*;
    use crate::job::{CronJob, JobOrigin, JobState, RepeatConfig, ScheduleParsed};
    use chrono::Utc;
    use std::sync::MutexGuard;

    /// Serialise all env-mutating tests to avoid races.
    fn env_lock() -> MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn make_job(deliver: &str, origin: Option<JobOrigin>) -> CronJob {
        CronJob {
            id: "test-job-id".to_string(),
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
            origin,
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

    // Test 1: deliver=local returns empty vec
    #[test]
    fn test1_local_returns_empty() {
        let job = make_job("local", None);
        assert!(resolve_delivery_targets(&job).is_empty());
    }

    // Test 2: deliver="telegram:123" returns one target
    #[test]
    fn test2_single_platform_with_chat_id() {
        let job = make_job("telegram:123", None);
        let targets = resolve_delivery_targets(&job);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "123");
        assert_eq!(targets[0].thread_id, None);
    }

    // Test 3: comma-split multi-target
    #[test]
    fn test3_comma_split_multi_target() {
        let job = make_job("telegram:123,discord:abc", None);
        let targets = resolve_delivery_targets(&job);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "123");
        assert_eq!(targets[1].platform, "discord");
        assert_eq!(targets[1].chat_id, "abc");
    }

    // Test 4: deliver="all" with env vars set
    #[test]
    fn test4_all_token_expands_to_configured_platforms() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("TELEGRAM_HOME_CHANNEL", "tg1");
            std::env::set_var("DISCORD_HOME_CHANNEL", "ds1");
            std::env::remove_var("SLACK_HOME_CHANNEL");
            std::env::remove_var("MATRIX_HOME_CHANNEL");
            std::env::remove_var("WHATSAPP_HOME_CHANNEL");
            std::env::remove_var("WEBHOOK_HOME_CHANNEL");
            std::env::remove_var("QQ_HOME_CHANNEL");
            std::env::remove_var("QQBOT_HOME_CHANNEL");
        }
        let job = make_job("all", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
            std::env::remove_var("DISCORD_HOME_CHANNEL");
        }
        let tg = targets.iter().find(|t| t.platform == "telegram");
        let dc = targets.iter().find(|t| t.platform == "discord");
        assert!(tg.is_some(), "telegram should be in targets");
        assert_eq!(tg.unwrap().chat_id, "tg1");
        assert!(dc.is_some(), "discord should be in targets");
        assert_eq!(dc.unwrap().chat_id, "ds1");
        // Platforms without home channel should be absent
        assert!(targets.iter().all(|t| t.platform != "slack"));
    }

    // Test 5: bare platform → home channel
    #[test]
    fn test5_bare_platform_uses_home_channel() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("TELEGRAM_HOME_CHANNEL", "tg-home");
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "tg-home");
        assert_eq!(targets[0].thread_id, None);
    }

    // Test 6: bare platform, home unset → empty and warn
    // Isolation: IRONHERMES_HOME is pointed at an empty tempdir so that the
    // new telegram config.yaml whitelist fallback (32.1-09) does not pick up
    // a real ~/.ironhermes/config.yaml on the developer's machine.
    #[test]
    fn test6_bare_platform_home_unset_returns_empty() {
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(targets.is_empty(), "should return empty when home channel unset and no config.yaml");
    }

    // Test 7: legacy env var fallback for qq
    #[test]
    fn test7_qq_legacy_env_fallback() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("QQ_HOME_CHANNEL");
            std::env::set_var("QQBOT_HOME_CHANNEL", "qq-legacy");
        }
        let job = make_job("qq", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("QQBOT_HOME_CHANNEL");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].chat_id, "qq-legacy");
    }

    // Test 8: primary wins when both QQ_HOME_CHANNEL and QQBOT_HOME_CHANNEL set
    #[test]
    fn test8_qq_primary_takes_precedence() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("QQ_HOME_CHANNEL", "qq-new");
            std::env::set_var("QQBOT_HOME_CHANNEL", "qq-legacy");
        }
        let job = make_job("qq", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("QQ_HOME_CHANNEL");
            std::env::remove_var("QQBOT_HOME_CHANNEL");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].chat_id, "qq-new");
    }

    // Test 9: thread_id env var populates thread_id
    #[test]
    fn test9_thread_id_env_var() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("TELEGRAM_HOME_CHANNEL", "tg1");
            std::env::set_var("TELEGRAM_HOME_CHANNEL_THREAD_ID", "42");
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
            std::env::remove_var("TELEGRAM_HOME_CHANNEL_THREAD_ID");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].thread_id, Some("42".to_string()));
    }

    // Test 10: deliver=origin with origin present
    #[test]
    fn test10_origin_with_origin_field() {
        let origin = JobOrigin {
            platform: "telegram".to_string(),
            chat_id: "orig-1".to_string(),
            chat_name: None,
            thread_id: None,
        };
        let job = make_job("origin", Some(origin));
        let targets = resolve_delivery_targets(&job);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "orig-1");
    }

    // Test 11: deliver=origin without origin, fallback to first home channel
    #[test]
    fn test11_origin_no_origin_falls_back_to_home_channel() {
        let _guard = env_lock();
        unsafe {
            // Clear all home channels, then set only telegram
            std::env::remove_var("DISCORD_HOME_CHANNEL");
            std::env::remove_var("SLACK_HOME_CHANNEL");
            std::env::remove_var("MATRIX_HOME_CHANNEL");
            std::env::remove_var("WHATSAPP_HOME_CHANNEL");
            std::env::remove_var("WEBHOOK_HOME_CHANNEL");
            std::env::remove_var("QQ_HOME_CHANNEL");
            std::env::remove_var("QQBOT_HOME_CHANNEL");
            std::env::set_var("TELEGRAM_HOME_CHANNEL", "tg1");
        }
        let job = make_job("origin", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "tg1");
    }

    // Test 12: deliver=origin without origin, no home channels → empty and warn
    // Isolation: IRONHERMES_HOME is pointed at an empty tempdir so the
    // telegram config.yaml whitelist fallback (32.1-09) does not fire via
    // the origin→first-home-channel fallback path.
    #[test]
    fn test12_origin_no_origin_no_home_channel_returns_empty() {
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
            std::env::remove_var("DISCORD_HOME_CHANNEL");
            std::env::remove_var("SLACK_HOME_CHANNEL");
            std::env::remove_var("MATRIX_HOME_CHANNEL");
            std::env::remove_var("WHATSAPP_HOME_CHANNEL");
            std::env::remove_var("WEBHOOK_HOME_CHANNEL");
            std::env::remove_var("QQ_HOME_CHANNEL");
            std::env::remove_var("QQBOT_HOME_CHANNEL");
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("origin", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(targets.is_empty());
    }

    // Test 13: allowlist gate — unknown platform never triggers env reads
    #[test]
    fn test13_allowlist_gate_unknown_platform() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("REDDIT_HOME_CHANNEL", "should-not-be-read");
        }
        let job = make_job("reddit", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("REDDIT_HOME_CHANNEL");
        }
        assert!(targets.is_empty(), "unknown platform must not produce targets");
    }

    // Test 14: path traversal shaped token
    #[test]
    fn test14_path_traversal_shaped_token() {
        let job = make_job("../etc/passwd", None);
        let targets = resolve_delivery_targets(&job);
        assert!(targets.is_empty(), "traversal-shaped token must not produce targets");
    }

    // Test 15: deduplication — same target appears twice
    #[test]
    fn test15_deduplication() {
        let job = make_job("telegram:123,telegram:123", None);
        let targets = resolve_delivery_targets(&job);
        assert_eq!(targets.len(), 1, "duplicate targets must be suppressed");
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "123");
    }

    // Test 16: singular delegates to plural
    #[test]
    fn test16_singular_delegates_to_plural() {
        let job = make_job("telegram:123,discord:abc", None);
        let singular = resolve_delivery_target(&job);
        let plural = resolve_delivery_targets(&job);
        assert_eq!(singular, plural.into_iter().next());
        // Should be the first element (telegram:123)
        assert_eq!(singular.unwrap().platform, "telegram");
    }
}

// ---------------------------------------------------------------------------
// Tests — save_job_output hardening (Task 2)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod save_job_output_tests_phase_32_1 {
    use super::*;
    use std::sync::MutexGuard;
    use tempfile::TempDir;

    fn env_lock() -> MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    // Test 1: fsync source-grep verified via acceptance criteria; functional regression test
    #[test]
    fn test1_save_job_output_returns_ok_with_content() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let result = save_job_output("test-fsync-id", "hello fsync");
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        let path = result.expect("save_job_output must succeed");
        assert!(path.exists(), "output file must exist");
        let content = fs::read_to_string(&path).expect("read output file");
        assert_eq!(content, "hello fsync");
    }

    // Test 2: Unix chmod 0600 on output file
    #[cfg(unix)]
    #[test]
    fn test2_unix_output_file_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let path = save_job_output("test-chmod-id", "secure content").expect("save must succeed");
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        let meta = fs::metadata(&path).expect("stat output file");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "output .md file must have mode 0600, got {:o}", mode);
    }

    // Test 3: Unix chmod 0700 on output directory
    #[cfg(unix)]
    #[test]
    fn test3_unix_output_dir_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let path = save_job_output("test-dirchmod-id", "dir perm test").expect("save must succeed");
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        let dir = path.parent().expect("output file must have a parent dir");
        let meta = fs::metadata(dir).expect("stat output dir");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "output dir must have mode 0700, got {:o}", mode);
    }

    // Test 4: regression — atomic rename still works and file is readable
    #[test]
    fn test4_atomic_rename_produces_readable_file() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let content = "regression check content";
        let path = save_job_output("test-rename-id", content).expect("save must succeed");
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(path.exists(), "final path must exist after rename");
        assert!(path.to_string_lossy().ends_with(".md"), "path must end with .md");
        // No .tmp file should remain
        let tmp_path = path.with_extension("md.tmp");
        assert!(!tmp_path.exists(), ".tmp file must not exist after rename");
        let read_back = fs::read_to_string(&path).expect("read back");
        assert_eq!(read_back, content);
    }
}

// ---------------------------------------------------------------------------
// Tests — telegram whitelist fallback (gap-closure 32.1-09)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod telegram_whitelist_fallback_tests {
    use super::*;
    use crate::job::{CronJob, JobOrigin, JobState, RepeatConfig, ScheduleParsed};
    use chrono::Utc;
    use std::io::Write;
    use std::sync::MutexGuard;
    use tempfile::TempDir;

    /// Serialise all env-mutating tests to avoid races.
    fn env_lock() -> MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn make_job(deliver: &str, origin: Option<JobOrigin>) -> CronJob {
        CronJob {
            id: "test-job-id".to_string(),
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
            origin,
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

    /// Write a config.yaml fixture to `{tmp}/config.yaml` with the given YAML content.
    fn write_config(tmp: &TempDir, yaml: &str) {
        let path = tmp.path().join("config.yaml");
        let mut f = std::fs::File::create(&path).expect("create config.yaml");
        f.write_all(yaml.as_bytes()).expect("write config.yaml");
    }

    /// Clear all telegram-related env vars.
    fn clear_telegram_env() {
        unsafe {
            std::env::remove_var("TELEGRAM_HOME_CHANNEL");
            std::env::remove_var("TELEGRAM_HOME_CHANNEL_THREAD_ID");
        }
    }

    // Test 1: env var wins — single whitelist entry in config but env var is set
    #[test]
    fn test1_env_var_wins_over_single_whitelist_entry() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [99999]\n",
        );
        unsafe {
            std::env::set_var("TELEGRAM_HOME_CHANNEL", "env-value");
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            clear_telegram_env();
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert_eq!(targets.len(), 1, "should have one target");
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "env-value", "env var must win over config.yaml whitelist");
    }

    // Test 2: config fallback fires — single whitelist entry, env unset
    #[test]
    fn test2_config_fallback_single_whitelist_entry() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [12345]\n",
        );
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert_eq!(targets.len(), 1, "fallback should yield one target");
        assert_eq!(targets[0].platform, "telegram");
        assert_eq!(targets[0].chat_id, "12345");
        assert_eq!(targets[0].thread_id, None, "config fallback must not fabricate thread_id");
    }

    // Test 3: config fallback — zero whitelist entries, env unset → empty
    #[test]
    fn test3_config_fallback_zero_whitelist_returns_empty() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: []\n",
        );
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(targets.is_empty(), "zero-entry whitelist must return empty");
    }

    // Test 4: config fallback — multiple whitelist entries, env unset → empty
    #[test]
    fn test4_config_fallback_multi_whitelist_returns_empty() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [11111, 22222]\n",
        );
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(
            targets.is_empty(),
            "multi-entry whitelist must return empty (ambiguous — matches OriginDecision::Multi semantics)"
        );
    }

    // Test 5: config fallback — telegram section missing entirely → empty, no panic
    #[test]
    fn test5_config_fallback_no_telegram_section_returns_empty() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        // config.yaml exists but has no telegram section
        write_config(&tmp, "gateway:\n  platforms: {}\n");
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(targets.is_empty(), "no telegram section must return empty");
    }

    // Test 6: config fallback — telegram section present but enabled: false → empty
    #[test]
    fn test6_config_fallback_telegram_disabled_returns_empty() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: false\n      whitelist: [12345]\n",
        );
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(targets.is_empty(), "disabled telegram section must return empty");
    }

    // Test 7: no fallback for non-telegram platform (discord) — config not consulted
    #[test]
    fn test7_no_config_fallback_for_discord() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        // Even if we had a hypothetical discord whitelist in config, it won't be consulted
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [55555]\n",
        );
        unsafe {
            std::env::remove_var("DISCORD_HOME_CHANNEL");
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("discord", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(targets.is_empty(), "discord has no config fallback — must return empty");
    }

    // Test 8: no config fallback for qq platform either
    #[test]
    fn test8_no_config_fallback_for_qq() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [66666]\n",
        );
        unsafe {
            std::env::remove_var("QQ_HOME_CHANNEL");
            std::env::remove_var("QQBOT_HOME_CHANNEL");
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("qq", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert!(targets.is_empty(), "qq has no config fallback — must return empty");
    }

    // Test 9: thread_id env var still wins when env var is set
    #[test]
    fn test9_thread_id_env_var_still_wins() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [77777]\n",
        );
        unsafe {
            std::env::set_var("TELEGRAM_HOME_CHANNEL", "env-tg");
            std::env::set_var("TELEGRAM_HOME_CHANNEL_THREAD_ID", "42");
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            clear_telegram_env();
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].chat_id, "env-tg", "env var chat_id must win");
        assert_eq!(targets[0].thread_id, Some("42".to_string()), "thread_id must be set from env");
    }

    // Test 10: config fallback yields no thread_id (config.yaml has no thread_id semantics)
    #[test]
    fn test10_config_fallback_yields_no_thread_id() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [12345]\n",
        );
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].chat_id, "12345");
        assert_eq!(
            targets[0].thread_id, None,
            "config fallback must not fabricate thread_id"
        );
    }

    // Test 11: explicit platform:chat_id bypasses both env and config fallback
    #[test]
    fn test11_explicit_chat_id_bypasses_config_fallback() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [88888]\n",
        );
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let job = make_job("telegram:caller-supplied", None);
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert_eq!(targets.len(), 1, "explicit chat_id form must produce one target");
        assert_eq!(
            targets[0].chat_id, "caller-supplied",
            "explicit chat_id must win over config whitelist"
        );
    }

    // Test 12: deliver=origin with origin set — origin wins over config fallback
    #[test]
    fn test12_origin_routing_wins_over_config_fallback() {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        write_config(
            &tmp,
            "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [44444]\n",
        );
        unsafe {
            clear_telegram_env();
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let origin = JobOrigin {
            platform: "telegram".to_string(),
            chat_id: "origin-1".to_string(),
            chat_name: None,
            thread_id: None,
        };
        let job = make_job("origin", Some(origin));
        let targets = resolve_delivery_targets(&job);
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].chat_id, "origin-1",
            "origin routing must win over config whitelist fallback"
        );
    }
}
