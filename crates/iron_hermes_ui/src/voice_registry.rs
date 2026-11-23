//! Phase 54.7 Plan 02 (D-06) -- the single shared voice provider registry.
//!
//! One ungated, wasm-safe module is the ONLY source of voice provider ids,
//! labels, per-provider field sets, default model/voice/format, supported
//! formats and curated model/voice lists for STT, TTS and realtime. The wasm
//! client cannot import the native-only core/tools crates, so the data and the
//! client-side validators live here (precedent: `skills_import_api.rs`'s
//! ungated `classify_import_source`).
//!
//! Open Question 3 (RESEARCH Pitfall 6): Edge and ElevenLabs hard-code MP3 and
//! ignore `output_format`, so they list exactly one format (`mp3`).

// Later plans (08, 12-15, 18) consume most of this surface; until then the
// native and wasm builds both see unused items.
#![allow(dead_code)]

/// Which voice pipeline a provider belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceFamily {
    Stt,
    Tts,
    Realtime,
}

/// One selectable provider and everything the panel needs to render its form.
#[derive(Clone, Copy, Debug)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub family: VoiceFamily,
    /// Base URL / Path / API key reference fields (only `openai_compatible`).
    pub endpoint_fields: bool,
    pub model_field: bool,
    pub voice_field: bool,
    pub formats: &'static [&'static str],
    pub default_model: &'static str,
    pub default_voice: &'static str,
    pub default_format: &'static str,
    pub default_path: &'static str,
    pub curated_models: &'static [&'static str],
    pub curated_voices: &'static [&'static str],
    pub discovers_models: bool,
    pub discovers_voices: bool,
}

/// Neutral base for the const tables below (`..BASE` keeps each entry short).
const BASE: ProviderSpec = ProviderSpec {
    id: "",
    label: "",
    family: VoiceFamily::Stt,
    endpoint_fields: false,
    model_field: false,
    voice_field: false,
    formats: &[],
    default_model: "",
    default_voice: "",
    default_format: "",
    default_path: "",
    curated_models: &[],
    curated_voices: &[],
    discovers_models: false,
    discovers_voices: false,
};

pub const STT_PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "auto",
        label: "Auto",
        family: VoiceFamily::Stt,
        ..BASE
    },
    ProviderSpec {
        id: "groq",
        label: "Groq",
        family: VoiceFamily::Stt,
        model_field: true,
        default_model: "whisper-large-v3-turbo",
        curated_models: &["whisper-large-v3-turbo", "whisper-large-v3"],
        ..BASE
    },
    ProviderSpec {
        id: "openai",
        label: "OpenAI",
        family: VoiceFamily::Stt,
        model_field: true,
        default_model: "whisper-1",
        curated_models: &["whisper-1", "gpt-4o-mini-transcribe", "gpt-4o-transcribe"],
        ..BASE
    },
    ProviderSpec {
        id: "openai_compatible",
        label: "OpenAI-compatible",
        family: VoiceFamily::Stt,
        endpoint_fields: true,
        model_field: true,
        default_model: "whisper-1",
        default_path: "/audio/transcriptions",
        curated_models: &["whisper-1", "whisper-large-v3", "whisper-large-v3-turbo"],
        discovers_models: true,
        ..BASE
    },
];

