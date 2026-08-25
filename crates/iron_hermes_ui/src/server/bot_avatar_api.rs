//! Phase 50.1 Plan 04 (D-11/D-12): avatar upload and AI-portrait generation
//! server paths.
//!
//! Avatar bytes are stored in their own directory beneath the hermes home
//! (`bot_avatar_dir`/`bot_avatar_path` below), one file per generated
//! identifier — the bot-meta descriptor (`protocol::BotAvatarDescriptor`)
//! carries only that identifier, never the bytes. `save_bot_meta_impl`
//! already rejects an inline data-URL `image_id` (T-50.1-01-05); this file
//! is the separate asset path that rejection makes workable.
//!
//! **`upload_bot_avatar`** follows this crate's four-step server-fn
//! protocol (validate → fresh `Config::load()` → `check_profile_write_gate`
//! → `spawn_blocking`, mirrors `update_profile_config`/`bot_meta_api`'s
//! `save_bot_meta`) — the write itself is synchronous disk I/O, so it
//! belongs on a blocking-pool thread.
//!
//! **`generate_bot_avatar`** validates, loads config and checks the write
//! gate the same way, but does NOT wrap the network round-trip in
//! `spawn_blocking` — an HTTP request is naturally async I/O, and blocking
//! a `spawn_blocking` worker thread on a network wait for up to
//! `image_gen.timeout_secs` would starve that pool (mirrors
//! `ironhermes_tools::gen_backend`'s own async cache-write helpers, which
//! use `tokio::fs` rather than `spawn_blocking` for the same reason). It
//! resolves the fal/venice choice through `gen_backend::resolve` — the one
//! seam that already owns that decision (D-02) — and, on the Venice branch,
//! ALWAYS sends a positive `steps` value and the `safe_mode` flag
//! ([`resolve_generation_args`]): omitting `steps` is a live footgun this
//! project has already hit — Venice returns a sub-9KB unusable blob rather
//! than an error, so the failure looks like a corrupt image rather than a
//! bad request. A response below [`UNUSABLE_BLOB_THRESHOLD_BYTES`] is
//! classified as a failure and surfaced ([`classify_generation_bytes`]) —
//! never stored, never silently substituted with a blank/default avatar.
//!
//! Every failure path in this file — upload or generation — resolves to
//! one of two FIXED Copywriting Contract strings
//! ([`AVATAR_UPLOAD_REJECT_MSG`]/[`AVATAR_GENERATE_REJECT_MSG`]). No
//! provider response body, key, or raw I/O error ever reaches the browser
//! (T-50.1-04-05) — the same CR-05/CR-06 discipline this crate already
//! enforces on `profile_api.rs`'s `.env` parse errors.
//!
//! Uploaded and AI-generated avatars do not carry the work-state
//! animation — there is no eye element to widen on a raster image. This is
//! an accepted limitation (D-12), not a deferred item; the alternative
//! treatment for raster avatars is the deferred decorative-companion
//! subsystem, explicitly out of scope for this phase.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::path::{Path, PathBuf};

#[cfg(feature = "server")]
use base64::Engine;

use crate::protocol::BotAvatarSaved;
#[cfg(feature = "server")]
use crate::protocol::{BotAvatarGenerateRequest, BotAvatarUploadRequest};

/// Decoded byte-length cap for an uploaded avatar image — matches the
/// Copywriting Contract's locked "under 5MB" upload-failure copy. Smaller
/// than the chat-attachment image cap (20 MiB): an avatar is a small
/// profile photo, never a full-resolution attachment.
#[cfg(feature = "server")]
const AVATAR_UPLOAD_MAX_BYTES: usize = 5 * 1024 * 1024;

/// UI-SPEC Copywriting Contract, locked verbatim — the ONE upload-failure
/// string, used for every upload rejection reason (decode failure, empty
/// payload, oversize, and unsupported signature all collapse onto this one
/// fixed string; T-50.1-04-05 forbids interpolating the real reason).
#[cfg(feature = "server")]
const AVATAR_UPLOAD_REJECT_MSG: &str = "Could not read that image. Try a PNG or JPG under 5MB.";

/// UI-SPEC Copywriting Contract, locked verbatim — the ONE generation-
/// failure string, used for every generation rejection reason (missing
/// provider key, network failure, non-2xx response, and an unusably small
/// response all collapse onto this one fixed string; T-50.1-04-05).
#[cfg(feature = "server")]
const AVATAR_GENERATE_REJECT_MSG: &str =
    "Avatar generation failed. Check your image provider key and try again.";

