//! Phase 36.2 Plan 06: per-provider in-memory rate-limit tracker.
//!
//! Tracker is a *pure observability* component — D-RL-03 forbids it from
//! calling into `fallback_providers` / `classify_llm_error`. It maintains
//! per-`(provider, api_key_hash, model)` state and emits structured
//! `RateLimitEvent`s on a `tokio::sync::broadcast` channel for downstream
//! subscribers (Plan 07 wires the AgentLoop subscriber).
//!
//! Two telemetry paths:
//!   1. Headers — `record_headers` parses provider-aware response headers:
//!        - "anthropic" → `anthropic-ratelimit-*` (RFC 3339 reset)
//!        - all other providers → `x-ratelimit-*` (float-seconds reset)
//!   2. Reactive 429 — `record_429` is the fallback for providers that don't
//!      expose headers (Ollama, custom endpoints).
//!
//! Security (T-36.2-06-LEAK):
//!   - `api_key_hash: [u8; 16]` is derived via SHA-256 truncated to 16 bytes
//!     (sha2 0.10.x, already in workspace deps). Cleartext keys never appear
//!     in any field, log line, or serialized form.
//!   - Manual `impl Debug` shows only the entry count — no map contents leak.
//!   - Static-grep invariants enforce these contracts at CI time.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::broadcast;

/// Three-axis tracking key (D-RL-02).
///
/// `api_key_hash` is SHA-256 truncated to 16 bytes (128 bits) — well above
/// birthday-collision risk for the small key set a single user holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateLimitKey {
    pub provider: String,
    pub api_key_hash: [u8; 16],
    pub model: String,
}

/// Stable hash of an API key (sha2 0.10.x — RESEARCH.md correction #5: blake3
/// is NOT in workspace deps, so SHA-256 is the canonical choice per D-RL-02
/// "Claude's Discretion").
pub fn hash_api_key(key: &str) -> [u8; 16] {
    let digest = Sha256::digest(key.as_bytes());
    digest[..16]
        .try_into()
        .expect("SHA-256 always produces 32 bytes; truncating to 16 is infallible")
}

/// Cached header-parsed state for a single tracking key.
#[derive(Debug, Clone)]
pub struct TrackerState {
    pub remaining_requests: Option<u32>,
    pub remaining_tokens: Option<u32>,
    pub limit_requests: Option<u32>,
    pub limit_tokens: Option<u32>,
    pub reset_at: Option<SystemTime>,
    pub captured_at: SystemTime,
    pub last_source: RateLimitSource,
}

/// Event emitted on the broadcast channel whenever state changes.
#[derive(Debug, Clone)]
pub struct RateLimitEvent {
    pub provider: String,
    pub key_hash: [u8; 16],
    pub model: String,
    pub remaining_requests: Option<u32>,
    pub remaining_tokens: Option<u32>,
    pub limit_requests: Option<u32>,
    pub limit_tokens: Option<u32>,
    pub reset_at: Option<SystemTime>,
    pub severity: RateLimitSeverity,
    pub source: RateLimitSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitSource {
    Headers,
    Reactive429,
}

/// Per-provider rate-limit tracker.
///
/// Cheap to clone — all clones share the same inner state via `Arc<Mutex<…>>`.
/// `subscribe()` returns additional `broadcast::Receiver`s for downstream
/// consumers; the channel buffer is fixed at 64.
#[derive(Clone)]
pub struct RateLimitTracker {
    inner: Arc<Mutex<HashMap<RateLimitKey, TrackerState>>>,
    tx: broadcast::Sender<RateLimitEvent>,
}

impl std::fmt::Debug for RateLimitTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // T-36.2-06-LEAK: never serialize map contents — only the entry count.
        let entries = self.inner.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("RateLimitTracker")
            .field("entries", &entries)
            .finish()
    }
}

impl RateLimitTracker {
    /// Construct a fresh tracker. Returns the tracker plus the first
    /// `broadcast::Receiver`; call `subscribe()` for additional receivers.
    pub fn new() -> (Self, broadcast::Receiver<RateLimitEvent>) {
        let (tx, rx) = broadcast::channel(64);
        (
            Self {
                inner: Arc::new(Mutex::new(HashMap::new())),
                tx,
            },
            rx,
        )
    }

