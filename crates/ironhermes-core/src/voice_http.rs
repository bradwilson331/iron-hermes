//! Phase 54.7 (D-02/D-04): shared HTTP primitives for the configurable voice
//! services — a no-redirect client, optional bearer auth, a fixed error
//! vocabulary and namespaced key resolution.
//!
//! The voice HTTP client deliberately applies no private-address blocking:
//! the operator's LAN and loopback hosts are the supported targets (D-04).

use std::time::Duration;

/// Total timeout for model/voice discovery requests.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// Total timeout for speech/transcription requests.
pub const SPEECH_TIMEOUT: Duration = Duration::from_secs(60);
/// Connect timeout for every voice request.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum accepted discovery response body in bytes.
pub const DISCOVERY_MAX_BYTES: usize = 1 << 20;
/// Maximum accepted synthesized-audio response body in bytes (WR-10). Replies are
/// truncated to 4096 characters before synthesis, so even uncompressed 44.1 kHz
/// stereo WAV stays well under this; it only bounds a misbehaving server.
pub const AUDIO_MAX_BYTES: usize = 64 << 20;

/// Build the voice HTTP client with the given total timeout.
///
/// Redirects are never followed (D-04, T-54.7-01): a 3xx from an
/// operator-typed host could otherwise bounce the request (and its bearer
/// token) to a metadata address or another host. A 3xx surfaces to the caller
/// as an unexpected response.
pub fn build_voice_client(total: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(total)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("voice HTTP client builds")
}

/// Attach `Authorization: Bearer <key>` only when a usable key is supplied.
///
/// `None` and an empty key add no header. A key containing a control
/// character (CR/LF header injection, T-54.7-06) is not attached.
pub fn with_optional_bearer(
    req: reqwest::RequestBuilder,
    key: Option<&secrecy::SecretString>,
) -> reqwest::RequestBuilder {
    use secrecy::ExposeSecret;
    let Some(key) = key else {
        return req;
    };
    let value = key.expose_secret();
    if value.is_empty() {
        return req;
    }
    if value.chars().any(char::is_control) {
        tracing::warn!("voice key contains a control character; Authorization header not attached");
        return req;
    }
    req.bearer_auth(value)
}

/// The fixed category vocabulary for voice-service failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceErrorCategory {
    ServerUnreachable,
    TimedOut,
    KeyRejected,
    UnexpectedResponse,
    ModelNotAvailable,
    AddressNotAllowed,
}

impl VoiceErrorCategory {
    /// The UI-SPEC category string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ServerUnreachable => "server unreachable",
            Self::TimedOut => "timed out",
            Self::KeyRejected => "key rejected",
            Self::UnexpectedResponse => "unexpected response",
            Self::ModelNotAvailable => "model not available",
            Self::AddressNotAllowed => "address not allowed",
        }
    }
}

/// Map a non-success HTTP status to a category: 401/403 are `KeyRejected`,
/// every other non-2xx (including a 3xx, since redirects are not followed) is
/// `UnexpectedResponse`.
pub fn category_for_status(status: reqwest::StatusCode) -> VoiceErrorCategory {
    match status.as_u16() {
        401 | 403 => VoiceErrorCategory::KeyRejected,
        _ => VoiceErrorCategory::UnexpectedResponse,
    }
}

/// Map a transport error to a category.
pub fn category_for_error(err: &reqwest::Error) -> VoiceErrorCategory {
    if err.is_timeout() {
        VoiceErrorCategory::TimedOut
    } else if err.is_connect() {
        VoiceErrorCategory::ServerUnreachable
    } else {
        VoiceErrorCategory::UnexpectedResponse
    }
}

/// An error carrying only a category, so callers can `downcast_ref` it.
#[derive(Debug, Clone, Copy)]
pub struct VoiceHttpError {
    pub category: VoiceErrorCategory,
}

impl std::fmt::Display for VoiceHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.category.as_str())
    }
}

impl std::error::Error for VoiceHttpError {}

/// Result of resolving a key reference.
pub enum VoiceKey {
    /// No reference configured: send no Authorization header.
    None,
    /// A reference was configured but could not be resolved.
    NotFound,
    /// The resolved secret.
    Found(secrecy::SecretString),
}

impl std::fmt::Debug for VoiceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("VoiceKey::None"),
            Self::NotFound => f.write_str("VoiceKey::NotFound"),
            Self::Found(_) => f.write_str("VoiceKey::Found([redacted])"),
        }
    }
}

fn secret_from_env(name: &str) -> VoiceKey {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => VoiceKey::Found(secrecy::SecretString::from(v)),
        _ => VoiceKey::NotFound,
    }
}

