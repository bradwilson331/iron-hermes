use crate::event::HookEvent;
use crate::registry::HookListener;
use ironhermes_core::constants::get_hermes_home;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// Create a HookListener that appends JSONL-formatted events to a file.
///
/// - If `path` is None, defaults to `{hermes_home}/hooks/events.jsonl`.
/// - Parent directories are created on first call if they don't exist.
/// - Errors are logged via `tracing::warn!` — the listener never panics.
///
/// Security: Content previews are already truncated to 200 chars before reaching
/// the listener, so the log file never contains full message/tool content (T-06-01).
pub fn create_jsonl_listener(path: Option<PathBuf>) -> HookListener {
    let resolved_path = path.unwrap_or_else(|| {
        get_hermes_home().join("hooks").join("events.jsonl")
    });

    // Create parent directory eagerly so we surface errors at setup time,
    // not silently at first event.
    if let Some(parent) = resolved_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            path = %resolved_path.display(),
            error = %e,
            "Failed to create hooks directory for events.jsonl"
        );
    }

    Arc::new(move |event: HookEvent| {
        let line = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to serialize HookEvent to JSON");
                return;
            }
        };

        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved_path)
        {
            Ok(mut file) => {
                if let Err(e) = writeln!(file, "{}", line) {
                    tracing::warn!(
                        path = %resolved_path.display(),
                        error = %e,
                        "Failed to write event to events.jsonl"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    path = %resolved_path.display(),
                    error = %e,
                    "Failed to open events.jsonl for appending"
                );
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::HookEventKind;

    #[test]
    fn test_jsonl_listener_writes_event() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let log_path = tmp.path().join("events.jsonl");

        let listener = create_jsonl_listener(Some(log_path.clone()));

        let event = HookEvent::new(
            "req-abc",
            HookEventKind::ToolCalled {
                tool_name: "bash".to_string(),
                args_preview: "{\"cmd\": \"ls\"}".to_string(),
            },
        );
        listener(event);

        let content = std::fs::read_to_string(&log_path).expect("read events.jsonl");
        assert!(!content.is_empty(), "events.jsonl should not be empty");

        // Each line must be valid JSON
        for line in content.lines() {
            let parsed: serde_json::Value =
                serde_json::from_str(line).expect("each line must be valid JSON");
            assert!(
                parsed.get("request_id").is_some(),
                "request_id field must be present"
            );
            assert!(
                parsed.get("kind").is_some(),
                "kind field must be present"
            );
        }
    }

    #[test]
    fn test_jsonl_listener_appends_multiple_events() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let log_path = tmp.path().join("hooks").join("events.jsonl");

        let listener = create_jsonl_listener(Some(log_path.clone()));

        for i in 0..5 {
            let event = HookEvent::new(
                &format!("req-{i}"),
                HookEventKind::ResponseSent {
                    platform: "test".to_string(),
                    chat_id: "0".to_string(),
                    response_preview: format!("response {i}"),
                },
            );
            listener(event);
        }

        let content = std::fs::read_to_string(&log_path).expect("read file");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 5, "should have 5 lines");
    }

    #[test]
    fn test_jsonl_listener_does_not_panic_on_bad_path() {
        // Path in a directory we cannot create (non-existent parent with no write perms)
        // We just verify no panic occurs when directory can't be created.
        // Use a path that is clearly impossible on all platforms.
        let bad_path = PathBuf::from("/nonexistent_root_dir_xyz/hooks/events.jsonl");
        let listener = create_jsonl_listener(Some(bad_path));

        let event = HookEvent::new(
            "req-noop",
            HookEventKind::MessageReceived {
                platform: "test".to_string(),
                chat_id: "0".to_string(),
                content_preview: "hi".to_string(),
            },
        );
        // Must not panic
        listener(event);
    }
}