    /// Subscribe to subsequent events. Existing buffered events may be
    /// delivered per the standard `tokio::sync::broadcast` semantics.
    pub fn subscribe(&self) -> broadcast::Receiver<RateLimitEvent> {
        self.tx.subscribe()
    }

    /// Parse response headers and update tracker state.
    ///
    /// Provider routing (D-RL-01, RESEARCH correction #1):
    ///   - `"anthropic"` → `parse_anthropic_headers` (RFC 3339 reset)
    ///   - anything else → `parse_x_ratelimit_headers` (float-seconds reset)
    ///
    /// If neither parser produces a state (no recognized headers present),
    /// the call is a silent no-op.
    pub fn record_headers<I, K, V>(
        &self,
        provider: &str,
        api_key_hash: [u8; 16],
        model: &str,
        headers: I,
    ) where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let lowered: HashMap<String, String> = headers
            .into_iter()
            .map(|(k, v)| (k.as_ref().to_ascii_lowercase(), v.as_ref().to_string()))
            .collect();
        let parsed = match provider {
            "anthropic" => parse_anthropic_headers(&lowered),
            _ => parse_x_ratelimit_headers(&lowered),
        };
        if let Some(state) = parsed {
            let severity = compute_severity(&state);
            let key = RateLimitKey {
                provider: provider.to_string(),
                api_key_hash,
                model: model.to_string(),
            };
            {
                let mut guard = self.inner.lock().expect("tracker mutex poisoned");
                guard.insert(key.clone(), state.clone());
            }
            // Send may fail when no subscribers are alive; that is fine —
            // observability events are best-effort.
            let _ = self.tx.send(RateLimitEvent {
                provider: provider.to_string(),
                key_hash: api_key_hash,
                model: model.to_string(),
                remaining_requests: state.remaining_requests,
                remaining_tokens: state.remaining_tokens,
                limit_requests: state.limit_requests,
                limit_tokens: state.limit_tokens,
                reset_at: state.reset_at,
                severity,
                source: RateLimitSource::Headers,
            });
        }
    }

    /// Record a reactive 429 event for providers without rate-limit headers.
    ///
    /// `retry_after` typically comes from `ProviderError::RateLimited` (Plan
    /// 03). When `None`, only `remaining_requests = 0` is recorded; severity
    /// is always `Critical` because the call did hit a rate limit.
    pub fn record_429(
        &self,
        provider: &str,
        api_key_hash: [u8; 16],
        model: &str,
        retry_after: Option<Duration>,
    ) {
        let reset_at = retry_after.map(|d| SystemTime::now() + d);
        let key = RateLimitKey {
            provider: provider.to_string(),
            api_key_hash,
            model: model.to_string(),
        };
        let state = TrackerState {
            remaining_requests: Some(0),
            remaining_tokens: None,
            limit_requests: None,
            limit_tokens: None,
            reset_at,
            captured_at: SystemTime::now(),
            last_source: RateLimitSource::Reactive429,
        };
        {
            let mut guard = self.inner.lock().expect("tracker mutex poisoned");
            guard.insert(key.clone(), state);
        }
        let _ = self.tx.send(RateLimitEvent {
            provider: provider.to_string(),
            key_hash: api_key_hash,
            model: model.to_string(),
            remaining_requests: Some(0),
            remaining_tokens: None,
            limit_requests: None,
            limit_tokens: None,
            reset_at,
            severity: RateLimitSeverity::Critical,
            source: RateLimitSource::Reactive429,
        });
    }

    /// Read-only snapshot of the tracker state for a key, if present.
    pub fn snapshot(&self, key: &RateLimitKey) -> Option<TrackerState> {
        self.inner.lock().ok()?.get(key).cloned()
    }

    /// Number of tracked keys. Cheap; used by tests + the `Debug` impl.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Parse Anthropic's `anthropic-ratelimit-*` headers (RFC 3339 reset).