pub const TTS_PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "edge",
        label: "Edge",
        family: VoiceFamily::Tts,
        voice_field: true,
        formats: &["mp3"],
        default_voice: "en-US-AriaNeural",
        default_format: "mp3",
        curated_voices: &["en-US-AriaNeural", "en-US-GuyNeural", "en-GB-SoniaNeural"],
        discovers_voices: true,
        ..BASE
    },
    ProviderSpec {
        id: "elevenlabs",
        label: "ElevenLabs",
        family: VoiceFamily::Tts,
        model_field: true,
        voice_field: true,
        formats: &["mp3"],
        default_model: "eleven_multilingual_v2",
        default_voice: "pNInz6obpgDQGcFmaJgB",
        default_format: "mp3",
        curated_models: &[
            "eleven_multilingual_v2",
            "eleven_turbo_v2_5",
            "eleven_flash_v2_5",
        ],
        curated_voices: &["pNInz6obpgDQGcFmaJgB"],
        discovers_models: true,
        discovers_voices: true,
        ..BASE
    },
    ProviderSpec {
        id: "openai",
        label: "OpenAI",
        family: VoiceFamily::Tts,
        model_field: true,
        voice_field: true,
        formats: &["mp3", "opus", "wav"],
        default_model: "tts-1",
        default_voice: "alloy",
        default_format: "mp3",
        curated_models: &["tts-1", "tts-1-hd", "gpt-4o-mini-tts"],
        curated_voices: &[
            "alloy", "ash", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer",
        ],
        ..BASE
    },
    ProviderSpec {
        id: "openai_compatible",
        label: "OpenAI-compatible",
        family: VoiceFamily::Tts,
        endpoint_fields: true,
        model_field: true,
        voice_field: true,
        formats: &["mp3", "opus", "wav"],
        default_model: "tts-1",
        default_voice: "alloy",
        default_format: "mp3",
        default_path: "/audio/speech",
        curated_models: &["tts-1", "kokoro"],
        curated_voices: &["alloy", "af_heart", "af_bella", "am_adam"],
        discovers_models: true,
        discovers_voices: true,
    },
];

pub const REALTIME_PROVIDERS: &[ProviderSpec] = &[ProviderSpec {
    id: "openai_compatible",
    label: "OpenAI Realtime-compatible",
    family: VoiceFamily::Realtime,
    endpoint_fields: true,
    model_field: true,
    voice_field: true,
    default_model: "gpt-realtime",
    default_voice: "shimmer",
    curated_models: &[
        "gpt-realtime",
        "gpt-realtime-2",
        "gpt-realtime-2.1",
        "gpt-realtime-1.5",
    ],
    curated_voices: &[
        "alloy", "ash", "ballad", "cedar", "coral", "echo", "marin", "sage", "shimmer", "verse",
    ],
    // Model discovery only for a custom base_url -- the caller decides.
    discovers_models: true,
    discovers_voices: false,
    ..BASE
}];

/// Realtime input-transcription model choices; `off` disables transcription.
pub const REALTIME_TRANSCRIPTION_MODELS: &[&str] = &[
    "gpt-4o-mini-transcribe",
    "gpt-4o-transcribe",
    "whisper-1",
    "off",
];

pub const REALTIME_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

fn find(list: &'static [ProviderSpec], id: &str) -> Option<&'static ProviderSpec> {
    list.iter().find(|s| s.id == id)
}

pub fn stt_provider(id: &str) -> Option<&'static ProviderSpec> {
    find(STT_PROVIDERS, id)
}

pub fn tts_provider(id: &str) -> Option<&'static ProviderSpec> {
    find(TTS_PROVIDERS, id)
}

pub fn realtime_provider(id: &str) -> Option<&'static ProviderSpec> {
    find(REALTIME_PROVIDERS, id)
}

// ── Client-side validation (D-02, D-04, D-11) ───────────────────────────────
//
// Advisory only: the server re-validates every field on save (plans 06, 07,
// 11). The rules mirror the native core crate's `voice_endpoint` module byte
// for byte; plan 07 asserts parity over one fixture table (T-54.7-09).

pub const BASE_URL_MAX: usize = 2048;
pub const PATH_MAX: usize = 256;
pub const MODEL_ID_MAX: usize = 128;
pub const VOICE_ID_MAX: usize = 64;
pub const KEY_REF_MAX: usize = 128;
pub const KEY_REF_NAME_MAX: usize = 64;
pub const KEY_REF_ENV_PREFIX: &str = "VOICE_KEY_";
pub const KEY_REF_VAULT_PREFIX: &str = "secret/voice/";
/// Reserved realtime transcription value that disables input transcription.
pub const TRANSCRIPTION_OFF: &str = "off";

/// Shared hint under every "API key reference" field (plans 12, 14, 18).
pub const KEY_REF_HINT: &str = "Name of an environment variable that starts with VOICE_KEY_, or a vault entry under secret/voice/, that holds the key. Leave empty if the server needs no key.";

