//! Phase 47 Plan 04 — provider dispatch seam (`GenBackend`).
//!
//! The single place the fal ↔ venice choice is made (D-02). `resolve` is a
//! pure, stateless function: it takes the resolved-per-call `explicit_provider`
//! (from `GenModeConfig::provider`) and the *effective* model string (an LLM
//! per-call override already applied over the config default, per Pattern 1)
//! and returns the matching backend — or a clear error on an explicit/prefix
//! mismatch (never a silent misroute to the wrong provider).
//!
//! Task 2 adds the backend-agnostic cache-write helper (Pattern 2) so both the
//! Venice image path (base64-decoded bytes, no network) and the fal image/video
//! path (CDN-downloaded bytes) converge on the SAME `DownloadOutcome`
//! classification `fal.rs` already defines.

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::fal::{DownloadOutcome, FalClient};
use crate::venice::VeniceClient;

/// The fal.ai model-id prefix used for D-02 provider inference. A trimmed
/// effective model starting with this prefix routes to fal; anything else
/// routes to venice.
const FAL_MODEL_PREFIX: &str = "fal-ai/";

/// The resolved generation backend for a single call. Wraps the existing,
/// unchanged `FalClient` (Phase 01) and `VeniceClient` (Plan 02) — `resolve`
/// constructs a fresh client each call, so this carries no shared state.
#[derive(Debug, Clone)]
pub enum GenBackend {
    Fal(FalClient),
    Venice(VeniceClient),
}

/// Resolve which backend a single generation call should use (D-02).
///
/// Order (must match exactly — Pattern 1):
/// 1. If `explicit_provider` is `Some`, it MUST be `"fal"` or `"venice"`
///    (case-insensitive, trimmed) AND consistent with `effective_model`'s
///    prefix. An unknown provider string, or an explicit provider that
///    contradicts the model prefix (e.g. `venice` with a `fal-ai/*` model),
///    is a terminal error — never a silent misroute.
/// 2. If `explicit_provider` is `None`, infer from the trimmed
///    `effective_model` prefix: `fal-ai/` → fal, anything else → venice.
///
/// `effective_model` MUST be the post-override model (an LLM per-call
/// override already applied over the config default) — resolution must never
/// be done against the config default alone (D-02 prohibition).
///
/// Pure and deterministic: the same inputs always yield the same backend, and
/// concurrent calls never interfere (no shared/mutable state is read or
/// written).
pub fn resolve(
    explicit_provider: Option<&str>,
    effective_model: &str,
) -> anyhow::Result<GenBackend> {
    let trimmed_model = effective_model.trim();
    let model_is_fal = trimmed_model.starts_with(FAL_MODEL_PREFIX);

    match explicit_provider {
        Some(raw_provider) => {
            let provider = raw_provider.trim().to_ascii_lowercase();
            match provider.as_str() {
                "fal" => {
                    if model_is_fal {
                        Ok(GenBackend::Fal(FalClient::new()))
                    } else {
                        anyhow::bail!(
                            "generation provider is configured as 'fal' but model '{trimmed_model}' does not have the fal-ai/ prefix — refusing to route to the wrong backend"
                        )
                    }
                }
                "venice" => {
                    if model_is_fal {
                        anyhow::bail!(
                            "generation provider is configured as 'venice' but model '{trimmed_model}' has the fal-ai/ prefix (a fal.ai model id) — refusing to route to the wrong backend"
                        )
                    } else {
                        Ok(GenBackend::Venice(VeniceClient::new()))
                    }
                }
                _ => anyhow::bail!(
                    "unknown generation provider '{raw_provider}' — must be 'fal' or 'venice'"
                ),
            }
        }
        None => {
            if model_is_fal {
                Ok(GenBackend::Fal(FalClient::new()))
            } else {
                Ok(GenBackend::Venice(VeniceClient::new()))
            }
        }
    }
}

/// Write `bytes` to the image cache and classify against `photo_cap` (mirrors
/// `fal.rs`'s `DownloadOutcome` classification exactly — Photo at/under the
/// cap, OversizedDocument over it). `ext` is the file extension WITHOUT a
/// leading dot (e.g. `"webp"`, `"png"`).
///
/// Feeds both the Venice image path (base64-decoded bytes, `ext` = Venice's
/// `format` param) and any other backend-agnostic caller so `image_gen.rs`
/// keeps its existing match arms unchanged.
pub async fn write_bytes_to_image_cache(
    bytes: &[u8],
    ext: &str,
    photo_cap: u64,
) -> anyhow::Result<DownloadOutcome> {
    write_bytes_to_cache(bytes, "images", ext, photo_cap, DownloadOutcome::Photo).await
}

