//! Phase 54.9 Plan 10 (D-09, T-54.9-27, T-54.9-28): the opt-in Thesys hosted Gateway.
//!
//! Off by default and reachable only through `ChatRequest.openui_gateway` (set by the OpenUI
//! screen's toggle, which itself refuses to turn on without a stored key). With the flag off no
//! code in this module runs, so nothing ever contacts `api.thesys.dev`.
//!
//! The key is resolved SERVER-side only (vault secret under provider name `thesys`, else env
//! `THESYS_API_KEY`); [`openui_gateway_status`] returns presence and nothing else, and no event
//! or error string produced here contains the key or the request body.
//!
//! Endpoint, auth header and stream format are the ones ADR-0007 "Gateway" recorded from the
//! vendor docs (OpenAI-compatible Chat Completions, `Authorization: Bearer`, SSE). The model name
//! and whether the Gateway accepts the locally generated prompt as the system message were NOT
//! verified with a live call (ADR-0007 "Changed: the prompt question"); both are one constant /
//! one message in [`stream_gateway_generation`].

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// What the screen may learn about the key: whether one is stored. Never the value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OpenUiGatewayStatus {
    pub key_present: bool,
    /// WR-04: `ui.openui_gateway_enabled` in the root config (default false).
    #[serde(default)]
    pub enabled: bool,
}

/// Whether a Thesys key is stored (vault secret `thesys`, else env `THESYS_API_KEY`).
#[server]
pub async fn openui_gateway_status() -> Result<OpenUiGatewayStatus, ServerFnError> {
    Ok(OpenUiGatewayStatus {
        key_present: gw::resolve_thesys_key().await.is_some(),
        enabled: gw::gateway_enabled(),
    })
}

#[cfg(feature = "server")]
pub(crate) use gw::*;

#[cfg(feature = "server")]
mod gw {
    use crate::protocol::ChatStreamEvent;
    use secrecy::{ExposeSecret, SecretString};
    use tokio::sync::mpsc;

    /// ADR-0007 "Gateway": the OpenAI-compatible base; chat completions live under it.
    pub(crate) const GATEWAY_BASE_URL: &str = "https://api.thesys.dev/v1/embed";
    /// ADR-0007 "Gateway": models are `{provider}/{model}`. shortcut: a fixed model named in the
    /// vendor docs example, not operator-configurable and not verified live; make it a config key
    /// if the operator ever needs another.
    const GATEWAY_MODEL: &str = "openai/gpt-5";
    /// The vault provider name the key is stored under (D-09).
    const GATEWAY_KEY_PROVIDER: &str = "thesys";
    /// The copy shown when the Gateway is requested with no stored key (UI-SPEC Part B).
    pub(crate) const GATEWAY_MISSING_KEY: &str =
        "No Thesys API key in the vault. Store THESYS_API_KEY first.";
    /// A line longer than this without a newline is not an SSE stream we understand.
    const MAX_LINE_BYTES: usize = 1024 * 1024;

    /// One parsed server-sent event of the Gateway stream.
    #[derive(Debug, PartialEq)]
    pub(crate) enum GatewayChunk {
        Delta(String),
        Done,
        /// An in-stream error event; carries a short reason (the error code), never the body.
        Error(String),
    }

