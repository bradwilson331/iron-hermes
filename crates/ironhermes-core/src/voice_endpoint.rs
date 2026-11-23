//! Phase 54.7 (D-01/D-03/D-04/D-11): pure endpoint, charset and key-reference
//! validators for the configurable voice services. No I/O lives here.

/// Default path joined onto a TTS `base_url` when no `path` override is set (D-03).
pub const TTS_DEFAULT_PATH: &str = "/audio/speech";
/// Default path joined onto an STT `base_url` when no `path` override is set (D-03).
pub const STT_DEFAULT_PATH: &str = "/audio/transcriptions";
/// Maximum accepted `base_url` length in characters (D-04).
pub const BASE_URL_MAX: usize = 2048;
/// Maximum accepted endpoint `path` length in characters (D-03).
pub const PATH_MAX: usize = 256;
/// Maximum accepted model id length in characters (D-11).
pub const MODEL_ID_MAX: usize = 128;
/// Maximum accepted voice id length in characters (D-11).
pub const VOICE_ID_MAX: usize = 64;
/// Maximum accepted key-reference length in characters (D-02).
pub const KEY_REF_MAX: usize = 128;
/// Reserved transcription-model literal that disables input transcription (D-11).
pub const TRANSCRIPTION_OFF: &str = "off";
/// Environment-variable key references must start with this prefix (T-54.7-70).
pub const KEY_REF_ENV_PREFIX: &str = "VOICE_KEY_";
/// Vault key references must start with this prefix (T-54.7-70).
pub const KEY_REF_VAULT_PREFIX: &str = "secret/voice/";
/// Maximum length of the name part after a key-reference prefix.
pub const KEY_REF_NAME_MAX: usize = 64;

/// Why an operator-supplied voice field was rejected. Messages are short and
/// field-tagged and never echo the rejected value (it may be a secret).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceFieldError {
    BaseUrlScheme,
    BaseUrlUserinfo,
    /// A query string or fragment (WR-07): `join_endpoint` appends the path to the
    /// base by concatenation, so `?x=1` / `#f` would corrupt the endpoint, and a
    /// query is where credentials tend to get pasted.
    BaseUrlShape,
    BaseUrlTooLong,
    BaseUrlEmpty,
    PathShape,
    ModelCharset,
    VoiceCharset,
    KeyRefShape,
    /// A well-formed reference that is outside the `VOICE_KEY_*` /
    /// `secret/voice/*` namespace (T-54.7-70).
    KeyRefNamespace,
}

impl std::fmt::Display for VoiceFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::BaseUrlScheme => "base_url: must be an http or https URL",
            Self::BaseUrlUserinfo => "base_url: must not contain a username or password",
            Self::BaseUrlShape => "base_url: must not contain a query string or fragment",
            Self::BaseUrlTooLong => "base_url: too long",
            Self::BaseUrlEmpty => "base_url: required",
            Self::PathShape => "path: must start with / and contain no spaces, ? or #",
            Self::ModelCharset => "model: unsupported characters or length",
            Self::VoiceCharset => "voice: unsupported characters or length",
            Self::KeyRefShape => "api_key_ref: not a valid reference name",
            Self::KeyRefNamespace => {
                "api_key_ref: must be a VOICE_KEY_* variable or a secret/voice/* entry"
            }
        };
        f.write_str(msg)
    }
}

impl std::error::Error for VoiceFieldError {}

/// Validate an operator-typed base URL (D-04). Private, loopback and LAN hosts
/// are accepted deliberately; only scheme, userinfo, query/fragment and length are policed.
pub fn validate_base_url(raw: &str) -> Result<url::Url, VoiceFieldError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(VoiceFieldError::BaseUrlEmpty);
    }
    if trimmed.chars().count() > BASE_URL_MAX {
        return Err(VoiceFieldError::BaseUrlTooLong);
    }
    let parsed = url::Url::parse(trimmed).map_err(|_| VoiceFieldError::BaseUrlScheme)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(VoiceFieldError::BaseUrlScheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(VoiceFieldError::BaseUrlUserinfo);
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(VoiceFieldError::BaseUrlShape);
    }
    Ok(parsed)
}