/// One client-side validation failure. `copy()` is the exact UI-SPEC error
/// line; `code()` / `from_code()` carry it over the `voice_invalid:{field}:{code}`
/// server save error (plan 06).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    BaseUrlEmpty,
    BaseUrlScheme,
    BaseUrlUserinfo,
    /// A query string or fragment in the Base URL (WR-07).
    BaseUrlShape,
    BaseUrlTooLong,
    PathShape,
    KeyRefLooksSecret,
    KeyRefNamespace,
    ModelCharset,
    VoiceCharset,
}

impl FieldError {
    const ALL: [FieldError; 10] = [
        FieldError::BaseUrlEmpty,
        FieldError::BaseUrlScheme,
        FieldError::BaseUrlUserinfo,
        FieldError::BaseUrlShape,
        FieldError::BaseUrlTooLong,
        FieldError::PathShape,
        FieldError::KeyRefLooksSecret,
        FieldError::KeyRefNamespace,
        FieldError::ModelCharset,
        FieldError::VoiceCharset,
    ];

    /// Exact UI-SPEC error copy. An invalid key-reference shape and a
    /// secret-looking value share the "looks like the key itself" line.
    pub fn copy(&self) -> &'static str {
        match self {
            FieldError::BaseUrlEmpty => "Enter the server's address to use this provider.",
            FieldError::BaseUrlScheme => "Enter a full address starting with http:// or https://.",
            FieldError::BaseUrlUserinfo => {
                "Remove the username and password from the address. Put the key in \"API key reference\" instead."
            }
            FieldError::BaseUrlShape => {
                "Remove the ? or # part from the address. Put the path in the Path field instead."
            }
            FieldError::BaseUrlTooLong => "That address is too long.",
            FieldError::PathShape => "Start the path with / and leave out any ? or # part.",
            FieldError::KeyRefLooksSecret => {
                "That looks like the key itself. Enter the name of an environment variable or vault entry instead."
            }
            FieldError::KeyRefNamespace => {
                "Use an environment variable that starts with VOICE_KEY_ or a vault entry under secret/voice/."
            }
            FieldError::ModelCharset => {
                "Model names can use letters, numbers and . _ - : / only, up to 128 characters."
            }
            FieldError::VoiceCharset => {
                "Voice names can use letters, numbers and . _ - : only, up to 64 characters."
            }
        }
    }

    /// Stable snake_case wire code (never changes copy-side wording).
    pub fn code(&self) -> &'static str {
        match self {
            FieldError::BaseUrlEmpty => "base_url_empty",
            FieldError::BaseUrlScheme => "base_url_scheme",
            FieldError::BaseUrlUserinfo => "base_url_userinfo",
            FieldError::BaseUrlShape => "base_url_shape",
            FieldError::BaseUrlTooLong => "base_url_too_long",
            FieldError::PathShape => "path_shape",
            FieldError::KeyRefLooksSecret => "key_ref_looks_secret",
            FieldError::KeyRefNamespace => "key_ref_namespace",
            FieldError::ModelCharset => "model_charset",
            FieldError::VoiceCharset => "voice_charset",
        }
    }

    pub fn from_code(code: &str) -> Option<FieldError> {
        FieldError::ALL.iter().copied().find(|e| e.code() == code)
    }
}

/// Base URL: http/https only, no userinfo, <= 2048 chars. `required` turns an
/// empty value into `BaseUrlEmpty` (an `openai_compatible` provider needs one).
pub fn validate_base_url(raw: &str, required: bool) -> Result<(), FieldError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return if required {
            Err(FieldError::BaseUrlEmpty)
        } else {
            Ok(())
        };
    }
    if raw.chars().count() > BASE_URL_MAX {
        return Err(FieldError::BaseUrlTooLong);
    }
    let parsed = url::Url::parse(raw).map_err(|_| FieldError::BaseUrlScheme)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(FieldError::BaseUrlScheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(FieldError::BaseUrlUserinfo);
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(FieldError::BaseUrlShape);
    }
    Ok(())
}