    /// Token usage the Gateway reports for one generation (requested via `include_usage`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) struct GatewayUsage {
        pub(crate) in_tok: i64,
        pub(crate) out_tok: i64,
    }

    /// The `usage` object of an OpenAI-style stream chunk, if this line carries one. Checked on
    /// every line, separately from [`parse_gateway_sse_line`], because some providers put it on
    /// the last content chunk and others on a chunk with empty `choices`.
    pub(crate) fn parse_gateway_usage(line: &str) -> Option<GatewayUsage> {
        let data = line.trim().strip_prefix("data:")?.trim();
        let v: serde_json::Value = serde_json::from_str(data).ok()?;
        let u = v.get("usage")?;
        let n = |k: &str| u.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0).max(0);
        Some(GatewayUsage {
            in_tok: n("prompt_tokens"),
            out_tok: n("completion_tokens"),
        })
    }

    /// Parse one SSE line of an OpenAI-style stream. Anything that is not a `data:` line with a
    /// text delta, an error event or the `[DONE]` marker (comments, role-only chunks, junk) is
    /// `None`.
    pub(crate) fn parse_gateway_sse_line(line: &str) -> Option<GatewayChunk> {
        let data = line.trim().strip_prefix("data:")?.trim();
        if data == "[DONE]" {
            return Some(GatewayChunk::Done);
        }
        let v: serde_json::Value = serde_json::from_str(data).ok()?;
        // The Gateway reports failures (e.g. a 429 quota limit) as an HTTP 200 stream whose
        // event is `{"error":{...,"code":"429"}}`.
        if let Some(err) = v.get("error") {
            let code = err
                .get("code")
                .and_then(|c| {
                    c.as_str()
                        .map(str::to_string)
                        .or_else(|| Some(c.to_string()))
                })
                .filter(|c| {
                    c != "null"
                        && c.len() <= 32
                        && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                });
            return Some(GatewayChunk::Error(match code {
                Some(code) => format!("the Gateway returned an error (code {code})"),
                None => "the Gateway returned an error".to_string(),
            }));
        }
        let text = v
            .get("choices")?
            .get(0)?
            .get("delta")?
            .get("content")?
            .as_str()?;
        (!text.is_empty()).then(|| GatewayChunk::Delta(text.to_string()))
    }

    /// WR-04: the server-side switch. Fresh read per call; unreadable config means off.
    pub(crate) fn gateway_enabled() -> bool {
        ironhermes_core::config::Config::load()
            .map(|c| c.ui.openui_gateway_enabled)
            .unwrap_or(false)
    }

    /// The stored Thesys key: the vault first, then the environment. Never logged.
    pub(crate) async fn resolve_thesys_key() -> Option<SecretString> {
        if let Ok(config) = ironhermes_core::config::Config::load() {
            let vault_config = ironhermes_core::resolve_vault_config(&config);
            if vault_config.enabled {
                if let Ok(store) = ironhermes_vault::open_store(&vault_config) {
                    if let Ok(Some(secret)) = store.get_secret(GATEWAY_KEY_PROVIDER).await {
                        if !secret.expose_secret().trim().is_empty() {
                            return Some(secret);
                        }
                    }
                }
            }
        }
        std::env::var("THESYS_API_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(SecretString::from)
    }

    /// WR-02: a stuck Gateway must not hold the turn's concurrency permits indefinitely: no
    /// bytes for this long, or this long in total, ends the turn with "the Gateway timed out".
    const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
    const TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

    fn fail(
        tx: &mpsc::UnboundedSender<ChatStreamEvent>,
        turn_id: uuid::Uuid,
        reason: &str,
    ) -> Option<GatewayUsage> {
        let _ = tx.send(ChatStreamEvent::Error {
            turn_id,
            message: reason.to_string(),
        });
        None
    }

    /// Stream one generation from the Gateway as the same `Delta` / `Finished` events a normal
    /// turn emits. No tools, no session history: the locally generated OpenUI prompt is the
    /// system message and `prompt` is the user message. Failures end the turn with an `Error`
    /// carrying a fixed short reason (never the key, the URL or any body). Returns the usage the
    /// Gateway reported for a successful generation, so the caller can record it.
    pub(crate) async fn stream_gateway_generation(
        base_url: &str,
        key: &SecretString,
        prompt: &str,
        turn_id: uuid::Uuid,
        tx: &mpsc::UnboundedSender<ChatStreamEvent>,
    ) -> Option<GatewayUsage> {
        stream_gateway_generation_with_idle(base_url, key, prompt, turn_id, tx, IDLE_TIMEOUT).await
    }

    /// [`stream_gateway_generation`] with an injectable idle timeout (tests use a short one).
    async fn stream_gateway_generation_with_idle(
        base_url: &str,
        key: &SecretString,
        prompt: &str,
        turn_id: uuid::Uuid,
        tx: &mpsc::UnboundedSender<ChatStreamEvent>,
        idle: std::time::Duration,
    ) -> Option<GatewayUsage> {
        let client = match reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(TOTAL_TIMEOUT)
            .build()
        {
            Ok(c) => c,
            Err(_) => return fail(tx, turn_id, "could not start the Gateway client"),
        };
        let body = serde_json::json!({
            "model": GATEWAY_MODEL,
            "stream": true,
            "stream_options": { "include_usage": true },
            "messages": [
                { "role": "system", "content": crate::server::openui_route::openui_overlay_body() },
                { "role": "user", "content": prompt },
            ],
        });
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        // RR-07: the idle timeout covers waiting for the response headers too, not only the body.
        let request = client
            .post(url)
            .bearer_auth(key.expose_secret())
            .json(&body)
            .send();
        let mut resp = match tokio::time::timeout(idle, request).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) if e.is_timeout() => return fail(tx, turn_id, "the Gateway timed out"),
            Ok(Err(_)) => return fail(tx, turn_id, "could not reach the Gateway"),
            Err(_) => return fail(tx, turn_id, "the Gateway timed out"),
        };
        if !resp.status().is_success() {
            let reason = format!("the Gateway returned HTTP {}", resp.status().as_u16());
            return fail(tx, turn_id, &reason);
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut any = false;
        let mut usage: Option<GatewayUsage> = None;
        loop {
            let chunk = match tokio::time::timeout(idle, resp.chunk()).await {
                Ok(Ok(Some(c))) => c,
                Ok(Ok(None)) => break,
                Ok(Err(e)) if e.is_timeout() => {
                    return fail(tx, turn_id, "the Gateway timed out");
                }
                Ok(Err(_)) => return fail(tx, turn_id, "the Gateway stream was interrupted"),
                Err(_) => return fail(tx, turn_id, "the Gateway timed out"),
            };
            buf.extend_from_slice(&chunk);
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                if let Some(u) = parse_gateway_usage(&line) {
                    usage = Some(u);
                }
                match parse_gateway_sse_line(&line) {
                    Some(GatewayChunk::Delta(text)) => {
                        any = true;
                        let _ = tx.send(ChatStreamEvent::Delta { turn_id, text });
                    }
                    Some(GatewayChunk::Done) => {
                        return finish(tx, turn_id, any, usage);
                    }
                    Some(GatewayChunk::Error(reason)) => {
                        return fail(tx, turn_id, &reason);
                    }
                    None => {}
                }
            }
            if buf.len() > MAX_LINE_BYTES {
                return fail(tx, turn_id, "the Gateway sent an unreadable stream");
            }
        }
        finish(tx, turn_id, any, usage)
    }

    fn finish(
        tx: &mpsc::UnboundedSender<ChatStreamEvent>,
        turn_id: uuid::Uuid,
        any: bool,
        usage: Option<GatewayUsage>,
    ) -> Option<GatewayUsage> {
        if !any {
            return fail(tx, turn_id, "the Gateway returned an empty reply");
        }
        let usage = usage.unwrap_or_default();
        let total = usage.in_tok.saturating_add(usage.out_tok);
        let _ = tx.send(ChatStreamEvent::Finished {
            turn_id,
            total_tokens: u32::try_from(total).unwrap_or(u32::MAX),
        });
        Some(usage)
    }

    /// Record one Gateway generation's usage against the OpenUI session, the same
    /// `usage_events` + `sessions` write a normal turn makes. Cost is 0: Thesys bills
    /// separately and publishes no per-token price here.
    pub(crate) fn record_gateway_usage(
        store: &std::sync::Mutex<ironhermes_state::StateStore>,
        session_id: &str,
        usage: GatewayUsage,
    ) {
        if usage == GatewayUsage::default() {
            return;
        }
        let ev = ironhermes_state::UsageEvent {
            session_id: session_id.to_string(),
            ts: chrono::Utc::now().timestamp_millis(),
            provider: "thesys".to_string(),
            model: GATEWAY_MODEL.to_string(),
            in_tok: usage.in_tok,
            out_tok: usage.out_tok,
            cache_read: 0,
            cache_create: 0,
            cost_usd_micros: 0,
            error_kind: None,
        };
        match store.lock() {
            Ok(mut s) => {
                if let Err(e) = s.record_turn_usage(&ev) {
                    tracing::warn!(error = %e, "openui gateway: usage record failed");
                }
            }
            Err(_) => tracing::warn!("openui gateway: state store lock poisoned; usage not recorded"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        #[test]
        fn gateway_stream_parser_emits_deltas() {
            let chunk = r#"data: {"choices":[{"delta":{"content":"root = Card("}}]}"#;
            assert_eq!(
                parse_gateway_sse_line(chunk),
                Some(GatewayChunk::Delta("root = Card(".into()))
            );
            assert_eq!(
                parse_gateway_sse_line("data: [DONE]"),
                Some(GatewayChunk::Done)
            );
            for junk in [
                "",
                ": keep-alive",
                "data: {not json",
                r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#,
                r#"data: {"choices":[{"delta":{"content":""}}]}"#,
                r#"data: {"choices":[]}"#,
                "event: message",
            ] {
                assert_eq!(parse_gateway_sse_line(junk), None, "{junk:?}");
            }
        }

        /// Live 2026-10-29: the Gateway sent HTTP 200 with this event on a quota limit; it was
        /// read as "empty reply".
        #[test]
        fn gateway_in_stream_error_event_is_an_error() {
            let live = r#"data: {"error":{"message":"Http Exception","type":"invalid_request_error","code":"429"}}"#;
            assert_eq!(
                parse_gateway_sse_line(live),
                Some(GatewayChunk::Error(
                    "the Gateway returned an error (code 429)".into()
                ))
            );
            assert_eq!(
                parse_gateway_sse_line(r#"data: {"error":{"message":"sk-live-9 bad"}}"#),
                Some(GatewayChunk::Error("the Gateway returned an error".into()))
            );
        }

        /// A one-shot HTTP server on 127.0.0.1: reads the whole request, answers with `reply`,
        /// and hands the request text back. Never leaves the machine.
        async fn mock(reply: &'static str) -> (String, tokio::task::JoinHandle<String>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}/v1/embed", listener.local_addr().unwrap());
            let handle = tokio::spawn(async move {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut req = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, content_len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    req.extend_from_slice(&tmp[..n]);
                    if let Some(p) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&req[..p]).to_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (p + 4, len);
                    }
                };
                while req.len() < head_end + content_len {
                    let n = sock.read(&mut tmp).await.unwrap();
                    req.extend_from_slice(&tmp[..n]);
                }
                sock.write_all(reply.as_bytes()).await.unwrap();
                sock.shutdown().await.ok();
                String::from_utf8_lossy(&req).to_string()
            });
            (base, handle)
        }

        fn drain(rx: &mut mpsc::UnboundedReceiver<ChatStreamEvent>) -> Vec<ChatStreamEvent> {
            let mut out = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                out.push(ev);
            }
            out
        }

        const OK_STREAM: &str =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"root = Card(\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"[])\"}}]}\n\n\
data: [DONE]\n\n";

        #[tokio::test]
        async fn gateway_streams_from_a_local_mock() {
            let (base, server) = mock(OK_STREAM).await;
            let key = SecretString::from("sk-thesys-SECRET-123".to_string());
            let (tx, mut rx) = mpsc::unbounded_channel();
            let turn = uuid::Uuid::from_u128(7);
            stream_gateway_generation(&base, &key, "a table of cron jobs", turn, &tx).await;
            let req = server.await.unwrap();
            assert!(req.starts_with("POST /v1/embed/chat/completions "), "{req}");
            assert!(
                req.to_lowercase()
                    .contains("authorization: bearer sk-thesys-secret-123"),
                "the key goes in the Authorization header"
            );
            assert!(req.contains("a table of cron jobs") && req.contains("\"stream\":true"));

            let events = drain(&mut rx);
            let texts: Vec<&str> = events
                .iter()
                .filter_map(|e| match e {
                    ChatStreamEvent::Delta { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(texts, ["root = Card(", "[])"]);
            assert!(matches!(
                events.last(),
                Some(ChatStreamEvent::Finished { turn_id, .. }) if *turn_id == turn
            ));
            let wire: String = events
                .iter()
                .map(|e| serde_json::to_string(e).unwrap())
                .collect();
            assert!(
                !wire.contains("SECRET-123"),
                "the key never appears in an event"
            );
        }

        #[tokio::test]
        async fn gateway_http_error_is_a_short_error_without_the_body() {
            let (base, server) = mock(
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 28\r\nConnection: close\r\n\r\n{\"error\":\"bad key sk-live-9\"}",
            )
            .await;
            let key = SecretString::from("sk-live-9".to_string());
            let (tx, mut rx) = mpsc::unbounded_channel();
            stream_gateway_generation(&base, &key, "p", uuid::Uuid::nil(), &tx).await;
            server.await.unwrap();
            let events = drain(&mut rx);
            assert_eq!(events.len(), 1);
            match &events[0] {
                ChatStreamEvent::Error { message, .. } => {
                    assert_eq!(message, "the Gateway returned HTTP 401");
                }
                other => panic!("expected Error, got {other:?}"),
            }
        }

        /// RR-07: a Gateway that accepts the request and never sends headers ends the turn.
        #[tokio::test]
        async fn gateway_silent_before_headers_times_out() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}/v1/embed", listener.local_addr().unwrap());
            let _server = tokio::spawn(async move {
                let (_sock, _) = listener.accept().await.unwrap();
                std::future::pending::<()>().await;
            });
            let key = SecretString::from("sk-thesys-1".to_string());
            let (tx, mut rx) = mpsc::unbounded_channel();
            let idle = std::time::Duration::from_millis(150);
            stream_gateway_generation_with_idle(&base, &key, "p", uuid::Uuid::nil(), &tx, idle)
                .await;
            let events = drain(&mut rx);
            assert_eq!(events.len(), 1, "{events:?}");
            assert!(matches!(
                &events[0],
                ChatStreamEvent::Error { message, .. } if message == "the Gateway timed out"
            ));
        }

        #[test]
        fn gateway_status_never_returns_the_key() {
            let json = serde_json::to_value(super::super::OpenUiGatewayStatus {
                key_present: true,
                enabled: true,
            })
            .unwrap();
            let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
            keys.sort();
            assert_eq!(keys, ["enabled", "key_present"]);
        }

        #[test]
        fn gateway_usage_is_parsed_from_the_usage_chunk() {
            let line = r#"data: {"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":45,"total_tokens":165}}"#;
            assert_eq!(
                parse_gateway_usage(line),
                Some(GatewayUsage { in_tok: 120, out_tok: 45 })
            );
            assert_eq!(
                parse_gateway_usage(r#"data: {"choices":[{"delta":{"content":"x"}}]}"#),
                None
            );
            assert_eq!(parse_gateway_usage("data: [DONE]"), None);
        }

        #[test]
        fn finish_reports_total_tokens_and_returns_usage() {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let id = uuid::Uuid::new_v4();
            let u = GatewayUsage { in_tok: 120, out_tok: 45 };
            assert_eq!(finish(&tx, id, true, Some(u)), Some(u));
            assert!(matches!(
                rx.try_recv().unwrap(),
                ChatStreamEvent::Finished { total_tokens: 165, .. }
            ));
            assert_eq!(finish(&tx, id, false, Some(u)), None, "empty reply records nothing");
        }

        #[test]
        fn gateway_default_endpoint_is_the_adr_recorded_one() {
            assert_eq!(GATEWAY_BASE_URL, "https://api.thesys.dev/v1/embed");
        }
    }
}