/// Validate an endpoint path override. Empty means "use the default path".
pub fn validate_endpoint_path(raw: &str) -> Result<(), VoiceFieldError> {
    if raw.is_empty() {
        return Ok(());
    }
    if !raw.starts_with('/')
        || raw.chars().count() > PATH_MAX
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c == '?' || c == '#')
    {
        return Err(VoiceFieldError::PathShape);
    }
    Ok(())
}

/// Join `base_url` and an optional path by plain string concatenation.
///
/// `Url::join` with a leading-slash path replaces the base path and would drop
/// a `/v1` segment, so it is never used here.
pub fn join_endpoint(
    base_url: &str,
    path: Option<&str>,
    default_path: &str,
) -> Result<url::Url, VoiceFieldError> {
    let base = validate_base_url(base_url)?;
    let effective = match path {
        Some(p) if !p.is_empty() => p,
        _ => default_path,
    };
    validate_endpoint_path(effective)?;
    let joined = format!("{}{}", base.as_str().trim_end_matches('/'), effective);
    // The concatenation of a validated http(s) base and a validated path
    // re-parses; a failure here is reported as a base-URL shape error.
    url::Url::parse(&joined).map_err(|_| VoiceFieldError::BaseUrlScheme)
}

/// Validate a model id (D-11): 1..=128 chars of ASCII alphanumerics and
/// `. _ : / -`, no `..`, no leading `/`.
pub fn validate_model_id(s: &str) -> Result<(), VoiceFieldError> {
    let len = s.chars().count();
    let charset_ok = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'));
    if len == 0 || len > MODEL_ID_MAX || !charset_ok || s.contains("..") || s.starts_with('/') {
        return Err(VoiceFieldError::ModelCharset);
    }
    Ok(())
}

/// Validate a voice id (D-11): 1..=64 chars of ASCII alphanumerics and `. _ : -`.
pub fn validate_voice_id(s: &str) -> Result<(), VoiceFieldError> {
    let len = s.chars().count();
    let charset_ok = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
    if len == 0 || len > VOICE_ID_MAX || !charset_ok {
        return Err(VoiceFieldError::VoiceCharset);
    }
    Ok(())
}

/// Validate a realtime input-transcription model: the model gate plus the
/// reserved literal `off`.
pub fn validate_transcription_model(s: &str) -> Result<(), VoiceFieldError> {
    if s == TRANSCRIPTION_OFF {
        return Ok(());
    }
    validate_model_id(s)
}

/// Where a validated key reference is resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRefKind {
    /// A `VOICE_KEY_*` environment variable.
    Env,
    /// A `secret/voice/*` vault entry.
    Vault,
}

/// Shape then namespace, returning where the reference resolves from.
fn classify_key_ref(s: &str) -> Result<KeyRefKind, VoiceFieldError> {
    let len = s.chars().count();
    let charset_ok = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '-'));
    let lower = s.to_ascii_lowercase();
    let secret_shaped = ["sk-", "gsk_", "xi-", "bearer "]
        .iter()
        .any(|p| lower.starts_with(p));
    if len == 0 || len > KEY_REF_MAX || !charset_ok || secret_shaped {
        return Err(VoiceFieldError::KeyRefShape);
    }
    if let Some(name) = s.strip_prefix(KEY_REF_ENV_PREFIX) {
        let ok = (1..=KEY_REF_NAME_MAX).contains(&name.chars().count())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        return if ok {
            Ok(KeyRefKind::Env)
        } else {
            Err(VoiceFieldError::KeyRefNamespace)
        };
    }
    if let Some(name) = s.strip_prefix(KEY_REF_VAULT_PREFIX) {
        // `..` is refused so a vault entry name can never climb out of the
        // `secret/voice/` prefix on a path-structured backend.
        let ok = (1..=KEY_REF_NAME_MAX).contains(&name.chars().count())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            && !name.contains("..");
        return if ok {
            Ok(KeyRefKind::Vault)
        } else {
            Err(VoiceFieldError::KeyRefNamespace)
        };
    }
    Err(VoiceFieldError::KeyRefNamespace)
}