/// Resolve a namespaced key reference (D-02, T-54.7-70).
///
/// - `None` / empty -> [`VoiceKey::None`] (no Authorization header).
/// - A reference outside the `VOICE_KEY_*` / `secret/voice/*` namespace (for
///   example a hand-edited config.yaml the web validator never saw) ->
///   [`VoiceKey::NotFound`], decided BEFORE any environment or vault read.
/// - `VOICE_KEY_*` is read from the environment only; `secret/voice/*` from
///   the vault only, and only when a vault config is supplied.
///
/// There is deliberately no fallback to any provider key variable. Warnings
/// name the reference only, never a value; a malformed (secret-shaped)
/// reference is not logged at all.
pub async fn resolve_voice_key(
    key_ref: Option<&str>,
    vault: Option<&ironhermes_vault::VaultConfig>,
) -> VoiceKey {
    use crate::voice_endpoint::{KeyRefKind, VoiceFieldError, key_ref_kind, validate_key_ref};

    let Some(name) = key_ref.map(str::trim).filter(|s| !s.is_empty()) else {
        return VoiceKey::None;
    };
    let Some(kind) = key_ref_kind(name) else {
        match validate_key_ref(name) {
            Err(VoiceFieldError::KeyRefNamespace) => {
                tracing::warn!(
                    reference = name,
                    "voice key reference is outside the VOICE_KEY_*/secret/voice/* namespace; not resolved"
                );
            }
            _ => tracing::warn!("voice key reference is malformed; not resolved"),
        }
        return VoiceKey::NotFound;
    };
    match kind {
        KeyRefKind::Env => secret_from_env(name),
        KeyRefKind::Vault => {
            let Some(vault) = vault else {
                tracing::warn!(
                    reference = name,
                    "vault is not enabled; key reference not resolved"
                );
                return VoiceKey::NotFound;
            };
            let store = match ironhermes_vault::open_store(vault) {
                Ok(s) => s,
                Err(_) => {
                    tracing::warn!(
                        reference = name,
                        "vault could not be opened for key reference"
                    );
                    return VoiceKey::NotFound;
                }
            };
            match store.get_secret(name).await {
                Ok(Some(secret)) => VoiceKey::Found(secret),
                Ok(None) => VoiceKey::NotFound,
                Err(_) => {
                    tracing::warn!(reference = name, "vault read failed for key reference");
                    VoiceKey::NotFound
                }
            }
        }
    }
}

/// Read one named environment variable.
///
/// ONLY for the implicit api.openai.com realtime default (plan 06
/// `resolve_realtime_key`, RESEARCH Pitfall 9). Never call this with an
/// operator-typed reference: it is not namespaced.
pub fn resolve_builtin_env_key(env_name: &str) -> VoiceKey {
    secret_from_env(env_name)
}

/// The vault config to resolve `secret/voice/*` references against, when enabled.
pub fn voice_vault_config(config: &crate::config::Config) -> Option<ironhermes_vault::VaultConfig> {
    if config.vault.enabled {
        Some(crate::vault::resolve_vault_config(config))
    } else {
        None
    }
}

