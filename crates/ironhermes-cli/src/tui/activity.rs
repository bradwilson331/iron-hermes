//! Shared activity state — published by agent callbacks, consumed by the render task.
//!
//! Per RESEARCH.md Pattern 3: use `tokio::sync::watch::channel::<ActivityState>(Idle)`
//! in Plan 21-02 so the render task always reads latest-wins without a mutex.
//!
//! Revision R1 (W6): Thinking variant dropped — agent callbacks (`with_streaming`,
//! `with_tool_progress`) fire at streaming-delta or tool-invocation boundaries only;
//! there is no observable "pre-stream latency" callback today. If such a callback
//! is added in a future phase, re-add Thinking then.

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ActivityState {
    #[default]
    Idle,
    Streaming,
    ToolCall {
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_default() {
        assert_eq!(ActivityState::default(), ActivityState::Idle);
    }

    #[test]
    fn tool_call_carries_name() {
        let s = ActivityState::ToolCall {
            name: "bash".to_string(),
        };
        let cloned = s.clone();
        assert_eq!(s, cloned);
    }

    #[test]
    fn variants_are_distinct() {
        assert_ne!(ActivityState::Idle, ActivityState::Streaming);
        assert_ne!(
            ActivityState::Streaming,
            ActivityState::ToolCall {
                name: "bash".to_string()
            }
        );
    }
}