/// Validate a key REFERENCE name (D-02, T-54.7-70).
///
/// A reference is either an environment variable `VOICE_KEY_[A-Z0-9_]{1,64}`
/// or a vault entry `secret/voice/[A-Za-z0-9_.-]{1,64}`. The namespace is what
/// stops a reference from forwarding an unrelated secret (`OPENAI_API_KEY`, a
/// platform token, `secret/providers/*`) to an operator-typed `base_url`.
/// Secret-shaped values (`sk-`, `gsk_`, `xi-`, `Bearer `) are rejected as
/// `KeyRefShape` so a pasted key never lands in config.yaml.
pub fn validate_key_ref(s: &str) -> Result<(), VoiceFieldError> {
    classify_key_ref(s).map(|_| ())
}

/// The single source of the namespace rule: `None` when `validate_key_ref` fails.
pub fn key_ref_kind(s: &str) -> Option<KeyRefKind> {
    classify_key_ref(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_endpoint_trailing_slash_and_default() {
        let u = join_endpoint("http://tts.lan:8880/v1/", None, TTS_DEFAULT_PATH).unwrap();
        assert_eq!(u.as_str(), "http://tts.lan:8880/v1/audio/speech");
        let u = join_endpoint("http://stt.lan:8178", None, STT_DEFAULT_PATH).unwrap();
        assert_eq!(u.as_str(), "http://stt.lan:8178/audio/transcriptions");
        let u = join_endpoint("http://stt.lan:8178", Some(""), STT_DEFAULT_PATH).unwrap();
        assert_eq!(u.as_str(), "http://stt.lan:8178/audio/transcriptions");
    }

    #[test]
    fn endpoint_path_rejects_missing_slash_query_fragment() {
        assert!(validate_endpoint_path("").is_ok());
        assert!(validate_endpoint_path("/inference").is_ok());
        for bad in ["inference", "/a b", "/x?y", "/x#y"] {
            assert_eq!(
                validate_endpoint_path(bad),
                Err(VoiceFieldError::PathShape),
                "{bad}"
            );
        }
        let long = format!("/{}", "a".repeat(256));
        assert_eq!(
            validate_endpoint_path(&long),
            Err(VoiceFieldError::PathShape)
        );
    }

    #[test]
    fn validate_base_url_accepts_lan_and_loopback_hosts() {
        for ok in [
            "http://127.0.0.1:8880/v1",
            "http://10.0.0.5:8178",
            "http://tts.lan:8880/v1",
            "https://api.openai.com/v1",
        ] {
            assert!(validate_base_url(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn validate_base_url_rejects_scheme_userinfo_length() {
        assert_eq!(
            validate_base_url("ftp://h/v1").unwrap_err(),
            VoiceFieldError::BaseUrlScheme
        );
        assert_eq!(
            validate_base_url("file:///etc").unwrap_err(),
            VoiceFieldError::BaseUrlScheme
        );
        assert_eq!(
            validate_base_url("http://user:pw@h/v1").unwrap_err(),
            VoiceFieldError::BaseUrlUserinfo
        );
        assert_eq!(
            validate_base_url("http://user@h/v1").unwrap_err(),
            VoiceFieldError::BaseUrlUserinfo
        );
        let long = format!("http://h/{}", "a".repeat(2049 - "http://h/".len()));
        assert_eq!(long.chars().count(), 2049);
        assert_eq!(
            validate_base_url(&long).unwrap_err(),
            VoiceFieldError::BaseUrlTooLong
        );
        // WR-07: a query or fragment would be corrupted by path concatenation.
        for bad in [
            "http://h/v1?x=1",
            "http://h/v1?api_key=abc",
            "http://h/v1?",
            "http://h/v1#frag",
            "http://h/v1#",
            "http://h:8880?x=1",
        ] {
            assert_eq!(
                validate_base_url(bad).unwrap_err(),
                VoiceFieldError::BaseUrlShape,
                "{bad}"
            );
        }
        assert!(validate_base_url("http://h/v1/").is_ok());
        assert_eq!(
            validate_base_url("").unwrap_err(),
            VoiceFieldError::BaseUrlEmpty
        );
        assert_eq!(
            validate_base_url("not a url").unwrap_err(),
            VoiceFieldError::BaseUrlScheme
        );
    }

    #[test]
    fn model_id_gate_accepts_and_rejects() {
        for ok in [
            "gpt-realtime",
            "gpt-realtime-2.1",
            "mlx-community/Kokoro-82M-bf16",
            "whisper-large-v3",
            "eleven_multilingual_v2",
            &"a".repeat(128),
        ] {
            assert!(validate_model_id(ok).is_ok(), "{ok}");
        }
        for bad in [
            "a\"b",
            "a\\b",
            "a/../b",
            "/abs",
            "a\nb",
            "",
            &"a".repeat(129),
        ] {
            assert_eq!(
                validate_model_id(bad),
                Err(VoiceFieldError::ModelCharset),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn voice_id_gate_accepts_and_rejects() {
        for ok in [
            "marin",
            "af_heart",
            "en-US-AriaNeural",
            "pNInz6obpgDQGcFmaJgB",
            &"a".repeat(64),
        ] {
            assert!(validate_voice_id(ok).is_ok(), "{ok}");
        }
        for bad in ["a/b", "a b", "", &"a".repeat(65)] {
            assert_eq!(
                validate_voice_id(bad),
                Err(VoiceFieldError::VoiceCharset),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn transcription_model_gate_reserves_off() {
        for ok in ["off", "gpt-4o-mini-transcribe", "whisper-1"] {
            assert!(validate_transcription_model(ok).is_ok(), "{ok}");
        }
        for bad in ["of f", "\"off\""] {
            assert!(validate_transcription_model(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn key_ref_rejects_secret_shaped_values() {
        for ok in ["VOICE_KEY_KOKORO", "secret/voice/kokoro"] {
            assert!(validate_key_ref(ok).is_ok(), "{ok}");
        }
        for bad in [
            "sk-test",
            "GSK_x",
            "xi-abc",
            "bearer x",
            "a b",
            &"a".repeat(129),
            "",
        ] {
            assert_eq!(
                validate_key_ref(bad),
                Err(VoiceFieldError::KeyRefShape),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn key_ref_must_be_in_voice_namespace() {
        for bad in [
            "KOKORO_API_KEY",
            "kokoro",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "VOICE_TOOLS_OPENAI_KEY",
            "secret/providers/openai",
            "VOICE_KEY_",
            "voice_key_kokoro",
            "VOICE_KEY_a",
            "secret/voice/",
            "secret/voice/a/b",
            "secret/voice/..",
            "secret/voice/a..b",
        ] {
            assert_eq!(
                validate_key_ref(bad),
                Err(VoiceFieldError::KeyRefNamespace),
                "{bad:?}"
            );
            assert_eq!(key_ref_kind(bad), None, "{bad:?}");
        }
        let too_long_env = format!("VOICE_KEY_{}", "A".repeat(65));
        assert_eq!(
            validate_key_ref(&too_long_env),
            Err(VoiceFieldError::KeyRefNamespace)
        );
        assert!(validate_key_ref("VOICE_KEY_A1_B").is_ok());
        assert!(validate_key_ref("secret/voice/kokoro.tts").is_ok());
        assert_eq!(key_ref_kind("VOICE_KEY_A1_B"), Some(KeyRefKind::Env));
        assert_eq!(
            key_ref_kind("secret/voice/kokoro.tts"),
            Some(KeyRefKind::Vault)
        );
    }
}
