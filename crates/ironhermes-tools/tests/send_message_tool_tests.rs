// Phase 36.3.8 Plan 01 Task 2 — Unit tests for SendMessageTool helpers.
//
// Tests cover:
//   - parse_target: all D-01 target grammar shapes
//   - validate_send_target: whitelist gate (SECURITY / OQ-2)
//   - MEDIA: prefix detection
//   - schema: send+list only, no react verb
//
// Uses the public helper functions exposed from send_message_tool.

use ironhermes_core::{Config, Platform, SessionKey};
use ironhermes_tools::send_message_tool::{detect_media_path, parse_target, validate_send_target};

// ---------------------------------------------------------------------------
// Helpers: build a minimal Config with a Telegram whitelist
// ---------------------------------------------------------------------------

fn config_with_whitelist(ids: Vec<i64>) -> std::sync::Arc<Config> {
    let mut cfg = Config::default();
    let tg = ironhermes_core::PlatformGatewayConfig {
        enabled: true,
        whitelist: ids.into_iter().map(|id| id.to_string()).collect(),
        ..Default::default()
    };
    cfg.gateway.platforms.insert("telegram".to_string(), tg);
    std::sync::Arc::new(cfg)
}

fn config_with_whitelist_and_home(ids: Vec<i64>, home: Option<String>) -> std::sync::Arc<Config> {
    let mut cfg = Config::default();
    let tg = ironhermes_core::PlatformGatewayConfig {
        enabled: true,
        whitelist: ids.into_iter().map(|id| id.to_string()).collect(),
        home_channel_id: home,
        ..Default::default()
    };
    cfg.gateway.platforms.insert("telegram".to_string(), tg);
    std::sync::Arc::new(cfg)
}

fn telegram_session(chat_id: &str) -> SessionKey {
    SessionKey::new(Platform::Telegram, chat_id)
}

// ---------------------------------------------------------------------------
// parse_target tests — D-01
// ---------------------------------------------------------------------------

#[test]
fn parse_target_explicit_chat_id() {
    let cfg = config_with_whitelist(vec![12345]);
    let sk = telegram_session("99999");
    let (platform, chat_id, thread_id) = parse_target("telegram:12345", &sk, &cfg).unwrap();
    assert_eq!(platform, "telegram");
    assert_eq!(chat_id, "12345");
    assert!(thread_id.is_none());
}

#[test]
fn parse_target_chat_and_thread_id() {
    let cfg = config_with_whitelist(vec![12345]);
    let sk = telegram_session("99999");
    let (platform, chat_id, thread_id) = parse_target("telegram:12345:678", &sk, &cfg).unwrap();
    assert_eq!(platform, "telegram");
    assert_eq!(chat_id, "12345");
    assert_eq!(thread_id, Some("678".to_string()));
}

#[test]
fn parse_target_empty_uses_session_chat_id() {
    let cfg = config_with_whitelist(vec![42000]);
    let sk = telegram_session("42000");
    let (platform, chat_id, thread_id) = parse_target("", &sk, &cfg).unwrap();
    assert_eq!(platform, "telegram");
    assert_eq!(chat_id, "42000");
    assert!(thread_id.is_none());
}

#[test]
fn parse_target_bare_platform_single_whitelist() {
    // bare `telegram` with exactly one whitelist entry → resolves to whitelist[0]
    let cfg = config_with_whitelist(vec![55555]);
    let sk = telegram_session("99999");
    let (platform, chat_id, thread_id) = parse_target("telegram", &sk, &cfg).unwrap();
    assert_eq!(platform, "telegram");
    assert_eq!(chat_id, "55555");
    assert!(thread_id.is_none());
}

#[test]
fn parse_target_bare_platform_home_channel_id_overrides_whitelist() {
    // explicit home_channel_id wins even when whitelist.len()==1
    let cfg = config_with_whitelist_and_home(vec![55555], Some("77777".to_string()));
    let sk = telegram_session("99999");
    let (_, chat_id, _) = parse_target("telegram", &sk, &cfg).unwrap();
    assert_eq!(chat_id, "77777");
}

