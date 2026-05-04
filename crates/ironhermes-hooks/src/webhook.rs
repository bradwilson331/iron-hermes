//! Webhook delivery with authentication, HMAC signing, retry, and event filtering.
//!
//! `WebhookDelivery` sends `HookEvent` payloads to configured HTTP endpoints as JSON
//! POST requests. Delivery is fire-and-forget via `create_webhook_listener()`.
//!
//! Failed deliveries are retried with exponential backoff (1s, 5s, 25s, 2m, 10m).
//! After all retries are exhausted, the entry is persisted to disk via `RetryQueue`
//! so it survives process restarts (per D-09).

use std::sync::Arc;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::config::WebhookEndpointConfig;
use crate::event::{HookEvent, HookEventKind};
use crate::retry_queue::{RetryEntry, RetryQueue};

type HmacSha256 = Hmac<Sha256>;

/// Exponential backoff delays for webhook retry (per D-09).
/// 1s, 5s, 25s, 2min, 10min
const RETRY_DELAYS_SECS: &[u64] = &[1, 5, 25, 120, 600];

/// Delivers webhook events to a single configured endpoint.
pub struct WebhookDelivery {
    client: reqwest::Client,
    endpoint: WebhookEndpointConfig,
    retry_queue: Arc<RetryQueue>,
}

impl WebhookDelivery {
    /// Create a new WebhookDelivery with a 10s connect timeout.
    pub fn new(endpoint: WebhookEndpointConfig, retry_queue: Arc<RetryQueue>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            client,
            endpoint,
            retry_queue,
        }
    }

    /// Check whether this endpoint should receive the given event.
    /// If `self.endpoint.events` is empty, all events match (per D-10).
    pub fn matches_event(&self, event: &HookEvent) -> bool {
        if self.endpoint.events.is_empty() {
            return true;
        }
        let name = event_kind_name(event);
        self.endpoint.events.iter().any(|e| e == name)
    }

    /// Attempt a single delivery of the event. Returns Ok(()) on 2xx, Err on
    /// non-2xx or network error.
    pub async fn deliver(&self, event: &HookEvent) -> anyhow::Result<()> {
        let body = serde_json::to_string(event)?;

        let mut req = self
            .client
            .post(&self.endpoint.url)
            .header("Content-Type", "application/json")
            .body(body.clone());

        // Authorization header (per D-08)
        if let Some(ref auth) = self.endpoint.auth_header {
            req = req.header("Authorization", auth);
        }

        // HMAC-SHA256 signing (per D-08)
        if let Some(ref secret) = self.endpoint.hmac_secret {
            let signature = compute_hmac_sha256(secret, body.as_bytes());
            req = req.header("X-Signature", format!("sha256={}", signature));
        }

        let response = req.send().await?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            anyhow::bail!("Webhook endpoint returned non-2xx status: {}", status)
        }
    }

    /// Attempt delivery up to `max_retries` times with exponential backoff.
    /// After all retries are exhausted, the failed delivery is persisted to disk
    /// via `RetryQueue` for retry on next startup (per D-09).
    /// This method never returns an error — it is fire-and-forget.
    pub async fn deliver_with_retry(&self, event: &HookEvent) {
        let max_retries = self.endpoint.max_retries.unwrap_or(5) as usize;

        for attempt in 0..=max_retries {
            match self.deliver(event).await {
                Ok(()) => {
                    tracing::debug!(
                        url = %self.endpoint.url,
                        event_id = %event.id,
                        attempt = attempt,
                        "Webhook delivered successfully"
                    );
                    return;
                }
                Err(e) => {
                    if attempt < max_retries {
                        let delay_secs = RETRY_DELAYS_SECS.get(attempt).copied().unwrap_or(600);
                        tracing::warn!(
                            url = %self.endpoint.url,
                            event_id = %event.id,
                            attempt = attempt + 1,
                            max = max_retries,
                            delay_secs = delay_secs,
                            error = %e,
                            "Webhook delivery failed, retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                    } else {
                        // All retries exhausted — persist to disk (per D-09)
                        let entry = RetryEntry {
                            endpoint_url: self.endpoint.url.clone(),
                            event: event.clone(),
                            queued_at: chrono::Utc::now(),
                            attempts: max_retries as u32,
                        };
                        if let Err(e) = self.retry_queue.enqueue(&entry) {
                            tracing::warn!("Failed to persist retry entry to disk: {}", e);
                        }
                        tracing::warn!(
                            url = %self.endpoint.url,
                            event_id = %event.id,
                            "Webhook delivery failed after {} retries, queued to disk for retry on next startup",
                            max_retries
                        );
                    }
                }
            }
        }
    }
}

