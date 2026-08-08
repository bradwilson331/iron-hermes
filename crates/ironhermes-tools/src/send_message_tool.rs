// Phase 36.3.8 Plan 01 — SendMessageTool + MessageDispatcher trait.
//
// `MessageDispatcher` trait lives here (in ironhermes-tools) so that gateway can
// implement it for `TelegramAdapter` without creating a circular crate dependency
// (gateway → tools, not the reverse). The orphan rule requires the impl to live
// in the gateway crate. This mirrors the AudioDispatcher split in send_audio_tool.rs.
//
// D-01: Addressable target grammar: `platform` | `platform:chat_id` |
//        `platform:chat_id:thread_id`
// D-02: MessageDispatcher trait in tools, impl in gateway (orphan-rule-safe).
// D-03: MEDIA:<path> body → send_attachment; plain text → send_text.
// SECURITY / OQ-2: explicit chat_id not in whitelist → rejected before dispatch.

use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// MessageDispatcher trait — D-02 (orphan-rule-safe split)
// ---------------------------------------------------------------------------

/// Platform-agnostic text/attachment delivery abstraction for `send_message`.
///
/// Implemented in `ironhermes-gateway` for `TelegramAdapter` (orphan-rule-safe:
/// trait defined here in tools, type defined there in gateway).
/// The `ironhermes-tools` crate itself never imports gateway types.
#[async_trait]
pub trait MessageDispatcher: Send + Sync {
    /// Send a plain text message to the given chat.
    async fn send_text(
        &self,
        chat_id: &str,
        text: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a local file as an attachment (document/photo/audio depending on
    /// the gateway impl's content-type detection).
    async fn send_attachment(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// SendMessageTool struct
// ---------------------------------------------------------------------------

/// LLM-callable tool that delivers a message to an addressable target.
///
/// Constructor-injected SessionKey + optional MessageDispatcher so the tool
/// is statically typed to the session at registration time and is inert
/// (is_available → false) when dispatcher is None (e.g. wasm32 compile).
pub struct SendMessageTool {
    session_key: ironhermes_core::SessionKey,
    dispatcher: Option<Arc<dyn MessageDispatcher>>,
    config: Arc<ironhermes_core::Config>,
}

impl SendMessageTool {
    pub fn new(
        session_key: ironhermes_core::SessionKey,
        dispatcher: Option<Arc<dyn MessageDispatcher>>,
        config: Arc<ironhermes_core::Config>,
    ) -> Self {
        Self {
            session_key,
            dispatcher,
            config,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl crate::registry::Tool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn toolset(&self) -> &str {
        "messaging"
    }

    fn description(&self) -> &str {
        "Send a message to an addressable platform target (chat, thread). \
         Optionally specify a MEDIA:<path> body to send a file as an attachment."
    }

    fn schema(&self) -> ironhermes_core::ToolSchema {
        ironhermes_core::ToolSchema::new(
            "send_message",
            "Send a text message or file attachment to a platform target.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Addressable target in the form: \
                            `platform` (home channel) | \
                            `platform:chat_id` | \
                            `platform:chat_id:thread_id`. \
                            Omit or leave empty to reply in the current session chat."
                    },
                    "message": {
                        "type": "string",
                        "description": "Text to send. Prefix with `MEDIA:<absolute_path>` to \
                            send a file as an attachment instead of text."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["send", "list"],
                        "description": "`send` (default) delivers the message; \
                            `list` returns the authorized Telegram targets from config."
                    }
                },
                "required": ["message"]
            }),
        )
    }

    /// Returns false when no dispatcher is attached (e.g. wasm32 or non-gateway
    /// sessions that haven't wired a MessageDispatcher), keeping registration
    /// inert on platforms where the tool can't function.
    fn is_available(&self) -> bool {
        // Local platform never needs a dispatcher
        if self.session_key.platform == ironhermes_core::Platform::Local {
            return true;
        }
        self.dispatcher.is_some()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args["action"].as_str().unwrap_or("send");
        let message = args["message"].as_str().unwrap_or("");
        let target_raw = args["target"].as_str().unwrap_or("");

        match action {
            "list" => self.execute_list(),
            "send" => {
                if message.is_empty() {
                    anyhow::bail!("send_message: 'message' parameter is required for action=send");
                }
                self.execute_send(target_raw, message).await
            }
            other => {
                anyhow::bail!("send_message: unknown action '{other}'; expected 'send' or 'list'")
            }
        }
    }
}

impl SendMessageTool {
    fn execute_list(&self) -> anyhow::Result<String> {
        let mut targets: Vec<serde_json::Value> = Vec::new();
        if let Some(tg) = self.config.gateway.platforms.get("telegram") {
            for id in &tg.whitelist {
                targets.push(serde_json::json!({
                    "platform": "telegram",
                    "chat_id": id,
                    "label": if Some(id) == tg.home_channel_id.as_ref() {
                        "home"
                    } else {
                        "whitelisted"
                    }
                }));
            }
            if let Some(ref hc) = tg.home_channel_id {
                // If home_channel_id is not already in whitelist, surface it too
                if !tg.whitelist.iter().any(|id| id == hc) {
                    targets.push(serde_json::json!({
                        "platform": "telegram",
                        "chat_id": hc,
                        "label": "home"
                    }));
                }
            }
        }
        targets.push(serde_json::json!({
            "platform": "local",
            "chat_id": "local",
            "label": "local (stdout)"
        }));
        Ok(serde_json::json!({ "targets": targets }).to_string())
    }

    async fn execute_send(&self, target_raw: &str, message: &str) -> anyhow::Result<String> {
        let (platform_str, chat_id, thread_id) =
            parse_target(target_raw, &self.session_key, &self.config)?;

        match platform_str.as_str() {
            "local" => {
                // Local path: print to stdout (no dispatcher needed)
                println!("[send_message local] {}", message);
                Ok(serde_json::json!({
                    "success": true,
                    "platform": "local",
                    "chat_id": chat_id
                })
                .to_string())
            }
            "telegram" => {
                // Always run the whitelist gate on the resolved chat_id (CR-02).
                // The ONLY exception is the session's own chat (the user who
                // triggered this turn) — that reply path is implicitly trusted.
                // Any other target, including a configured home_channel_id that
                // is not in the whitelist, is rejected before dispatch.
                let is_session_self = chat_id == self.session_key.chat_id
                    && platform_str == self.session_key.platform.to_string();
                if !is_session_self {
                    validate_send_target("telegram", &chat_id, &self.config)?;
                }

                let dispatcher = self.dispatcher.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "send_message on Telegram requires a MessageDispatcher \
                         (none attached this session)"
                    )
                })?;

                let thread_ref = thread_id.as_deref();

                if let Some(media_path) = detect_media_path(message) {
                    let path = std::path::PathBuf::from(&media_path);
                    // WR-05: validate the path at the tool layer so every
                    // MessageDispatcher impl is protected — not just Telegram.
                    let path = validate_media_path(&path)?;
                    dispatcher
                        .send_attachment(&chat_id, &path, thread_ref)
                        .await?;
                    Ok(serde_json::json!({
                        "success": true,
                        "platform": "telegram",
                        "chat_id": chat_id,
                        "sent": "attachment",
                        "path": media_path
                    })
                    .to_string())
                } else {
                    dispatcher.send_text(&chat_id, message, thread_ref).await?;
                    Ok(serde_json::json!({
                        "success": true,
                        "platform": "telegram",
                        "chat_id": chat_id,
                        "sent": "text"
                    })
                    .to_string())
                }
            }
            other => anyhow::bail!(
                "send_message: platform '{other}' is not yet wired \
                 (Discord/Slack deferred to a follow-up phase)"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Free functions — public for tests and downstream use
// ---------------------------------------------------------------------------

/// Parse the target string into (platform, chat_id, thread_id).
///
/// Target grammar (D-01):
///   - `""` (empty)   → use session platform + session chat_id, no thread
///   - `platform`     → resolve home channel for that platform
///   - `platform:id`  → explicit chat_id on platform
///   - `platform:id:thread` → explicit chat_id + thread_id
///
/// Panics never; returns Err for unresolvable home channels.
pub fn parse_target(
    target: &str,
    session_key: &ironhermes_core::SessionKey,
    config: &ironhermes_core::Config,
) -> anyhow::Result<(String, String, Option<String>)> {
    let target = target.trim();

    if target.is_empty() {
        // No target — use current session
        return Ok((
            session_key.platform.to_string(),
            session_key.chat_id.clone(),
            None,
        ));
    }

    // Split on first ':'
    let mut parts = target.splitn(2, ':');
    let platform = parts.next().unwrap_or("").trim().to_lowercase();

    let rest = parts.next().map(str::trim);

    if rest.is_none() || rest == Some("") {
        // Bare platform name → resolve home channel
        let chat_id = resolve_home_channel(&platform, config)?;
        return Ok((platform, chat_id, None));
    }

    let rest = rest.unwrap();

    // Telegram supports chat_id:thread_id (two-level split after platform)
    if let Some((chat, thread)) = rest.split_once(':').filter(|_| platform == "telegram") {
        return Ok((
            platform,
            chat.trim().to_string(),
            Some(thread.trim().to_string()),
        ));
    }

    Ok((platform, rest.to_string(), None))
}

/// Resolve the home channel for a bare platform target.
///
/// For Telegram:
///
/// 1. If `config.gateway.platforms["telegram"].home_channel_id` is set → use
///    it. The value MUST be a numeric Telegram chat/user ID — this validates
///    a Telegram Bot API constraint (chat/user IDs are always numeric),
///    independent of the whitelist's own type: `@username` style handles are
///    not supported. A non-numeric value returns a clear error rather than a
///    confusing downstream failure.
/// 2. Else if whitelist.len() == 1 → use whitelist[0].
/// 3. Else → Err("ambiguous home channel: set telegram.home_channel_id").
pub fn resolve_home_channel(
    platform: &str,
    config: &ironhermes_core::Config,
) -> anyhow::Result<String> {
    match platform {
        "telegram" => {
            let tg = config
                .gateway
                .platforms
                .get("telegram")
                .ok_or_else(|| anyhow::anyhow!("telegram platform not configured"))?;

            // Prefer explicit home_channel_id (WR-01: validate it is numeric
            // so misconfigurations surface immediately with a clear message).
            if let Some(ref hc) = tg.home_channel_id {
                if hc.trim().parse::<i64>().is_err() {
                    anyhow::bail!(
                        "send_message: telegram.home_channel_id '{}' is not a valid \
                         numeric Telegram chat ID (expected i64). \
                         @username handles are not supported — use the numeric chat ID.",
                        hc
                    );
                }
                return Ok(hc.clone());
            }

            // Fall back to single whitelist entry
            match tg.whitelist.len() {
                1 => Ok(tg.whitelist[0].clone()),
                0 => anyhow::bail!(
                    "ambiguous home channel: telegram whitelist is empty; \
                     set telegram.home_channel_id in config"
                ),
                _ => anyhow::bail!(
                    "ambiguous home channel: telegram whitelist has {} entries; \
                     set telegram.home_channel_id in config to disambiguate",
                    tg.whitelist.len()
                ),
            }
        }
        "local" => Ok("local".to_string()),
        other => anyhow::bail!("resolve_home_channel: unsupported platform '{other}'"),
    }
}

/// Validate that `chat_id` is authorized for the given platform.
///
/// For Telegram: chat_id must be present in the (string) whitelist.
///
/// - Empty whitelist → Err("no authorized targets configured").
/// - Non-member → Err naming the rejected id.
/// - `local` → always Ok (no whitelist defined at this layer; the local
///   stdout sink has no notion of authorized senders).
///
/// Every other platform → Err naming the platform (Phase 47.6 Plan 01
/// fail-closed correction — this function previously returned `Ok(())` for
/// EVERY non-Telegram platform string, which meant any future caller routing
/// a non-Telegram target through it got an authorization pass with no
/// whitelist consulted at all. This is a fail-closed correction, not a new
/// feature: adding a Buzz send arm to `execute_send` is deferred — see
/// `execute_send`'s own `other =>` arm).
pub fn validate_send_target(
    platform: &str,
    chat_id: &str,
    config: &ironhermes_core::Config,
) -> anyhow::Result<()> {
    match platform {
        "local" => return Ok(()),
        "telegram" => {}
        other => anyhow::bail!(
            "send_message: platform '{other}' has no whitelist gate defined in \
             validate_send_target — rejecting rather than silently authorizing (fail-closed)"
        ),
    }

    let tg = config
        .gateway
        .platforms
        .get("telegram")
        .ok_or_else(|| anyhow::anyhow!("telegram platform not configured"))?;

    if tg.whitelist.is_empty() {
        anyhow::bail!(
            "send_message: no authorized targets configured \
             (telegram.whitelist is empty)"
        );
    }

    if !tg.whitelist.iter().any(|w| w == chat_id) {
        anyhow::bail!(
            "send_message: chat_id '{}' is not in the telegram whitelist \
             (authorized targets: {:?})",
            chat_id,
            tg.whitelist
        );
    }

    Ok(())
}

/// Detect a `MEDIA:<path>` prefix in the message body.
///
/// Returns `Some(path_string)` when the body starts with the `MEDIA:` prefix,
/// with surrounding whitespace stripped from the path token.
/// Returns `None` for plain text bodies.
///
/// Only the uppercase `MEDIA:` prefix is recognised (D-03 convention).
pub fn detect_media_path(message: &str) -> Option<String> {
    let path = message.strip_prefix("MEDIA:")?.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Validate and canonicalize a media attachment path (WR-05).
///
/// Centralises the path-traversal confinement guard at the tool layer so it
/// is enforced for **every** `MessageDispatcher` implementation — not just the
/// Telegram gateway adapter. Any dispatcher that calls `send_attachment` with
/// the returned `PathBuf` can trust the path is under an allowed root.
///
/// Allowed roots:
/// - `$IRONHERMES_HOME` (or `~/.ironhermes` if unset) — where app-generated
///   media lives (e.g. TTS output in `audio_cache/`).
/// - `/tmp` — for transient scratch files.
///
/// Returns `Err` for paths outside these roots or that cannot be canonicalized
/// (symlink attacks, non-existent files, relative paths).
pub fn validate_media_path(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    let canonical = path.canonicalize().map_err(|e| {
        anyhow::anyhow!(
            "send_message: cannot canonicalize media path '{}': {} \
             (file must exist and be accessible)",
            path.display(),
            e
        )
    })?;

    let mut allowed_roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(home) = ironhermes_core::constants::get_hermes_home().canonicalize() {
        allowed_roots.push(home);
    }
    if let Ok(tmp) = std::path::PathBuf::from("/tmp").canonicalize() {
        allowed_roots.push(tmp);
    }

    let allowed = allowed_roots.iter().any(|root| canonical.starts_with(root));

    if !allowed {
        anyhow::bail!(
            "send_message: media path '{}' is outside the allowed roots \
             ({:?}); path traversal rejected",
            path.display(),
            allowed_roots
        );
    }

    Ok(canonical)
}

// ---------------------------------------------------------------------------
// Inline unit tests for the free functions (fast, no I/O)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{Config, Platform, SessionKey};

    fn minimal_config_with_whitelist(ids: Vec<i64>) -> Arc<Config> {
        let mut cfg = Config::default();
        let tg = ironhermes_core::PlatformGatewayConfig {
            enabled: true,
            whitelist: ids.into_iter().map(|id| id.to_string()).collect(),
            ..Default::default()
        };
        cfg.gateway.platforms.insert("telegram".to_string(), tg);
        Arc::new(cfg)
    }

    #[test]
    fn clarify_timeout_secs_default_is_120() {
        let cfg = Config::default();
        assert_eq!(cfg.agent.clarify_timeout_secs, 120);
    }

    #[test]
    fn home_channel_id_default_is_none() {
        // PlatformGatewayConfig default leaves home_channel_id unset
        let tg = ironhermes_core::PlatformGatewayConfig::default();
        assert!(tg.home_channel_id.is_none());
    }

    #[test]
    fn detect_media_path_plain() {
        assert!(detect_media_path("hello world").is_none());
    }

    #[test]
    fn detect_media_path_media_prefix() {
        assert_eq!(
            detect_media_path("MEDIA:/tmp/file.png"),
            Some("/tmp/file.png".to_string())
        );
    }

    #[test]
    fn detect_media_path_with_space() {
        assert_eq!(
            detect_media_path("MEDIA: /tmp/file.png"),
            Some("/tmp/file.png".to_string())
        );
    }

    #[test]
    fn parse_target_empty_uses_session() {
        let cfg = minimal_config_with_whitelist(vec![]);
        let sk = SessionKey::new(Platform::Telegram, "42000");
        let (p, c, t) = parse_target("", &sk, &cfg).unwrap();
        assert_eq!(p, "telegram");
        assert_eq!(c, "42000");
        assert!(t.is_none());
    }

    #[test]
    fn parse_target_explicit_chat() {
        let cfg = minimal_config_with_whitelist(vec![12345]);
        let sk = SessionKey::new(Platform::Telegram, "99");
        let (p, c, t) = parse_target("telegram:12345", &sk, &cfg).unwrap();
        assert_eq!(p, "telegram");
        assert_eq!(c, "12345");
        assert!(t.is_none());
    }

    #[test]
    fn parse_target_explicit_chat_and_thread() {
        let cfg = minimal_config_with_whitelist(vec![12345]);
        let sk = SessionKey::new(Platform::Telegram, "99");
        let (p, c, t) = parse_target("telegram:12345:678", &sk, &cfg).unwrap();
        assert_eq!(p, "telegram");
        assert_eq!(c, "12345");
        assert_eq!(t, Some("678".to_string()));
    }

    #[test]
    fn parse_target_bare_telegram_single_whitelist() {
        let cfg = minimal_config_with_whitelist(vec![55555]);
        let sk = SessionKey::new(Platform::Telegram, "99");
        let (_, c, _) = parse_target("telegram", &sk, &cfg).unwrap();
        assert_eq!(c, "55555");
    }

    #[test]
    fn parse_target_bare_telegram_ambiguous_errors() {
        let cfg = minimal_config_with_whitelist(vec![11111, 22222]);
        let sk = SessionKey::new(Platform::Telegram, "99");
        let result = parse_target("telegram", &sk, &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn validate_whitelist_accepts_member() {
        let cfg = minimal_config_with_whitelist(vec![12345, 99999]);
        validate_send_target("telegram", "12345", &cfg).unwrap();
    }

    #[test]
    fn validate_whitelist_rejects_non_member() {
        let cfg = minimal_config_with_whitelist(vec![12345]);
        assert!(validate_send_target("telegram", "99999", &cfg).is_err());
    }

    #[test]
    fn validate_whitelist_empty_rejects_all() {
        let cfg = minimal_config_with_whitelist(vec![]);
        let err = validate_send_target("telegram", "12345", &cfg).unwrap_err();
        assert!(err.to_string().contains("no authorized"));
    }

    #[test]
    fn validate_non_telegram_always_ok() {
        let cfg = minimal_config_with_whitelist(vec![]);
        validate_send_target("local", "anything", &cfg).unwrap();
    }

    /// Phase 47.6 Plan 01 fail-closed correction: an unrecognized platform
    /// token must be rejected, not silently authorized.
    #[test]
    fn validate_unrecognized_platform_rejected() {
        let cfg = minimal_config_with_whitelist(vec![]);
        let err = validate_send_target("discord", "anything", &cfg).unwrap_err();
        assert!(
            err.to_string().contains("discord"),
            "error should name the rejected platform, got: {err}"
        );
    }

    /// CR-02: home_channel_id outside the whitelist must be rejected by
    /// validate_send_target. The execute_send path now always runs this gate
    /// on the resolved chat_id (except for the session's own chat), so a
    /// home_channel_id that is not a whitelist member must fail.
    #[test]
    fn home_channel_outside_whitelist_is_rejected() {
        let mut cfg = Config::default();
        let tg = ironhermes_core::PlatformGatewayConfig {
            enabled: true,
            whitelist: vec!["11111".to_string()],
            home_channel_id: Some("99999".to_string()), // NOT in whitelist
            ..Default::default()
        };
        cfg.gateway.platforms.insert("telegram".to_string(), tg);
        let cfg = Arc::new(cfg);

        // Resolve the home channel the same way parse_target / resolve_home_channel does
        let resolved = resolve_home_channel("telegram", &cfg).unwrap();
        assert_eq!(resolved, "99999");

        // validate_send_target must reject the resolved home channel
        let err = validate_send_target("telegram", &resolved, &cfg).unwrap_err();
        assert!(
            err.to_string().contains("not in the telegram whitelist"),
            "expected whitelist rejection, got: {err}"
        );
    }

    /// CR-02: home_channel_id that IS in the whitelist must be accepted.
    #[test]
    fn home_channel_in_whitelist_is_accepted() {
        let mut cfg = Config::default();
        let tg = ironhermes_core::PlatformGatewayConfig {
            enabled: true,
            whitelist: vec!["11111".to_string(), "55555".to_string()],
            home_channel_id: Some("55555".to_string()), // in whitelist
            ..Default::default()
        };
        cfg.gateway.platforms.insert("telegram".to_string(), tg);
        let cfg = Arc::new(cfg);

        let resolved = resolve_home_channel("telegram", &cfg).unwrap();
        assert_eq!(resolved, "55555");
        validate_send_target("telegram", &resolved, &cfg).unwrap();
    }
}