///
/// Returns `None` if none of the recognized headers were present.
fn parse_anthropic_headers(lowered: &HashMap<String, String>) -> Option<TrackerState> {
    let remaining_requests = lowered
        .get("anthropic-ratelimit-requests-remaining")
        .and_then(|v| v.parse().ok());
    let remaining_tokens = lowered
        .get("anthropic-ratelimit-tokens-remaining")
        .and_then(|v| v.parse().ok());
    let limit_requests = lowered
        .get("anthropic-ratelimit-requests-limit")
        .and_then(|v| v.parse().ok());
    let limit_tokens = lowered
        .get("anthropic-ratelimit-tokens-limit")
        .and_then(|v| v.parse().ok());
    let reset_at = lowered
        .get("anthropic-ratelimit-requests-reset")
        .or_else(|| lowered.get("anthropic-ratelimit-tokens-reset"))
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| SystemTime::UNIX_EPOCH + Duration::from_secs(dt.timestamp().max(0) as u64));
    if remaining_requests.is_none()
        && remaining_tokens.is_none()
        && limit_requests.is_none()
        && limit_tokens.is_none()
    {
        return None;
    }
    Some(TrackerState {
        remaining_requests,
        remaining_tokens,
        limit_requests,
        limit_tokens,
        reset_at,
        captured_at: SystemTime::now(),
        last_source: RateLimitSource::Headers,
    })
}

/// Parse OpenRouter / Nous / OpenAI-style `x-ratelimit-*` headers
/// (reset values are float seconds from "now").
fn parse_x_ratelimit_headers(lowered: &HashMap<String, String>) -> Option<TrackerState> {
    let remaining_requests = lowered
        .get("x-ratelimit-remaining-requests")
        .and_then(|v| v.parse().ok());
    let remaining_tokens = lowered
        .get("x-ratelimit-remaining-tokens")
        .and_then(|v| v.parse().ok());
    let limit_requests = lowered
        .get("x-ratelimit-limit-requests")
        .and_then(|v| v.parse().ok());
    let limit_tokens = lowered
        .get("x-ratelimit-limit-tokens")
        .and_then(|v| v.parse().ok());
    let reset_at = lowered
        .get("x-ratelimit-reset-requests")
        .or_else(|| lowered.get("x-ratelimit-reset-tokens"))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| SystemTime::now() + Duration::from_secs_f64(secs.max(0.0)));
    if remaining_requests.is_none()
        && remaining_tokens.is_none()
        && limit_requests.is_none()
        && limit_tokens.is_none()
    {
        return None;
    }
    Some(TrackerState {
        remaining_requests,
        remaining_tokens,
        limit_requests,
        limit_tokens,
        reset_at,
        captured_at: SystemTime::now(),
        last_source: RateLimitSource::Headers,
    })
}

/// Compute severity from current state (nous_rate_guard.py port).
///
/// Rules:
///   - `Critical` if any bucket has `remaining = 0` AND `reset_at >= now + 60s`
///     (genuine quota — transient `0` jitter is reported as `Warning` instead).
///   - `Warning` if any bucket has `remaining < 10%` of `limit`.
///   - `Info` otherwise.
fn compute_severity(state: &TrackerState) -> RateLimitSeverity {
    let buckets = [
        (state.remaining_requests, state.limit_requests),
        (state.remaining_tokens, state.limit_tokens),
    ];
    let mut severity = RateLimitSeverity::Info;
    for (remaining, limit) in buckets {
        if let (Some(rem), Some(lim)) = (remaining, limit) {
            if lim == 0 {
                continue;
            }
            if rem == 0 {
                let is_genuine = match state.reset_at {
                    Some(reset) => reset
                        .duration_since(SystemTime::now())
                        .map(|d| d.as_secs() >= 60)
                        .unwrap_or(false),
                    None => true,
                };
                if is_genuine {
                    return RateLimitSeverity::Critical;
                } else if severity != RateLimitSeverity::Critical {
                    severity = RateLimitSeverity::Warning;
                }
            } else {
                let pct = (rem as f64 / lim as f64) * 100.0;
                if pct < 10.0 && severity != RateLimitSeverity::Critical {
                    severity = RateLimitSeverity::Warning;
                }
            }
        }
    }
    severity
}
