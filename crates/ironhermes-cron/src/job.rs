use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

// ---------------------------------------------------------------------------
// ScheduleParsed
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleParsed {
    Once {
        run_at: DateTime<Utc>,
        display: String,
    },
    Interval {
        minutes: u32,
        display: String,
    },
    Cron {
        expr: String,
        display: String,
    },
}

// ---------------------------------------------------------------------------
// JobState
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    #[default]
    Scheduled,
    Paused,
    Completed,
}

pub fn default_job_state() -> JobState {
    JobState::Scheduled
}

// ---------------------------------------------------------------------------
// RepeatConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepeatConfig {
    pub times: Option<u32>, // None = forever
    pub completed: u32,
}

// ---------------------------------------------------------------------------
// JobOrigin
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JobOrigin {
    pub platform: String,
    pub chat_id: String,
    pub chat_name: Option<String>,
    pub thread_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Defensive origin deserializer
// ---------------------------------------------------------------------------

/// Deserializes `Option<JobOrigin>` leniently: a non-object value (e.g., a
/// bare string like `"telegram"` from hand-edited jobs.json) produces `None`
/// instead of a serde error. A warn! is emitted so operators see the coercion.
fn deserialize_origin_lenient<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<JobOrigin>, D::Error> {
    let value: serde_json::Value = Deserialize::deserialize(d)?;
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(_) => {
            serde_json::from_value::<JobOrigin>(value)
                .map(Some)
                .or(Ok(None))
        }
        other => {
            tracing::warn!(
                origin = ?other,
                "CronJob origin field is not an object — treating as None"
            );
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// CronJob
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub skills: Vec<String>,
    pub schedule: ScheduleParsed,
    pub schedule_display: String,
    #[serde(default)]
    pub repeat: RepeatConfig,
    pub enabled: bool,
    #[serde(default = "default_job_state")]
    pub state: JobState,
    #[serde(default)]
    pub paused_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub paused_reason: Option<String>,
    pub deliver: String,
    #[serde(deserialize_with = "deserialize_origin_lenient", default)]
    pub origin: Option<JobOrigin>,
    pub created_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    // Per-job runtime overrides — all #[serde(default)] so existing jobs.json records load
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub no_agent: bool,
    #[serde(default)]
    pub context_from: Option<Vec<String>>,
    #[serde(default)]
    pub enabled_toolsets: Option<Vec<String>>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub last_delivery_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cronjob_serde_tests {
    use super::*;

    /// Minimal valid CronJob JSON payload (omits all nine new fields).
    fn minimal_job_json() -> &'static str {
        r#"{
            "id": "test-id-1",
            "name": "Test Job",
            "prompt": "do something",
            "skills": [],
            "schedule": { "kind": "interval", "minutes": 60, "display": "every 60m" },
            "schedule_display": "every 60m",
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
        }"#
    }

    /// Test 1: CronJob with all nine new fields round-trips through serde_json.
    #[test]
    fn test1_roundtrip_all_nine_new_fields() {
        let json = r#"{
            "id": "rt-id",
            "name": "Roundtrip Job",
            "prompt": "hello",
            "skills": [],
            "schedule": { "kind": "interval", "minutes": 30, "display": "every 30m" },
            "schedule_display": "every 30m",
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
            "last_error": null,
            "model": "claude-3-opus",
            "provider": "anthropic",
            "base_url": "https://api.anthropic.com",
            "script": "check.sh",
            "no_agent": true,
            "context_from": ["job-a", "job-b"],
            "enabled_toolsets": ["web", "code"],
            "workdir": "/home/user/projects",
            "last_delivery_error": "timeout"
        }"#;

        let job: CronJob = serde_json::from_str(json).expect("deserialize");
        assert_eq!(job.model.as_deref(), Some("claude-3-opus"));
        assert_eq!(job.provider.as_deref(), Some("anthropic"));
        assert_eq!(job.base_url.as_deref(), Some("https://api.anthropic.com"));
        assert_eq!(job.script.as_deref(), Some("check.sh"));
        assert!(job.no_agent);
        assert_eq!(
            job.context_from.as_deref(),
            Some(["job-a".to_string(), "job-b".to_string()].as_slice())
        );
        assert_eq!(
            job.enabled_toolsets.as_deref(),
            Some(["web".to_string(), "code".to_string()].as_slice())
        );
        assert_eq!(job.workdir.as_deref(), Some("/home/user/projects"));
        assert_eq!(job.last_delivery_error.as_deref(), Some("timeout"));

        // Round-trip: serialize then deserialize back
        let serialized = serde_json::to_string(&job).expect("serialize");
        let job2: CronJob = serde_json::from_str(&serialized).expect("re-deserialize");
        assert_eq!(job2.model, job.model);
        assert_eq!(job2.provider, job.provider);
        assert_eq!(job2.base_url, job.base_url);
        assert_eq!(job2.script, job.script);
        assert_eq!(job2.no_agent, job.no_agent);
        assert_eq!(job2.context_from, job.context_from);
        assert_eq!(job2.enabled_toolsets, job.enabled_toolsets);
        assert_eq!(job2.workdir, job.workdir);
        assert_eq!(job2.last_delivery_error, job.last_delivery_error);
    }

    /// Test 2: A jobs.json payload omitting all nine new fields deserializes with defaults.
    #[test]
    fn test2_omitted_new_fields_default_to_none_and_false() {
        let job: CronJob = serde_json::from_str(minimal_job_json()).expect("deserialize");
        assert_eq!(job.model, None);
        assert_eq!(job.provider, None);
        assert_eq!(job.base_url, None);
        assert_eq!(job.script, None);
        assert!(!job.no_agent);
        assert_eq!(job.context_from, None);
        assert_eq!(job.enabled_toolsets, None);
        assert_eq!(job.workdir, None);
        assert_eq!(job.last_delivery_error, None);
    }

    /// Test 3: A jobs.json whose `origin` is a bare string deserializes to origin=None.
    /// The job MUST still load — serde MUST NOT return an error.
    #[test]
    fn test3_string_origin_deserializes_to_none() {
        let json = r#"{
            "id": "bad-origin",
            "name": "Bad Origin Job",
            "prompt": "do something",
            "skills": [],
            "schedule": { "kind": "interval", "minutes": 60, "display": "every 60m" },
            "schedule_display": "every 60m",
            "repeat": { "times": null, "completed": 0 },
            "enabled": true,
            "state": "scheduled",
            "paused_at": null,
            "paused_reason": null,
            "deliver": "local",
            "origin": "telegram",
            "created_at": "2026-01-01T00:00:00Z",
            "next_run_at": null,
            "last_run_at": null,
            "last_status": null,
            "last_error": null
        }"#;

        let result: Result<CronJob, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "Deserialization MUST succeed even with string origin");
        let job = result.unwrap();
        assert_eq!(job.origin, None, "String origin must produce origin=None");
    }

    /// Test 4: A valid object `origin` still deserializes into Some(JobOrigin{..}).
    #[test]
    fn test4_valid_object_origin_deserializes_correctly() {
        let json = r#"{
            "id": "good-origin",
            "name": "Good Origin Job",
            "prompt": "do something",
            "skills": [],
            "schedule": { "kind": "interval", "minutes": 60, "display": "every 60m" },
            "schedule_display": "every 60m",
            "repeat": { "times": null, "completed": 0 },
            "enabled": true,
            "state": "scheduled",
            "paused_at": null,
            "paused_reason": null,
            "deliver": "origin",
            "origin": {
                "platform": "telegram",
                "chat_id": "99999",
                "chat_name": "My Chat",
                "thread_id": "42"
            },
            "created_at": "2026-01-01T00:00:00Z",
            "next_run_at": null,
            "last_run_at": null,
            "last_status": null,
            "last_error": null
        }"#;

        let job: CronJob = serde_json::from_str(json).expect("deserialize");
        let origin = job.origin.expect("origin should be Some");
        assert_eq!(origin.platform, "telegram");
        assert_eq!(origin.chat_id, "99999");
        assert_eq!(origin.chat_name.as_deref(), Some("My Chat"));
        assert_eq!(origin.thread_id.as_deref(), Some("42"));
    }

    /// Test 5: `origin` = null or missing → origin = None (existing behavior preserved).
    #[test]
    fn test5_null_or_missing_origin_is_none() {
        // null origin
        let job_null: CronJob = serde_json::from_str(minimal_job_json()).expect("null origin");
        assert_eq!(job_null.origin, None);

        // missing origin field entirely
        let json_missing = r#"{
            "id": "no-origin",
            "name": "No Origin",
            "prompt": "do something",
            "skills": [],
            "schedule": { "kind": "interval", "minutes": 60, "display": "every 60m" },
            "schedule_display": "every 60m",
            "repeat": { "times": null, "completed": 0 },
            "enabled": true,
            "state": "scheduled",
            "paused_at": null,
            "paused_reason": null,
            "deliver": "local",
            "created_at": "2026-01-01T00:00:00Z",
            "next_run_at": null,
            "last_run_at": null,
            "last_status": null,
            "last_error": null
        }"#;
        let job_missing: CronJob = serde_json::from_str(json_missing).expect("missing origin");
        assert_eq!(job_missing.origin, None);
    }

    /// Test 6: JobOrigin missing optional fields deserializes with chat_name=None, thread_id=None.
    #[test]
    fn test6_job_origin_optional_fields_are_none_when_absent() {
        let json = r#"{
            "platform": "telegram",
            "chat_id": "12345"
        }"#;
        let origin: JobOrigin = serde_json::from_str(json).expect("deserialize");
        assert_eq!(origin.platform, "telegram");
        assert_eq!(origin.chat_id, "12345");
        assert_eq!(origin.chat_name, None);
        assert_eq!(origin.thread_id, None);
    }
}
