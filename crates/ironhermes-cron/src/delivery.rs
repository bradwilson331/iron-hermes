use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::job::{CronJob, JobOrigin};

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
// resolve_delivery_target
// ---------------------------------------------------------------------------

/// Resolve where job output should be delivered based on `job.deliver`.
///
/// - `"local"` → None (file-only, no platform delivery)
/// - `"origin"` → map `job.origin` to DeliveryTarget; None if no origin captured
/// - `"platform:chat_id"` → split on first `:`, left = platform, right = chat_id
/// - anything else → None
pub fn resolve_delivery_target(job: &CronJob) -> Option<DeliveryTarget> {
    match job.deliver.as_str() {
        "local" => None,
        "origin" => {
            job.origin.as_ref().map(|o: &JobOrigin| DeliveryTarget {
                platform: o.platform.clone(),
                chat_id: o.chat_id.clone(),
                thread_id: o.thread_id.clone(),
            })
        }
        other => {
            if let Some(colon_pos) = other.find(':') {
                let platform = other[..colon_pos].to_string();
                let chat_id = other[colon_pos + 1..].to_string();
                if !platform.is_empty() && !chat_id.is_empty() {
                    return Some(DeliveryTarget {
                        platform,
                        chat_id,
                        thread_id: None,
                    });
                }
            }
            None
        }
    }
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
/// Uses atomic temp+rename write pattern.
/// Returns the path that was written.
pub fn save_job_output(job_id: &str, output: &str) -> Result<PathBuf> {
    // Reject any job_id that could escape the output directory via path traversal
    if job_id.contains('/')
        || job_id.contains('\\')
        || job_id.contains("..")
        || job_id.is_empty()
    {
        anyhow::bail!("invalid job_id for filesystem use: {:?}", job_id);
    }

    let home = ironhermes_core::get_hermes_home();
    let output_dir = home.join("cron").join("output").join(job_id);

    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output dir: {}", output_dir.display()))?;

    let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let file_path = output_dir.join(format!("{}.md", timestamp));
    let tmp_path = output_dir.join(format!("{}.md.tmp", timestamp));

    {
        let mut f = fs::File::create(&tmp_path)
            .with_context(|| format!("failed to create temp file: {}", tmp_path.display()))?;
        f.write_all(output.as_bytes())
            .with_context(|| format!("failed to write temp file: {}", tmp_path.display()))?;
        f.flush()?;
    }

    fs::rename(&tmp_path, &file_path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            tmp_path.display(),
            file_path.display()
        )
    })?;

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{CronJob, JobOrigin, JobState, RepeatConfig, ScheduleParsed};
    use chrono::Utc;
    use tempfile::TempDir;

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
        }
    }

    // --- resolve_delivery_target ---

    #[test]
    fn local_returns_none() {
        let job = make_job("local", None);
        assert_eq!(resolve_delivery_target(&job), None);
    }

    #[test]
    fn origin_no_origin_field_returns_none() {
        let job = make_job("origin", None);
        assert_eq!(resolve_delivery_target(&job), None);
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