/// D-12's binding default: `ImageGenConfig::default().steps` is `20`. A
/// generation request whose configured step count is `0` (Venice's
/// server-side "omit the field" sentinel, per `ImageGenConfig`'s own doc)
/// still gets a positive value here — this fn is what makes the request
/// builder's "always positive" guarantee hold regardless of config.
#[cfg(feature = "server")]
const DEFAULT_AVATAR_GEN_STEPS: u32 = 20;

/// Sub-9KB responses are the known Venice "blob" failure mode (D-12,
/// CONTEXT.md: an omitted/near-zero `steps` value returns a near-zero
/// denoise pass under this size). Any generated payload below this
/// threshold is classified as a failure rather than stored.
#[cfg(feature = "server")]
const UNUSABLE_BLOB_THRESHOLD_BYTES: usize = 9 * 1024;

/// Phase 50.1 Plan 04: the avatar storage root for one bot — its own
/// directory beneath the hermes home, never nested inside `profiles/` or
/// the bot-meta store's own files (D-11's "its own directory" instruction).
#[cfg(feature = "server")]
pub(crate) fn bot_avatar_dir(name: &str) -> PathBuf {
    ironhermes_core::get_hermes_home().join("bot-avatars").join(name)
}

/// Phase 50.1 Plan 04 (T-50.1-04-01): resolve one avatar's on-disk path.
/// The bot name is profile-name-validated and the identifier is confirmed
/// to be a single safe path leaf (`ironhermes_core::safe_attachment_leaf` —
/// the same traversal guard `chat_attachments_api.rs` uses for an opaque
/// per-upload id) BEFORE either is joined into a path. A traversal-shaped
/// name or identifier errors here, before any path is constructed or any
/// filesystem call is made.
#[cfg(feature = "server")]
pub(crate) fn bot_avatar_path(name: &str, image_id: &str) -> Result<PathBuf, String> {
    let validated_name = ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    let safe_id = ironhermes_core::safe_attachment_leaf(image_id)
        .ok_or_else(|| "invalid avatar identifier".to_string())?;
    Ok(bot_avatar_dir(&validated_name).join(safe_id))
}

/// Phase 50.1 Plan 06 (D-17): copy every stored avatar file from one bot's
/// avatar directory to another's — the "look" D-17 requires travel with a
/// clone. Reuses `profile_api::copy_dir_recursive` (this crate's one
/// recursive-copy implementation) rather than a second hand-rolled walk. A
/// source with no avatar directory at all (never uploaded/generated one —
/// the deterministic geometric default needs no file) is a no-op success.
#[cfg(feature = "server")]
pub(crate) fn copy_bot_avatar_files(source: &str, target: &str) -> Result<(), String> {
    let validated_source = ironhermes_core::profile::validate_profile_name(source)
        .map_err(|e| format!("invalid source profile name: {e}"))?;
    let validated_target = ironhermes_core::profile::validate_profile_name(target)
        .map_err(|e| format!("invalid target profile name: {e}"))?;

    let source_dir = bot_avatar_dir(&validated_source);
    if !source_dir.is_dir() {
        return Ok(());
    }
    let target_dir = bot_avatar_dir(&validated_target);
    crate::server::profile_api::copy_dir_recursive(&source_dir, &target_dir)
}

/// Sniff the leading bytes for one of four supported raster signatures —
/// a deliberate copy of `chat_attachment_route.rs`'s own sniffer (this
/// crate has no shared mime-sniff utility, matching the established
/// porting-not-sharing convention already used for
/// `chat_attachments_api::content_type_from_ext`). Trusting the
/// client-supplied content type instead would let a non-image payload
/// (e.g. an SVG carrying a `<script>`) masquerade as an accepted upload.
#[cfg(feature = "server")]
fn sniff_raster_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Write `bytes` to `final_path` atomically — temp file in the same
/// directory, then `rename` onto the final path. Same temp-then-rename
/// discipline `bot_meta_api::write_json_atomic` and
/// `profile_api::write_env_atomic_0600` already use for this crate's other
/// mutating writes.
#[cfg(feature = "server")]
fn write_avatar_bytes_atomic(final_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = final_path
        .parent()
        .ok_or_else(|| "avatar path resolution failed".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all: {e}"))?;
    let file_name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp_path = final_path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, bytes).map_err(|e| format!("write avatar: {e}"))?;
    std::fs::rename(&tmp_path, final_path).map_err(|e| format!("rename avatar: {e}"))
}