/// Endpoint path: empty (use the default) or `/`-prefixed with no whitespace,
/// `?` or `#`, <= 256 chars.
pub fn validate_endpoint_path(raw: &str) -> Result<(), FieldError> {
    if raw.is_empty() {
        return Ok(());
    }
    let ok = raw.starts_with('/')
        && raw.chars().count() <= PATH_MAX
        && !raw
            .chars()
            .any(|c| c.is_whitespace() || c == '?' || c == '#');
    if ok {
        Ok(())
    } else {
        Err(FieldError::PathShape)
    }
}

/// Key reference (optional, D-02). Shape `[A-Za-z0-9_./:-]{1,128}` that does
/// not start like a real key (`sk-`, `gsk_`, `xi-`, `Bearer`, case-insensitive),
/// AND inside the voice namespace (T-54.7-70): `VOICE_KEY_[A-Z0-9_]{1,64}` or
/// `secret/voice/[A-Za-z0-9_.-]{1,64}`.
pub fn validate_key_ref(raw: &str) -> Result<(), FieldError> {
    if raw.is_empty() {
        return Ok(());
    }
    let shape_ok = raw.chars().count() <= KEY_REF_MAX
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '-'));
    let lower = raw.to_ascii_lowercase();
    let looks_secret = ["sk-", "gsk_", "xi-", "bearer"]
        .iter()
        .any(|p| lower.starts_with(p));
    if !shape_ok || looks_secret {
        return Err(FieldError::KeyRefLooksSecret);
    }
    let in_namespace = if let Some(name) = raw.strip_prefix(KEY_REF_ENV_PREFIX) {
        (1..=KEY_REF_NAME_MAX).contains(&name.chars().count())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    } else if let Some(name) = raw.strip_prefix(KEY_REF_VAULT_PREFIX) {
        // `..` is refused so a vault entry name can never climb out of the
        // `secret/voice/` prefix (parity with core `voice_endpoint`, plan 07).
        (1..=KEY_REF_NAME_MAX).contains(&name.chars().count())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            && !name.contains("..")
    } else {
        false
    };
    if in_namespace {
        Ok(())
    } else {
        Err(FieldError::KeyRefNamespace)
    }
}

/// Model id: `[A-Za-z0-9._:/-]{1,128}`, no `..`, no leading `/`.
pub fn validate_model_id(raw: &str) -> Result<(), FieldError> {
    let ok = (1..=MODEL_ID_MAX).contains(&raw.chars().count())
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
        && !raw.contains("..")
        && !raw.starts_with('/');
    if ok {
        Ok(())
    } else {
        Err(FieldError::ModelCharset)
    }
}

/// Voice id: `[A-Za-z0-9._:-]{1,64}`.
pub fn validate_voice_id(raw: &str) -> Result<(), FieldError> {
    let ok = (1..=VOICE_ID_MAX).contains(&raw.chars().count())
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
    if ok {
        Ok(())
    } else {
        Err(FieldError::VoiceCharset)
    }
}

/// Realtime transcription model: a model id, or the reserved `off`.
pub fn validate_transcription_model(raw: &str) -> Result<(), FieldError> {
    if raw == TRANSCRIPTION_OFF {
        return Ok(());
    }
    validate_model_id(raw)
}

// ── Error categories, option-list status copy, key pills (UI-SPEC) ─────────

pub const CATEGORY_SERVER_UNREACHABLE: &str = "server unreachable";
pub const CATEGORY_TIMED_OUT: &str = "timed out";
pub const CATEGORY_KEY_REJECTED: &str = "key rejected";
pub const CATEGORY_UNEXPECTED_RESPONSE: &str = "unexpected response";
pub const CATEGORY_MODEL_NOT_AVAILABLE: &str = "model not available";
pub const CATEGORY_ADDRESS_NOT_ALLOWED: &str = "address not allowed";
pub const CATEGORY_BROWSER_UNREACHABLE: &str = "this browser can't reach the address";