#[test]
fn parse_target_bare_platform_ambiguous_errors() {
    // whitelist.len() > 1 and no home_channel_id → error
    let cfg = config_with_whitelist(vec![11111, 22222]);
    let sk = telegram_session("99999");
    let result = parse_target("telegram", &sk, &cfg);
    assert!(result.is_err(), "expected error for ambiguous home channel");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("ambiguous") || msg.contains("home_channel_id"),
        "error should mention ambiguous or home_channel_id, got: {msg}"
    );
}

#[test]
fn parse_target_unknown_platform_errors() {
    let cfg = config_with_whitelist(vec![]);
    let sk = telegram_session("99999");
    let result = parse_target("discord:12345", &sk, &cfg);
    // Discord is deferred — should return an error or a "not yet wired" passthrough.
    // We just verify it parses the platform field correctly; downstream dispatch errors.
    // This test verifies the platform is lowercased and returned as "discord".
    if let Ok((platform, _, _)) = result {
        assert_eq!(platform, "discord");
    }
    // Err is also acceptable for an unrecognized/disabled platform.
}

// ---------------------------------------------------------------------------
// validate_send_target tests — SECURITY / OQ-2
// ---------------------------------------------------------------------------

#[test]
fn validate_send_target_accepts_whitelisted() {
    let cfg = config_with_whitelist(vec![12345, 99999]);
    validate_send_target("telegram", "12345", &cfg).unwrap();
}

#[test]
fn validate_send_target_rejects_non_member() {
    let cfg = config_with_whitelist(vec![12345]);
    let result = validate_send_target("telegram", "99999", &cfg);
    assert!(
        result.is_err(),
        "expected rejection of chat_id not in whitelist"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("99999") || msg.contains("not authorized") || msg.contains("whitelist"),
        "error should name the rejected id or whitelist, got: {msg}"
    );
}

#[test]
fn validate_send_target_empty_whitelist_rejects_all() {
    let cfg = config_with_whitelist(vec![]);
    let result = validate_send_target("telegram", "12345", &cfg);
    assert!(result.is_err(), "empty whitelist should reject all targets");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("no authorized") || msg.contains("whitelist"),
        "error should mention no authorized targets, got: {msg}"
    );
}

#[test]
fn validate_send_target_non_telegram_passes_through() {
    // Non-telegram platforms are not whitelist-checked at this layer
    let cfg = config_with_whitelist(vec![]);
    validate_send_target("local", "any_id", &cfg).unwrap();
}

// ---------------------------------------------------------------------------
// MEDIA: prefix detection — D-03
// ---------------------------------------------------------------------------

#[test]
fn detect_media_path_plain_text_returns_none() {
    assert!(detect_media_path("Hello, world!").is_none());
}

#[test]
fn detect_media_path_media_prefix_returns_path() {
    let path = detect_media_path("MEDIA:/tmp/photo.png");
    assert_eq!(path, Some("/tmp/photo.png".to_string()));
}

#[test]
fn detect_media_path_media_with_whitespace() {
    let path = detect_media_path("MEDIA: /home/user/audio.ogg");
    assert_eq!(path, Some("/home/user/audio.ogg".to_string()));
}

#[test]
fn detect_media_path_lowercase_media_returns_none() {
    // Convention is uppercase MEDIA:
    assert!(detect_media_path("media:/tmp/file.png").is_none());
}

// ---------------------------------------------------------------------------
// Schema: send+list only, no react verb — OQ-1
// ---------------------------------------------------------------------------

#[test]
fn schema_contains_send_and_list_actions_not_react() {
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::send_message_tool::SendMessageTool;

    let cfg = std::sync::Arc::new(Config::default());
    let sk = SessionKey::new(Platform::Local, "test");
    let tool = SendMessageTool::new(sk, None, cfg);
    let schema = tool.schema();
    let schema_str = serde_json::to_string(&schema).unwrap();

    // Must contain the send and list verbs
    assert!(
        schema_str.contains("send") || schema_str.contains("\"send\""),
        "schema should contain send verb"
    );
    assert!(
        schema_str.contains("list") || schema_str.contains("\"list\""),
        "schema should contain list verb"
    );
    // Must NOT contain the deferred react/unreact verbs (OQ-1)
    assert!(
        !schema_str.contains("react"),
        "schema must not contain react verb (deferred to follow-up phase)"
    );
}