/// Phase 50.1 Plan 04: the `upload_bot_avatar` impl layer — decode, cap,
/// sniff, write. Every rejection returns the ONE fixed
/// [`AVATAR_UPLOAD_REJECT_MSG`] string and writes nothing.
#[cfg(feature = "server")]
fn upload_bot_avatar_impl(bot_name: &str, content_b64: &str) -> Result<BotAvatarSaved, String> {
    // Validate the name first — cheap, no I/O, and the same order
    // `bot_avatar_path` itself enforces.
    let validated_name = ironhermes_core::profile::validate_profile_name(bot_name)
        .map_err(|e| format!("invalid profile name: {e}"))?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content_b64.as_bytes())
        .map_err(|_| AVATAR_UPLOAD_REJECT_MSG.to_string())?;

    if bytes.is_empty() || bytes.len() > AVATAR_UPLOAD_MAX_BYTES {
        return Err(AVATAR_UPLOAD_REJECT_MSG.to_string());
    }
    // T-50.1-04-03: the leading bytes decide, never a client-supplied
    // content type (this request carries none anyway — the whole point).
    if sniff_raster_signature(&bytes).is_none() {
        return Err(AVATAR_UPLOAD_REJECT_MSG.to_string());
    }

    let image_id = uuid::Uuid::new_v4().to_string();
    let final_path = bot_avatar_path(&validated_name, &image_id)?;
    write_avatar_bytes_atomic(&final_path, &bytes)?;

    Ok(BotAvatarSaved { image_id })
}

/// Phase 50.1 Plan 04 (D-12): resolves the diffusion step count this fn
/// will send — ALWAYS positive, regardless of what the config carries. A
/// configured `0` (Venice's own "omit the field" sentinel for every OTHER
/// generation caller in this crate) still resolves to
/// [`DEFAULT_AVATAR_GEN_STEPS`] here, because avatar generation has no
/// caller-facing reason to ever omit the field — the omission is exactly
/// the footgun this fn exists to close. Pure and deterministic.
#[cfg(feature = "server")]
pub(crate) fn resolve_avatar_gen_steps(configured: u32) -> u32 {
    if configured == 0 {
        DEFAULT_AVATAR_GEN_STEPS
    } else {
        configured
    }
}

/// Phase 50.1 Plan 04 (D-12): the exact `(steps, safe_mode)` argument pair
/// this file passes to `VeniceClient::generate_image` — `steps` is ALWAYS
/// `Some(positive)`, never `None` and never `Some(0)`. Pure and
/// deterministic — the "generation request builder always sets a positive
/// step count and the safety flag" guarantee lives entirely in this fn.
#[cfg(feature = "server")]
pub(crate) fn resolve_generation_args(
    configured_steps: u32,
    configured_safe_mode: bool,
) -> (Option<u32>, bool) {
    (Some(resolve_avatar_gen_steps(configured_steps)), configured_safe_mode)
}

/// Phase 50.1 Plan 04 (D-12): classify a generated image's byte length.
/// Below [`UNUSABLE_BLOB_THRESHOLD_BYTES`] is the known Venice "blob"
/// failure mode — classified as a failure and surfaced via the fixed
/// [`AVATAR_GENERATE_REJECT_MSG`] string, never stored and never silently
/// substituted with a blank or default avatar. Pure and deterministic.
#[cfg(feature = "server")]
pub(crate) fn classify_generation_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < UNUSABLE_BLOB_THRESHOLD_BYTES {
        return Err(AVATAR_GENERATE_REJECT_MSG.to_string());
    }
    Ok(())
}

