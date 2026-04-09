use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Scheduled,
    Paused,
    Completed,
}

impl Default for JobState {
    fn default() -> Self {
        JobState::Scheduled
    }
}

pub fn default_job_state() -> JobState {
    JobState::Scheduled
}

// ---------------------------------------------------------------------------
// RepeatConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatConfig {
    pub times: Option<u32>, // None = forever
    pub completed: u32,
}

impl Default for RepeatConfig {
    fn default() -> Self {
        RepeatConfig {
            times: None,
            completed: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// JobOrigin
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOrigin {
    pub platform: String,
    pub chat_id: String,
    pub chat_name: Option<String>,
    pub thread_id: Option<String>,
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
    #[serde(default)]
    pub origin: Option<JobOrigin>,
    pub created_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}
