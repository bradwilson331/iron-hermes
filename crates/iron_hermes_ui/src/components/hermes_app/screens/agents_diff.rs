//! Pure diff helper for the Agents screen — Phase 26.7.1 Plan 01 Task 2.
//!
//! Extracted from `agents.rs` so the diff logic can be unit-tested without
//! spinning up a Dioxus VirtualDom (agents.rs carries the `#[component]` and
//! `use_server_future` proc-macro expansion, which prevents plain unit tests).
//!
//! D-02: HOLD applies to ALL terminations uniformly — natural exit, kill,
//! interrupt. No per-termination-kind branching at the diff level.
//!
//! D-11: The returned `AgentInfo` is cloned from the `live_prev` slot so the
//! caller has the LAST-OBSERVED snapshot, including the uptime_secs value that
//! was frozen at termination time.

use std::collections::HashSet;

/// Returns every [`AgentInfo`] whose `id` is present in `live_prev` but
/// absent from `live_next`.
///
/// The returned `AgentInfo` is cloned from the `live_prev` slot so the caller
/// has the last-observed snapshot (D-11: uptime_secs frozen at termination
/// time). No per-kind branching — any id absent from the next list is
/// considered terminated (D-02).
///
/// # D-13 invariant
/// An id present in BOTH `live_prev` AND `live_next` is NOT returned, even if
/// its status changed. Re-running id wins: the caller must also remove any
/// existing `recently_terminated` entry for that id.
pub fn diff_terminations(
    live_prev: &[crate::server::api::AgentInfo],
    live_next: &[crate::server::api::AgentInfo],
) -> Vec<crate::server::api::AgentInfo> {
    let next_ids: HashSet<&str> = live_next.iter().map(|a| a.id.as_str()).collect();
    live_prev
        .iter()
        .filter(|a| !next_ids.contains(a.id.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to construct a minimal AgentInfo with only the fields that
    /// diff_terminations reads (just `id`). Other fields are set to
    /// representative defaults so the struct is complete.
    fn make_agent(id: &str, uptime_secs: u64) -> crate::server::api::AgentInfo {
        crate::server::api::AgentInfo {
            id: id.to_string(),
            task_summary: String::new(),
            uptime_secs,
            status: "running".to_string(),
            parent_id: None,
        }
    }

    /// Three agents in prev, one missing in next → that one is returned with
    /// its uptime_secs intact (D-11 snapshot check).
    #[test]
    fn test_diff_terminations_returns_absent_agents() {
        let prev = vec![
            make_agent("agent-a", 10),
            make_agent("agent-b", 42),
            make_agent("agent-c", 7),
        ];
        let next = vec![make_agent("agent-a", 11), make_agent("agent-c", 8)];

        let result = diff_terminations(&prev, &next);

        assert_eq!(result.len(), 1, "exactly one agent should be absent");
        assert_eq!(result[0].id, "agent-b");
        // D-11: uptime_secs is frozen at the prev snapshot value (42), not the
        // incremented value that the live list would show.
        assert_eq!(result[0].uptime_secs, 42, "uptime_secs must be frozen at prev snapshot");
    }

    /// Same set in prev and next → empty result.
    #[test]
    fn test_diff_terminations_empty_when_all_present() {
        let prev = vec![make_agent("agent-a", 5), make_agent("agent-b", 15)];
        let next = vec![make_agent("agent-a", 6), make_agent("agent-b", 16)];

        let result = diff_terminations(&prev, &next);

        assert!(result.is_empty(), "no terminations when all ids reappear");
    }

    /// Empty prev + non-empty next → empty result.
    #[test]
    fn test_diff_terminations_handles_empty_prev() {
        let prev: Vec<crate::server::api::AgentInfo> = vec![];
        let next = vec![make_agent("agent-a", 3)];

        let result = diff_terminations(&prev, &next);

        assert!(result.is_empty(), "empty prev should produce no terminations");
    }

    /// D-13 invariant — re-running id wins: an id present in BOTH prev AND
    /// next must NOT appear in the returned Vec even if it was previously
    /// observed. This is the critical regression guard for the
    /// "terminated-then-restarted-with-same-id" edge case.
    #[test]
    fn test_diff_terminations_rerunning_id_is_not_a_termination() {
        let prev = vec![
            make_agent("agent-rerun", 100),
            make_agent("agent-gone", 50),
        ];
        // agent-rerun reappears in next (e.g. restarted with same id).
        // agent-gone is absent.
        let next = vec![make_agent("agent-rerun", 101)];

        let result = diff_terminations(&prev, &next);

        // D-13: agent-rerun must NOT be in the result.
        let ids: Vec<&str> = result.iter().map(|a| a.id.as_str()).collect();
        assert!(
            !ids.contains(&"agent-rerun"),
            "D-13: re-running id must not appear in terminations list"
        );
        // agent-gone IS absent → it should appear.
        assert!(
            ids.contains(&"agent-gone"),
            "agent-gone (absent from next) must appear in terminations list"
        );
        assert_eq!(result.len(), 1, "exactly one termination");
    }
}