/// Phase 50.1 Plan 04: the `generate_bot_avatar` impl layer — resolve the
/// backend through `gen_backend::resolve` (D-02's one dispatch seam), drive
/// the resolved provider, classify the response size, then write. Async
/// (not `spawn_blocking`) — the provider round-trip is network I/O; see
/// module doc for why.
#[cfg(feature = "server")]
async fn generate_bot_avatar_impl(
    bot_name: &str,
    prompt: &str,
    config: &ironhermes_core::config::Config,
) -> Result<BotAvatarSaved, String> {
    let validated_name = ironhermes_core::profile::validate_profile_name(bot_name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    if prompt.trim().is_empty() {
        return Err(AVATAR_GENERATE_REJECT_MSG.to_string());
    }

    // D-02: resolve fal vs venice from the configured t2i model/provider —
    // the one seam that already owns this choice, never a provider client
    // called directly.
    let backend = ironhermes_tools::gen_backend::resolve(
        config.image_gen.t2i.provider.as_deref(),
        &config.image_gen.t2i.model,
    )
    .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;

    let (steps, safe_mode) =
        resolve_generation_args(config.image_gen.steps, config.image_gen.safe_mode);

    let bytes: Vec<u8> = match backend {
        ironhermes_tools::gen_backend::GenBackend::Venice(client) => {
            let venice_key = std::env::var("VENICE_API_KEY")
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;
            client
                .generate_image(
                    &venice_key,
                    &config.image_gen.t2i.model,
                    prompt,
                    "webp",
                    None,
                    None,
                    // Binding requirement (D-12): steps is ALWAYS
                    // Some(positive); safe_mode always forwarded.
                    steps,
                    safe_mode,
                )
                .await
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?
        }
        ironhermes_tools::gen_backend::GenBackend::Fal(client) => {
            let fal_key = std::env::var("FAL_KEY")
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;
            let submit = client
                .submit(&fal_key, &config.image_gen.t2i.model, prompt)
                .await
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;
            client
                .poll(
                    &fal_key,
                    &submit,
                    std::time::Duration::from_secs(config.image_gen.timeout_secs),
                )
                .await
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;
            let result = client
                .fetch(&fal_key, &submit)
                .await
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;
            let image = result
                .images
                .first()
                .ok_or_else(|| AVATAR_GENERATE_REJECT_MSG.to_string())?;
            // SAFE-03: `download_to_cache` SSRF-validates the fal-returned
            // URL before the GET — reused verbatim rather than a raw
            // `reqwest::get` on a provider-returned URL.
            let outcome = client
                .download_to_cache(&image.url, &image.content_type)
                .await
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;
            let cached_path = match outcome {
                ironhermes_tools::fal::DownloadOutcome::Photo(p)
                | ironhermes_tools::fal::DownloadOutcome::OversizedDocument(p) => p,
                ironhermes_tools::fal::DownloadOutcome::Video(_) => {
                    return Err(AVATAR_GENERATE_REJECT_MSG.to_string());
                }
            };
            tokio::fs::read(&cached_path)
                .await
                .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?
        }
    };

    classify_generation_bytes(&bytes)?;

    let image_id = uuid::Uuid::new_v4().to_string();
    let final_path = bot_avatar_path(&validated_name, &image_id)?;
    tokio::task::spawn_blocking({
        let bytes = bytes.clone();
        move || write_avatar_bytes_atomic(&final_path, &bytes)
    })
    .await
    .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?
    .map_err(|_| AVATAR_GENERATE_REJECT_MSG.to_string())?;

    Ok(BotAvatarSaved { image_id })
}

/// Phase 50.1 Plan 04 (D-11/D-12/T-50.1-04-02/03/04): upload one avatar
/// image. Follows this crate's four-step server-fn protocol: validate →
/// fresh `Config::load()` → `check_profile_write_gate` (fails closed) →
/// `spawn_blocking` for the disk write.
#[server]
pub async fn upload_bot_avatar(
    req: crate::protocol::BotAvatarUploadRequest,
) -> Result<BotAvatarSaved, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        let BotAvatarUploadRequest { bot_name, content_b64 } = req;
        let result = tokio::task::spawn_blocking(move || {
            upload_bot_avatar_impl(&bot_name, &content_b64)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(result)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "upload_bot_avatar unavailable without `server` feature",
        ))
    }
}