/// The only strings allowed in a `{category}` slot, in UI-SPEC order. Raw
/// error text, credentialed URLs and response bodies are never shown.
pub const ERROR_CATEGORIES: [&str; 7] = [
    CATEGORY_SERVER_UNREACHABLE,
    CATEGORY_TIMED_OUT,
    CATEGORY_KEY_REJECTED,
    CATEGORY_UNEXPECTED_RESPONSE,
    CATEGORY_MODEL_NOT_AVAILABLE,
    CATEGORY_ADDRESS_NOT_ALLOWED,
    CATEGORY_BROWSER_UNREACHABLE,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    Models,
    Voices,
}

/// State of an option list (UI-SPEC "Option lists: fetch, cache, fallback").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListStatus {
    Curated,
    Loading,
    Loaded(usize),
    LoadedEmpty,
    /// Carries one of [`ERROR_CATEGORIES`].
    Failed(&'static str),
}

/// The status line under a model/voice combobox.
pub fn list_status_copy(status: &ListStatus, kind: OptionKind, provider_label: &str) -> String {
    let (plural, singular) = match kind {
        OptionKind::Models => ("models", "model"),
        OptionKind::Voices => ("voices", "voice"),
    };
    match status {
        ListStatus::Curated => {
            format!("Built-in {plural} for {provider_label}. Type any value to use it.")
        }
        ListStatus::Loading => format!("Loading {plural}…"),
        ListStatus::Loaded(n) => {
            let noun = if *n == 1 { singular } else { plural };
            format!("{n} {noun} loaded from your server.")
        }
        ListStatus::LoadedEmpty => {
            format!("This server didn't list any {plural}. Type one to use it.")
        }
        ListStatus::Failed(category) => format!(
            "Couldn't load {plural}: {category}. Showing built-in defaults. You can still type any value."
        ),
    }
}

pub const KEY_STATUS_FOUND: &str = "found";
pub const KEY_STATUS_NOT_FOUND: &str = "not_found";
pub const KEY_STATUS_NONE: &str = "none";

/// `(pill text, pill modifier class)` for a key-reference status. Unknown
/// statuses read as `NO KEY` -- the pill never shows the reference or key.
pub fn key_pill(status: &str) -> (&'static str, &'static str) {
    match status {
        KEY_STATUS_FOUND => ("FOUND", "is-active"),
        KEY_STATUS_NOT_FOUND => ("NOT FOUND", "is-warn"),
        _ => ("NO KEY", "is-unavailable"),
    }
}