/// Return the snake_case event kind name for filtering.
fn event_kind_name(event: &HookEvent) -> &str {
    match &event.kind {
        HookEventKind::MessageReceived { .. } => "message_received",
        HookEventKind::ToolCalled { .. } => "tool_called",
        HookEventKind::ToolCompleted { .. } => "tool_completed",
        HookEventKind::ResponseSent { .. } => "response_sent",
        HookEventKind::SkillActivated { .. } => "skill_activated",
        HookEventKind::ContextPreCompress { .. } => "context_pre_compress",
        HookEventKind::ContextPressure { .. } => "context_pressure",
    }
}

/// Compute HMAC-SHA256 of `body` using `secret` and return a lowercase hex string.
fn compute_hmac_sha256(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take any key size");
    mac.update(body);
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// Create a `HookListener` that delivers events to a webhook endpoint.
/// Uses `tokio::spawn` internally for fire-and-forget delivery with retry.
/// Failed deliveries that exhaust retries are persisted to the shared RetryQueue.
///
/// Returns a no-op listener if the URL fails SSRF validation.
pub fn create_webhook_listener(
    endpoint: WebhookEndpointConfig,
    retry_queue: Arc<RetryQueue>,
) -> crate::HookListener {
    // SSRF protection (T-06-07): validate URL before creating delivery client
    if !ironhermes_core::ssrf::is_safe_url(&endpoint.url) {
        tracing::warn!(
            url = %endpoint.url,
            "Webhook endpoint URL failed SSRF validation — using no-op listener"
        );
        return Arc::new(|_event: HookEvent| {});
    }

    let delivery = Arc::new(WebhookDelivery::new(endpoint, retry_queue));
    Arc::new(move |event: HookEvent| {
        let d = delivery.clone();
        tokio::spawn(async move {
            if d.matches_event(&event) {
                d.deliver_with_retry(&event).await;
            }
        });
    })
}

/// Drain the persistent retry queue and re-attempt delivery for all valid entries.
/// Called once at startup. Entries that fail again are re-enqueued.
/// Entries older than `ttl_hours` are discarded (per D-09).
pub async fn drain_retry_queue(
    retry_queue: Arc<RetryQueue>,
    endpoints: &[WebhookEndpointConfig],
    ttl_hours: u32,
) {
    let entries = retry_queue.drain(ttl_hours);
    if entries.is_empty() {
        return;
    }
    tracing::info!(
        "Draining {} entries from webhook retry queue",
        entries.len()
    );

    for entry in entries {
        // Find the matching endpoint config by URL
        let endpoint = endpoints.iter().find(|e| e.url == entry.endpoint_url);
        let Some(endpoint) = endpoint else {
            tracing::debug!(
                url = %entry.endpoint_url,
                "Skipping retry queue entry: endpoint no longer configured"
            );
            continue;
        };

        let delivery = WebhookDelivery::new(endpoint.clone(), retry_queue.clone());
        // Attempt a single delivery (not full retry cycle — if it fails again it re-enqueues)
        if delivery.deliver(&entry.event).await.is_err() {
            // Re-enqueue with incremented attempt count
            let re_entry = RetryEntry {
                endpoint_url: entry.endpoint_url,
                event: entry.event,
                queued_at: entry.queued_at, // preserve original queue time for TTL
                attempts: entry.attempts + 1,
            };
            if let Err(e) = retry_queue.enqueue(&re_entry) {
                tracing::warn!("Failed to re-enqueue retry entry: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::HookEventKind;

    fn make_event(kind: HookEventKind) -> HookEvent {
        HookEvent::new("req-test", kind)
    }

    fn make_queue() -> Arc<RetryQueue> {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("retry-queue.jsonl");
        // Keep tempdir alive by leaking it for test duration
        let path = std::mem::ManuallyDrop::new(tmp)
            .path()
            .join("retry-queue.jsonl");
        Arc::new(RetryQueue::new(path).expect("queue"))
    }

    #[test]
    fn test_matches_event_empty_filter_matches_all() {
        let endpoint = WebhookEndpointConfig {
            url: "https://example.com/hook".to_string(),
            events: vec![], // empty = match all
            ..Default::default()
        };
        let queue = {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("q.jsonl");
            std::mem::forget(tmp);
            Arc::new(RetryQueue::new(path).expect("queue"))
        };
        let delivery = WebhookDelivery::new(endpoint, queue);

        assert!(
            delivery.matches_event(&make_event(HookEventKind::MessageReceived {
                platform: "telegram".to_string(),
                chat_id: "1".to_string(),
                content_preview: "hi".to_string(),
            }))
        );
        assert!(
            delivery.matches_event(&make_event(HookEventKind::ToolCalled {
                tool_name: "bash".to_string(),
                args_preview: "{}".to_string(),
            }))
        );
    }

    #[test]
    fn test_matches_event_specific_filter() {
        let endpoint = WebhookEndpointConfig {
            url: "https://example.com/hook".to_string(),
            events: vec!["tool_called".to_string()],
            ..Default::default()
        };
        let queue = {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("q.jsonl");
            std::mem::forget(tmp);
            Arc::new(RetryQueue::new(path).expect("queue"))
        };
        let delivery = WebhookDelivery::new(endpoint, queue);

        // Should match tool_called
        assert!(
            delivery.matches_event(&make_event(HookEventKind::ToolCalled {
                tool_name: "bash".to_string(),
                args_preview: "{}".to_string(),
            }))
        );

        // Should NOT match message_received
        assert!(
            !delivery.matches_event(&make_event(HookEventKind::MessageReceived {
                platform: "telegram".to_string(),
                chat_id: "1".to_string(),
                content_preview: "hi".to_string(),
            }))
        );
    }

    #[test]
    fn test_event_kind_name_variants() {
        assert_eq!(
            event_kind_name(&make_event(HookEventKind::MessageReceived {
                platform: "t".to_string(),
                chat_id: "1".to_string(),
                content_preview: "".to_string(),
            })),
            "message_received"
        );
        assert_eq!(
            event_kind_name(&make_event(HookEventKind::ToolCalled {
                tool_name: "t".to_string(),
                args_preview: "".to_string(),
            })),
            "tool_called"
        );
        assert_eq!(
            event_kind_name(&make_event(HookEventKind::ToolCompleted {
                tool_name: "t".to_string(),
                success: true,
                result_preview: "".to_string(),
                duration_ms: 0,
            })),
            "tool_completed"
        );
        assert_eq!(
            event_kind_name(&make_event(HookEventKind::ResponseSent {
                platform: "t".to_string(),
                chat_id: "1".to_string(),
                response_preview: "".to_string(),
            })),
            "response_sent"
        );
    }

    #[test]
    fn test_hmac_signature_computation() {
        let secret = "my-secret-key";
        let body = b"hello world";
        let sig = compute_hmac_sha256(secret, body);

        // Verify it's a 64-char hex string (256-bit HMAC = 32 bytes = 64 hex chars)
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));

        // Verify deterministic
        let sig2 = compute_hmac_sha256(secret, body);
        assert_eq!(sig, sig2);

        // Different key → different signature
        let sig3 = compute_hmac_sha256("other-key", body);
        assert_ne!(sig, sig3);
    }

    #[tokio::test]
    async fn test_deliver_failure_does_not_panic() {
        // TEST-NET address (192.0.2.1) — won't connect
        let endpoint = WebhookEndpointConfig {
            url: "http://192.0.2.1:1/hook".to_string(),
            ..Default::default()
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("q.jsonl");
        let queue = Arc::new(RetryQueue::new(path).expect("queue"));
        let delivery = WebhookDelivery::new(endpoint, queue);

        let event = make_event(HookEventKind::MessageReceived {
            platform: "telegram".to_string(),
            chat_id: "1".to_string(),
            content_preview: "test".to_string(),
        });

        // Should return Err without panicking
        let result = delivery.deliver(&event).await;
        assert!(result.is_err(), "Expected error for unreachable endpoint");
    }
}