/// Read a response body, failing once it exceeds `max` bytes.
pub async fn read_body_capped(
    resp: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, VoiceErrorCategory> {
    let mut resp = resp;
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Err(VoiceErrorCategory::UnexpectedResponse);
    }
    let mut body: Vec<u8> = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > max {
                    return Err(VoiceErrorCategory::UnexpectedResponse);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(e) => return Err(category_for_error(&e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::{ExposeSecret, SecretString};
    use tokio::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Serialises the tests that mutate process environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    fn found_value(k: &VoiceKey) -> Option<String> {
        match k {
            VoiceKey::Found(s) => Some(s.expose_secret().to_string()),
            _ => None,
        }
    }

    #[tokio::test]
    async fn resolve_voice_key_reads_only_namespaced_refs() {
        let _guard = ENV_LOCK.lock().await;
        // SAFETY: serialised by ENV_LOCK; names are unique to this test.
        unsafe {
            std::env::set_var("VOICE_KEY_IH_TEST_547_SET", "test-fake-key");
            std::env::set_var("IH_TEST_547_UNRELATED", "test-unrelated-key");
            std::env::remove_var("VOICE_KEY_IH_TEST_547_UNSET");
        }

        assert!(matches!(
            resolve_voice_key(None, None).await,
            VoiceKey::None
        ));
        assert!(matches!(
            resolve_voice_key(Some(""), None).await,
            VoiceKey::None
        ));

        let set = resolve_voice_key(Some("VOICE_KEY_IH_TEST_547_SET"), None).await;
        assert_eq!(found_value(&set).as_deref(), Some("test-fake-key"));

        // An out-of-namespace variable that IS set must not be read.
        let unrelated = resolve_voice_key(Some("IH_TEST_547_UNRELATED"), None).await;
        assert!(matches!(unrelated, VoiceKey::NotFound), "{unrelated:?}");

        let unset = resolve_voice_key(Some("VOICE_KEY_IH_TEST_547_UNSET"), None).await;
        assert!(matches!(unset, VoiceKey::NotFound), "{unset:?}");

        // Vault references: no vault supplied, and an env-var-backend vault.
        let no_vault = resolve_voice_key(Some("secret/voice/x"), None).await;
        assert!(matches!(no_vault, VoiceKey::NotFound), "{no_vault:?}");
        let env_backend = ironhermes_vault::VaultConfig {
            enabled: true,
            ..Default::default()
        };
        let via_env_backend = resolve_voice_key(Some("secret/voice/x"), Some(&env_backend)).await;
        assert!(
            matches!(via_env_backend, VoiceKey::NotFound),
            "{via_env_backend:?}"
        );

        // The builtin path is not namespaced; it is only ever called with a
        // provider key name.
        let builtin = resolve_builtin_env_key("IH_TEST_547_UNRELATED");
        assert_eq!(found_value(&builtin).as_deref(), Some("test-unrelated-key"));
        assert!(matches!(
            resolve_builtin_env_key("IH_TEST_547_DEFINITELY_UNSET"),
            VoiceKey::NotFound
        ));

        // SAFETY: serialised by ENV_LOCK.
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_SET");
            std::env::remove_var("IH_TEST_547_UNRELATED");
        }
    }

    #[tokio::test]
    async fn voice_client_does_not_follow_redirects() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/a"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", format!("{}/b", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/b"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = build_voice_client(Duration::from_secs(5));
        let resp = client
            .get(format!("{}/a", server.uri()))
            .send()
            .await
            .expect("request completes");
        assert_eq!(resp.status().as_u16(), 302);

        let received = server.received_requests().await.expect("recording on");
        assert_eq!(received.len(), 1, "exactly one request: {received:?}");
        assert!(
            received.iter().all(|r| r.url.path() != "/b"),
            "the redirect target must never be requested"
        );
    }

    #[tokio::test]
    async fn optional_bearer_omits_header_without_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = build_voice_client(Duration::from_secs(5));
        let url = format!("{}/x", server.uri());

        with_optional_bearer(client.get(&url), None)
            .send()
            .await
            .expect("no-key request");
        let key = SecretString::from("test-fake-key".to_string());
        with_optional_bearer(client.get(&url), Some(&key))
            .send()
            .await
            .expect("keyed request");
        let bad = SecretString::from("test-fake\r\nX-Injected: 1".to_string());
        with_optional_bearer(client.get(&url), Some(&bad))
            .send()
            .await
            .expect("control-char key request");

        let received = server.received_requests().await.expect("recording on");
        assert_eq!(received.len(), 3);
        assert!(received[0].headers.get("authorization").is_none());
        assert_eq!(
            received[1]
                .headers
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer test-fake-key")
        );
        assert!(
            received[2].headers.get("authorization").is_none(),
            "a key with a control character must not be attached"
        );
        assert!(received[2].headers.get("x-injected").is_none());
    }

    #[tokio::test]
    async fn read_body_capped_enforces_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 2048]))
            .mount(&server)
            .await;
        let client = build_voice_client(Duration::from_secs(5));
        let url = format!("{}/big", server.uri());

        let resp = client.get(&url).send().await.expect("request");
        assert_eq!(
            read_body_capped(resp, 1024).await,
            Err(VoiceErrorCategory::UnexpectedResponse)
        );
        let resp = client.get(&url).send().await.expect("request");
        assert_eq!(
            read_body_capped(resp, 4096).await.map(|b| b.len()),
            Ok(2048)
        );
    }

    #[test]
    fn category_mapping_is_fixed_vocabulary() {
        use reqwest::StatusCode;
        assert_eq!(
            category_for_status(StatusCode::UNAUTHORIZED),
            VoiceErrorCategory::KeyRejected
        );
        assert_eq!(
            category_for_status(StatusCode::FORBIDDEN),
            VoiceErrorCategory::KeyRejected
        );
        assert_eq!(
            category_for_status(StatusCode::FOUND),
            VoiceErrorCategory::UnexpectedResponse
        );
        assert_eq!(
            category_for_status(StatusCode::INTERNAL_SERVER_ERROR),
            VoiceErrorCategory::UnexpectedResponse
        );
        let allowed = [
            "server unreachable",
            "timed out",
            "key rejected",
            "unexpected response",
            "model not available",
            "address not allowed",
            "this browser can't reach the address",
        ];
        for c in [
            VoiceErrorCategory::ServerUnreachable,
            VoiceErrorCategory::TimedOut,
            VoiceErrorCategory::KeyRejected,
            VoiceErrorCategory::UnexpectedResponse,
            VoiceErrorCategory::ModelNotAvailable,
            VoiceErrorCategory::AddressNotAllowed,
        ] {
            assert!(allowed.contains(&c.as_str()), "{}", c.as_str());
        }
        assert_eq!(VoiceErrorCategory::KeyRejected.as_str(), "key rejected");
        assert_eq!(
            VoiceHttpError {
                category: VoiceErrorCategory::TimedOut
            }
            .to_string(),
            "timed out"
        );
    }
}