/// Write `bytes` to the video cache and classify against `video_cap` (mirrors
/// `fal.rs`'s `DownloadOutcome` classification exactly — Video at/under the
/// cap, OversizedDocument over it). `ext` is the file extension WITHOUT a
/// leading dot (e.g. `"mp4"`).
pub async fn write_bytes_to_video_cache(
    bytes: &[u8],
    ext: &str,
    video_cap: u64,
) -> anyhow::Result<DownloadOutcome> {
    write_bytes_to_cache(bytes, "videos", ext, video_cap, DownloadOutcome::Video).await
}

/// Shared cache-write core (D-01/D-02/D-03 filename scheme):
/// `$IRONHERMES_HOME/cache/{images,videos}/<UTC %Y%m%dT%H%M%S>-<uuid v4>.<ext>`.
/// Creates the parent dir, writes the bytes, then classifies the WRITTEN size
/// against `cap`: at/under → `under_cap(path)`, over → `OversizedDocument`. The
/// file is written in both cases (matching `fal.rs`'s existing behavior).
async fn write_bytes_to_cache(
    bytes: &[u8],
    subdir: &str,
    ext: &str,
    cap: u64,
    under_cap: impl FnOnce(PathBuf) -> DownloadOutcome,
) -> anyhow::Result<DownloadOutcome> {
    let dir = ironhermes_core::constants::get_hermes_home()
        .join("cache")
        .join(subdir);
    tokio::fs::create_dir_all(&dir).await?;

    let fname = format!(
        "{}-{}.{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        uuid::Uuid::new_v4(),
        ext
    );
    let abs_path = dir.join(fname);

    let mut file = tokio::fs::File::create(&abs_path).await?;
    file.write_all(bytes).await?;
    file.flush().await?;

    let written = bytes.len() as u64;
    if written > cap {
        Ok(DownloadOutcome::OversizedDocument(abs_path))
    } else {
        Ok(under_cap(abs_path))
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn none_provider_fal_prefix_resolves_to_fal() {
        let backend = resolve(None, "fal-ai/flux/schnell").expect("should resolve");
        assert!(matches!(backend, GenBackend::Fal(_)));
    }

    #[test]
    fn none_provider_non_fal_model_resolves_to_venice() {
        let backend = resolve(None, "wan-2-7-text-to-video").expect("should resolve");
        assert!(matches!(backend, GenBackend::Venice(_)));
    }

    #[test]
    fn explicit_venice_with_fal_model_is_a_mismatch_error() {
        let err = resolve(Some("venice"), "fal-ai/flux/schnell")
            .expect_err("venice + fal-ai/* model must error, never silently misroute");
        let msg = err.to_string();
        assert!(
            msg.contains("venice"),
            "error should name the provider: {msg}"
        );
        assert!(
            msg.contains("fal-ai/flux/schnell"),
            "error should name the model: {msg}"
        );
    }

    #[test]
    fn explicit_fal_with_fal_model_is_consistent() {
        let backend =
            resolve(Some("fal"), "fal-ai/x").expect("consistent explicit fal should resolve");
        assert!(matches!(backend, GenBackend::Fal(_)));
    }

    #[test]
    fn explicit_fal_with_non_fal_model_is_a_mismatch_error() {
        let err = resolve(Some("fal"), "wan-2-7-text-to-video")
            .expect_err("fal + non-fal-prefixed model must error, never silently misroute");
        let msg = err.to_string();
        assert!(msg.contains("fal"), "error should name the provider: {msg}");
    }

    #[test]
    fn none_provider_trims_whitespace_and_tolerates_casing_in_model() {
        let backend = resolve(None, "  fal-ai/x  ").expect("trimmed model should resolve");
        assert!(matches!(backend, GenBackend::Fal(_)));
    }

    #[test]
    fn unknown_explicit_provider_is_an_error() {
        let err =
            resolve(Some("bogus"), "wan-2-7").expect_err("an unknown provider string must error");
        let msg = err.to_string();
        assert!(
            msg.contains("bogus"),
            "error should name the bad provider: {msg}"
        );
    }

    #[test]
    fn resolve_is_deterministic_across_repeated_calls() {
        let a = resolve(None, "fal-ai/flux/schnell").expect("first call should resolve");
        let b = resolve(None, "fal-ai/flux/schnell").expect("second call should resolve");
        assert!(matches!(a, GenBackend::Fal(_)));
        assert!(matches!(b, GenBackend::Fal(_)));

        let a =
            resolve(Some("venice"), "wan-2-7-text-to-video").expect("first call should resolve");
        let b =
            resolve(Some("venice"), "wan-2-7-text-to-video").expect("second call should resolve");
        assert!(matches!(a, GenBackend::Venice(_)));
        assert!(matches!(b, GenBackend::Venice(_)));
    }

    #[test]
    fn resolve_uses_effective_model_not_a_config_default() {
        // Even though `explicit_provider` is None (as if the config default
        // provider were unset), an effective model with the fal-ai/ prefix
        // must still route to fal — resolution reads ONLY the arguments
        // passed in, never a config struct (D-02 prohibition).
        let backend = resolve(None, "fal-ai/some-llm-overridden-model").expect("should resolve");
        assert!(matches!(backend, GenBackend::Fal(_)));
    }
}

#[cfg(test)]
mod cache_write_tests {
    use super::*;
    use tempfile::TempDir;

    /// Serialize IRONHERMES_HOME env mutation across tests in this module
    /// (parallel-safe) — mirrors the established pattern in `skill_manage.rs`.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard: sets IRONHERMES_HOME to a fresh tempdir on construct,
    /// restores the prior value on drop. Holds the env lock for its lifetime.
    struct HermesHomeGuard {
        _tmp: TempDir,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HermesHomeGuard {
        fn new() -> Self {
            let lock = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let tmp = TempDir::new().expect("create tempdir");
            let prev = std::env::var("IRONHERMES_HOME").ok();
            unsafe {
                std::env::set_var("IRONHERMES_HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for HermesHomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                    None => std::env::remove_var("IRONHERMES_HOME"),
                }
            }
        }
    }

    #[tokio::test]
    async fn image_bytes_under_cap_classify_photo_and_exist_under_cache_images() {
        let _guard = HermesHomeGuard::new();
        let bytes = vec![0u8; 100];
        let outcome = write_bytes_to_image_cache(&bytes, "webp", 20 * 1024 * 1024)
            .await
            .expect("write should succeed");
        match outcome {
            DownloadOutcome::Photo(path) => {
                assert!(path.exists(), "written file must exist at {path:?}");
                let expected_dir = ironhermes_core::constants::get_hermes_home()
                    .join("cache")
                    .join("images");
                assert!(
                    path.starts_with(&expected_dir),
                    "path {path:?} must be under {expected_dir:?}"
                );
                assert_eq!(path.extension().and_then(|e| e.to_str()), Some("webp"));
            }
            other => panic!("expected Photo, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn image_bytes_over_cap_classify_oversized_document() {
        let _guard = HermesHomeGuard::new();
        let bytes = vec![0u8; 200];
        let outcome = write_bytes_to_image_cache(&bytes, "png", 100)
            .await
            .expect("write should succeed");
        assert!(matches!(outcome, DownloadOutcome::OversizedDocument(_)));
    }

    #[tokio::test]
    async fn video_bytes_under_cap_classify_video() {
        let _guard = HermesHomeGuard::new();
        let bytes = vec![0u8; 100];
        let outcome = write_bytes_to_video_cache(&bytes, "mp4", 50 * 1024 * 1024)
            .await
            .expect("write should succeed");
        match outcome {
            DownloadOutcome::Video(path) => {
                assert!(path.exists(), "written file must exist at {path:?}");
                let expected_dir = ironhermes_core::constants::get_hermes_home()
                    .join("cache")
                    .join("videos");
                assert!(
                    path.starts_with(&expected_dir),
                    "path {path:?} must be under {expected_dir:?}"
                );
                assert_eq!(path.extension().and_then(|e| e.to_str()), Some("mp4"));
            }
            other => panic!("expected Video, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn video_bytes_over_cap_classify_oversized_document() {
        let _guard = HermesHomeGuard::new();
        let bytes = vec![0u8; 200];
        let outcome = write_bytes_to_video_cache(&bytes, "mp4", 100)
            .await
            .expect("write should succeed");
        assert!(matches!(outcome, DownloadOutcome::OversizedDocument(_)));
    }

    #[tokio::test]
    async fn produced_path_is_absolute_and_under_hermes_home_cache() {
        let _guard = HermesHomeGuard::new();
        let bytes = vec![0u8; 10];
        let outcome = write_bytes_to_image_cache(&bytes, "png", 20 * 1024 * 1024)
            .await
            .expect("write should succeed");
        let path = match outcome {
            DownloadOutcome::Photo(p)
            | DownloadOutcome::OversizedDocument(p)
            | DownloadOutcome::Video(p) => p,
        };
        assert!(path.is_absolute(), "cache path must be absolute");
        let expected_cache_dir = ironhermes_core::constants::get_hermes_home().join("cache");
        assert!(
            path.starts_with(&expected_cache_dir),
            "path {path:?} must be under {expected_cache_dir:?}"
        );
    }
}