/// Phase 50.1 Plan 04 (D-11/D-12/T-50.1-04-04/05/06): generate one AI
/// portrait avatar. Validate → fresh `Config::load()` →
/// `check_profile_write_gate` (fails closed) → drive the resolved backend
/// → classify → write. The provider round-trip is NOT wrapped in
/// `spawn_blocking` (see module doc).
#[server]
pub async fn generate_bot_avatar(
    req: crate::protocol::BotAvatarGenerateRequest,
) -> Result<BotAvatarSaved, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        let BotAvatarGenerateRequest { bot_name, prompt } = req;
        let result = generate_bot_avatar_impl(&bot_name, &prompt, &config)
            .await
            .map_err(ServerFnError::new)?;
        Ok(result)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "generate_bot_avatar unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; env_lock() below
            // serializes every test in this crate that mutates process env.
            unsafe { std::env::set_var(key, value) };
            Self { key: key.to_string(), prev }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: still holding env_lock() from the caller.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().expect("tempdir path must be utf8"))
    }

    // -------------------------------------------------------------------
    // bot_avatar_path
    // -------------------------------------------------------------------

    #[test]
    fn bot_avatar_path_for_valid_name_resolves_under_the_avatar_directory() {
        let path = bot_avatar_path("scout", "abc123").expect("valid name must resolve");
        assert!(path.starts_with(bot_avatar_dir("scout")));
        assert_eq!(path.file_name().and_then(|f| f.to_str()), Some("abc123"));
    }

    #[test]
    fn bot_avatar_path_for_traversal_shaped_name_errors_before_any_path_is_joined() {
        let err = bot_avatar_path("../escape", "abc123")
            .expect_err("traversal-shaped name must be rejected");
        assert!(err.contains("invalid profile name"));
    }

    #[test]
    fn bot_avatar_path_for_traversal_shaped_identifier_errors() {
        let err = bot_avatar_path("scout", "../../etc/passwd")
            .expect_err("traversal-shaped identifier must be rejected");
        assert!(err.contains("invalid avatar identifier"));
    }

    // -------------------------------------------------------------------
    // upload_bot_avatar_impl
    // -------------------------------------------------------------------

    #[test]
    fn upload_bot_avatar_impl_rejects_oversize_payload_and_writes_nothing() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend(std::iter::repeat_n(0u8, AVATAR_UPLOAD_MAX_BYTES + 1));
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let err = upload_bot_avatar_impl("scout", &b64).expect_err("oversize payload must be rejected");
        assert_eq!(err, AVATAR_UPLOAD_REJECT_MSG);
        assert!(
            !bot_avatar_dir("scout").exists() || std::fs::read_dir(bot_avatar_dir("scout")).unwrap().next().is_none(),
            "no file must be written for an oversize payload"
        );
    }

    #[test]
    fn upload_bot_avatar_impl_rejects_non_image_signature_and_names_accepted_formats() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let b64 = base64::engine::general_purpose::STANDARD.encode(b"not an image at all");
        let err = upload_bot_avatar_impl("scout", &b64)
            .expect_err("non-raster payload must be rejected");
        assert_eq!(err, AVATAR_UPLOAD_REJECT_MSG);
        assert!(err.contains("PNG") && err.contains("JPG"), "message must name accepted formats: {err}");
        assert!(!err.contains("not an image at all"), "must never echo payload bytes: {err}");
    }

    #[test]
    fn upload_bot_avatar_impl_on_valid_payload_writes_bytes_and_returns_id_with_no_path() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let bytes = b"\x89PNG\r\n\x1a\nJUNKPAYLOAD".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let saved = upload_bot_avatar_impl("scout", &b64).expect("valid upload must succeed");
        assert!(!saved.image_id.is_empty());
        assert!(!saved.image_id.contains('/') && !saved.image_id.contains('\\'), "identifier must contain no filesystem path");

        let written = bot_avatar_path("scout", &saved.image_id).expect("path must resolve");
        assert_eq!(std::fs::read(&written).expect("file must exist"), bytes);
    }

    // -------------------------------------------------------------------
    // resolve_generation_args / resolve_avatar_gen_steps
    // -------------------------------------------------------------------

    #[test]
    fn resolve_avatar_gen_steps_zero_falls_back_to_positive_default() {
        assert_eq!(resolve_avatar_gen_steps(0), DEFAULT_AVATAR_GEN_STEPS);
        assert!(resolve_avatar_gen_steps(0) > 0);
    }

    #[test]
    fn resolve_generation_args_no_explicit_steps_still_carries_a_positive_default() {
        let (steps, safe_mode) = resolve_generation_args(0, true);
        assert_eq!(steps, Some(DEFAULT_AVATAR_GEN_STEPS));
        assert!(steps.unwrap() > 0);
        assert!(safe_mode);
    }

    #[test]
    fn resolve_generation_args_explicit_positive_steps_forwarded_verbatim() {
        let (steps, safe_mode) = resolve_generation_args(32, false);
        assert_eq!(steps, Some(32));
        assert!(!safe_mode);
    }

    // -------------------------------------------------------------------
    // classify_generation_bytes
    // -------------------------------------------------------------------

    #[test]
    fn classify_generation_bytes_below_threshold_is_a_failure() {
        let tiny = vec![0u8; UNUSABLE_BLOB_THRESHOLD_BYTES - 1];
        let err = classify_generation_bytes(&tiny).expect_err("sub-threshold blob must be classified as a failure");
        assert_eq!(err, AVATAR_GENERATE_REJECT_MSG);
    }

    #[test]
    fn classify_generation_bytes_at_or_above_threshold_succeeds() {
        let ok = vec![0u8; UNUSABLE_BLOB_THRESHOLD_BYTES];
        assert!(classify_generation_bytes(&ok).is_ok());
    }
}