/// Test-only: the hard-coded voice defaults that must never reappear as
/// inline literals in the voice panel files (D-06). Needles are assembled from
/// fragments so a source-text test over a file cannot match its own needle
/// list. Reused by the no-hard-coded-defaults tests of `voice_settings.rs`
/// (plan 02), `voice_turn_based.rs` / `voice_realtime.rs` (plan 08),
/// `voice_fields.rs` (plan 18) and `voice_test_controls.rs` (plan 12).
#[cfg(test)]
pub(crate) fn hardcoded_voice_default_needles() -> Vec<String> {
    vec![
        ["whisper", "-1"].concat(),
        ["whisper-large", "-v3-turbo"].concat(),
        ["en-US-", "AriaNeural"].concat(),
        ["\"al", "loy\""].concat(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_every_provider_once() {
        let ids = |l: &[ProviderSpec]| l.iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids(STT_PROVIDERS),
            vec!["auto", "groq", "openai", "openai_compatible"]
        );
        assert_eq!(
            ids(TTS_PROVIDERS),
            vec!["edge", "elevenlabs", "openai", "openai_compatible"]
        );
        assert_eq!(ids(REALTIME_PROVIDERS), vec!["openai_compatible"]);
        let labels = |l: &[ProviderSpec]| l.iter().map(|s| s.label).collect::<Vec<_>>();
        assert_eq!(
            labels(STT_PROVIDERS),
            vec!["Auto", "Groq", "OpenAI", "OpenAI-compatible"]
        );
        assert_eq!(
            labels(TTS_PROVIDERS),
            vec!["Edge", "ElevenLabs", "OpenAI", "OpenAI-compatible"]
        );
        assert_eq!(
            labels(REALTIME_PROVIDERS),
            vec!["OpenAI Realtime-compatible"]
        );
        for list in [STT_PROVIDERS, TTS_PROVIDERS, REALTIME_PROVIDERS] {
            let mut seen = std::collections::HashSet::new();
            for s in list {
                assert!(seen.insert(s.id), "duplicate id {}", s.id);
            }
        }
        assert!(stt_provider("groq").is_some());
        assert!(tts_provider("edge").is_some());
        assert!(realtime_provider("openai_compatible").is_some());
        assert!(tts_provider("nope").is_none());
    }

    #[test]
    fn registry_defaults_and_formats_per_provider() {
        let edge = tts_provider("edge").unwrap();
        assert!(!edge.model_field);
        assert_eq!(edge.formats, &["mp3"]);
        assert_eq!(edge.default_voice, "en-US-AriaNeural");
        let eleven = tts_provider("elevenlabs").unwrap();
        assert_eq!(eleven.formats, &["mp3"]);
        for id in ["openai", "openai_compatible"] {
            let s = tts_provider(id).unwrap();
            assert_eq!(s.formats, &["mp3", "opus", "wav"], "{id}");
            assert_eq!(s.default_format, "mp3", "{id}");
        }
        let compat_tts = tts_provider("openai_compatible").unwrap();
        assert_eq!(compat_tts.default_path, "/audio/speech");
        let compat_stt = stt_provider("openai_compatible").unwrap();
        assert_eq!(compat_stt.default_path, "/audio/transcriptions");
        for list in [STT_PROVIDERS, TTS_PROVIDERS, REALTIME_PROVIDERS] {
            for s in list {
                assert_eq!(
                    s.endpoint_fields,
                    s.id == "openai_compatible",
                    "endpoint_fields only for openai_compatible ({})",
                    s.id
                );
            }
        }
        assert_eq!(
            stt_provider("groq").unwrap().default_model,
            "whisper-large-v3-turbo"
        );
        assert_eq!(stt_provider("openai").unwrap().default_model, "whisper-1");
        let rt = &REALTIME_PROVIDERS[0];
        assert_eq!(rt.default_model, "gpt-realtime");
        assert_eq!(rt.default_voice, "shimmer");
        assert!(rt.curated_voices.contains(&"marin"));
        assert_eq!(
            REALTIME_TRANSCRIPTION_MODELS,
            &[
                "gpt-4o-mini-transcribe",
                "gpt-4o-transcribe",
                "whisper-1",
                "off"
            ]
        );
        assert_eq!(REALTIME_DEFAULT_BASE_URL, "https://api.openai.com/v1");
    }

    #[test]
    fn voice_registry_imports_no_native_crate() {
        let src = include_str!("voice_registry.rs");
        // Needles are assembled from fragments so this test cannot match itself.
        let core = ["ironhermes", "_core"].concat();
        let tools = ["ironhermes", "_tools"].concat();
        assert!(
            !src.contains(&core),
            "registry must not import the native core crate"
        );
        assert!(
            !src.contains(&tools),
            "registry must not import the native tools crate"
        );
    }

    #[test]
    fn client_base_url_rules_and_copy() {
        let copy = |r: Result<(), FieldError>| r.unwrap_err().copy();
        assert_eq!(
            copy(validate_base_url("ftp://h", true)),
            "Enter a full address starting with http:// or https://."
        );
        assert_eq!(
            copy(validate_base_url("not a url", true)),
            "Enter a full address starting with http:// or https://."
        );
        assert_eq!(
            copy(validate_base_url("http://u:p@h/v1", true)),
            "Remove the username and password from the address. Put the key in \"API key reference\" instead."
        );
        assert_eq!(
            copy(validate_base_url("http://u@h/v1", true)),
            "Remove the username and password from the address. Put the key in \"API key reference\" instead."
        );
        // WR-07: query / fragment are rejected with their own line.
        for bad in ["http://h/v1?x=1", "http://h/v1#f", "http://h/v1?api_key=abc"] {
            assert_eq!(
                validate_base_url(bad, true).unwrap_err(),
                FieldError::BaseUrlShape,
                "{bad}"
            );
        }
        assert_eq!(
            FieldError::BaseUrlShape.copy(),
            "Remove the ? or # part from the address. Put the path in the Path field instead."
        );
        let long = format!("http://h/{}", "a".repeat(2049));
        assert_eq!(
            copy(validate_base_url(&long, true)),
            "That address is too long."
        );
        assert_eq!(
            copy(validate_base_url("", true)),
            "Enter the server's address to use this provider."
        );
        assert!(validate_base_url("", false).is_ok());
        assert!(validate_base_url("http://10.0.0.5:8880/v1", true).is_ok());
        assert!(validate_base_url("http://localhost:8880/v1", true).is_ok());
        assert!(validate_base_url("https://api.openai.com/v1", true).is_ok());
    }

    #[test]
    fn client_path_rules_and_copy() {
        assert!(validate_endpoint_path("").is_ok());
        assert!(validate_endpoint_path("/inference").is_ok());
        for bad in [
            "inference",
            "/a?b",
            "/a b",
            "/a#b",
            &format!("/{}", "a".repeat(256)),
        ] {
            assert_eq!(
                validate_endpoint_path(bad).unwrap_err().copy(),
                "Start the path with / and leave out any ? or # part.",
                "{bad}"
            );
        }
    }

    #[test]
    fn client_key_ref_rules_and_copy() {
        for ok in ["VOICE_KEY_KOKORO", "secret/voice/kokoro", ""] {
            assert!(validate_key_ref(ok).is_ok(), "{ok}");
        }
        let secret_copy = "That looks like the key itself. Enter the name of an environment variable or vault entry instead.";
        for bad in [
            "sk-abc",
            "GSK_abc",
            "gsk_abc",
            "xi-abc",
            "Bearer abc",
            "bearer abc",
        ] {
            let e = validate_key_ref(bad).unwrap_err();
            assert_eq!(e, FieldError::KeyRefLooksSecret, "{bad}");
            assert_eq!(e.copy(), secret_copy, "{bad}");
        }
        let ns_copy = "Use an environment variable that starts with VOICE_KEY_ or a vault entry under secret/voice/.";
        for bad in [
            "KOKORO_API_KEY",
            "kokoro",
            "OPENAI_API_KEY",
            "VOICE_TOOLS_OPENAI_KEY",
            "secret/providers/openai",
            "secret/voice/a/b",
            "VOICE_KEY_",
        ] {
            let e = validate_key_ref(bad).unwrap_err();
            assert_eq!(e, FieldError::KeyRefNamespace, "{bad}");
            assert_eq!(e.copy(), ns_copy, "{bad}");
        }
        assert_eq!(
            validate_key_ref(&format!("VOICE_KEY_{}", "A".repeat(65))).unwrap_err(),
            FieldError::KeyRefNamespace
        );
        assert!(validate_key_ref(&format!("VOICE_KEY_{}", "A".repeat(64))).is_ok());
        assert_eq!(
            KEY_REF_HINT,
            "Name of an environment variable that starts with VOICE_KEY_, or a vault entry under secret/voice/, that holds the key. Leave empty if the server needs no key."
        );
    }

    #[test]
    fn client_model_and_voice_rules_and_copy() {
        let model_copy =
            "Model names can use letters, numbers and . _ - : / only, up to 128 characters.";
        let voice_copy =
            "Voice names can use letters, numbers and . _ - : only, up to 64 characters.";
        for ok in [
            "gpt-realtime-2.1",
            "mlx-community/Kokoro-82M-bf16",
            "whisper-large-v3",
        ] {
            assert!(validate_model_id(ok).is_ok(), "{ok}");
        }
        for bad in [
            "a\"b",
            "a\\b",
            "a..b",
            "/lead",
            "a\nb",
            "",
            &"m".repeat(129),
        ] {
            assert_eq!(
                validate_model_id(bad).unwrap_err().copy(),
                model_copy,
                "{bad:?}"
            );
        }
        assert!(validate_model_id(&"m".repeat(128)).is_ok());
        for ok in ["marin", "af_heart", "en-US-AriaNeural"] {
            assert!(validate_voice_id(ok).is_ok(), "{ok}");
        }
        for bad in ["a/b", "a b", "", &"v".repeat(65)] {
            assert_eq!(
                validate_voice_id(bad).unwrap_err().copy(),
                voice_copy,
                "{bad:?}"
            );
        }
        assert!(validate_transcription_model(TRANSCRIPTION_OFF).is_ok());
        assert!(validate_transcription_model("gpt-4o-mini-transcribe").is_ok());
        assert!(validate_transcription_model("a b").is_err());
        // `off` is reserved for transcription only; it is also a valid model id charset-wise.
        assert!(validate_model_id("off").is_ok());
    }

    #[test]
    fn field_error_code_round_trips() {
        let all = [
            FieldError::BaseUrlEmpty,
            FieldError::BaseUrlScheme,
            FieldError::BaseUrlUserinfo,
            FieldError::BaseUrlShape,
            FieldError::BaseUrlTooLong,
            FieldError::PathShape,
            FieldError::KeyRefLooksSecret,
            FieldError::KeyRefNamespace,
            FieldError::ModelCharset,
            FieldError::VoiceCharset,
        ];
        let mut codes = std::collections::HashSet::new();
        for v in all {
            assert_eq!(FieldError::from_code(v.code()), Some(v));
            assert!(codes.insert(v.code()), "duplicate code {}", v.code());
        }
        assert_eq!(FieldError::BaseUrlScheme.code(), "base_url_scheme");
        assert_eq!(FieldError::BaseUrlShape.code(), "base_url_shape");
        assert_eq!(FieldError::KeyRefNamespace.code(), "key_ref_namespace");
        assert_eq!(FieldError::from_code("nope"), None);
    }

    #[test]
    fn category_vocabulary_is_the_seven_ui_spec_strings() {
        assert_eq!(
            ERROR_CATEGORIES,
            [
                "server unreachable",
                "timed out",
                "key rejected",
                "unexpected response",
                "model not available",
                "address not allowed",
                "this browser can't reach the address",
            ]
        );
        assert_eq!(CATEGORY_SERVER_UNREACHABLE, "server unreachable");
        assert_eq!(
            CATEGORY_BROWSER_UNREACHABLE,
            "this browser can't reach the address"
        );
    }

    #[test]
    fn list_status_copy_pluralises() {
        use ListStatus::*;
        use OptionKind::*;
        assert_eq!(
            list_status_copy(&Loaded(1), Models, "x"),
            "1 model loaded from your server."
        );
        assert_eq!(
            list_status_copy(&Loaded(3), Voices, "x"),
            "3 voices loaded from your server."
        );
        assert_eq!(
            list_status_copy(&Loaded(1), Voices, "x"),
            "1 voice loaded from your server."
        );
        assert_eq!(
            list_status_copy(&LoadedEmpty, Voices, "x"),
            "This server didn't list any voices. Type one to use it."
        );
        assert_eq!(
            list_status_copy(&Failed("timed out"), Models, "x"),
            "Couldn't load models: timed out. Showing built-in defaults. You can still type any value."
        );
        assert_eq!(
            list_status_copy(&Curated, Models, "Groq"),
            "Built-in models for Groq. Type any value to use it."
        );
        assert_eq!(list_status_copy(&Loading, Voices, "x"), "Loading voices…");
    }

    #[test]
    fn key_status_pill_text() {
        assert_eq!(key_pill(KEY_STATUS_FOUND), ("FOUND", "is-active"));
        assert_eq!(key_pill(KEY_STATUS_NOT_FOUND), ("NOT FOUND", "is-warn"));
        assert_eq!(key_pill(KEY_STATUS_NONE), ("NO KEY", "is-unavailable"));
        assert_eq!(key_pill("garbage"), ("NO KEY", "is-unavailable"));
    }
}
