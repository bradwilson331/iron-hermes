//! D-05: benign handling of unknown-method probes (`ping`/`health`/`healthcheck`, and any
//! other unrecognized JSON-RPC request).
//!
//! Registered as the catch-all fallback arm of the SDK connection in `entry.rs` (registered
//! LAST, after every typed `initialize`/`authenticate`/`session/*` handler, so it only ever
//! sees methods none of them claimed). An unrecognized *request* gets a `-32601` error
//! response and the server keeps serving; an unrecognized *notification* (no `id`) gets no
//! response at all — responding to a notification would itself desync a hand-rolled client
//! per JSON-RPC 2.0. Behavioral reference: the Python `hermes-agent/acp_adapter/entry.py`
//! adapter this crate ports (external repo, no in-repo analog — see PATTERNS.md).

/// Local alias for the JSON-RPC error payload this module produces, so the intent reads
/// clearly at call sites without exposing SDK internals by name.
pub type JsonRpcErrorPayload = agent_client_protocol::Error;

/// Well-known probe method names a monitoring tool or naive client is likely to send before
/// (or instead of) a real `initialize` handshake. Not a filter — `probe_reply` replies
/// `-32601` for ANY unrecognized method, probe-listed or not; this list exists purely to
/// name the methods this behavior was specifically written to tolerate.
pub const PROBE_METHODS: &[&str] = &["ping", "health", "healthcheck"];

/// Build the JSON-RPC `-32601` ("Method not found") reply for any unrecognized request
/// method — `PROBE_METHODS`-listed or not. Always returns `Some`; the `Option` return type
/// exists to keep this a drop-in fallback classifier if a future revision needs a case that
/// intentionally produces no reply (mirroring the notification's "no response at all" rule
/// handled separately in `entry.rs`).
#[must_use]
pub fn probe_reply(method: &str) -> Option<JsonRpcErrorPayload> {
    let _ = method;
    Some(agent_client_protocol::Error::method_not_found())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_reply_is_method_not_found_for_listed_and_unlisted_methods() {
        for method in PROBE_METHODS {
            let error = probe_reply(method).expect("probe_reply always returns Some");
            assert_eq!(i32::from(error.code), -32601);
        }
        let error =
            probe_reply("totally/unknown/method").expect("probe_reply always returns Some");
        assert_eq!(i32::from(error.code), -32601);
    }
}
