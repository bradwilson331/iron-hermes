//! Phase 46.7 Plan 04 (D-09): web-chat attachment upload transport.
//!
//! `upload_attachment` / `fetch_session_attachments` are arg-bearing POST
//! `#[server]` fns — NEVER `#[get]`. An arg-bearing `#[get]`-method server fn
//! cannot decode its body and silently 500s at runtime (Phase 46.6 round 5,
//! see `.planning/phases/46.6-agent-artifact-webpage/deferred-items.md`
//! lines ~322-360). Mirrors `kanban_api.rs`'s `attach_file`/`fetch_attachments`
//! shape (base64-in-JSON transport, `spawn_blocking`, cap-before-write), but
//! swaps the per-board `KanbanStore` for the web `AppState`'s shared
//! `state_store` (already open — no second sqlite connection to the same
//! purpose) and the kanban attachments dir for
//! `ironhermes_core::session_attachments_dir`.
//!
//! The core logic lives in `#[cfg(feature = "server")]`-only, synchronous,
//! dependency-injected `*_impl` functions (`state_store: &Arc<Mutex<StateStore>>`
//! passed in explicitly) so unit tests never touch the process-global
//! `global_app_state()` `OnceLock` singleton — each test opens its own
//! tempdir-backed `StateStore` and sets `IRONHERMES_HOME` to a tempdir under
//! the crate-wide `server::test_support::env_lock()` guard. The `#[server]`
//! entrypoints are thin wrappers that fetch the real `state_store` from
//! `global_app_state()` and delegate.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "server")]
use base64::Engine;

use crate::protocol::ChatAttachmentRow;

/// Phase 46.7 Plan 04 (D-09/T-46.7-11): the decoded (post-base64) payload
/// cap for non-image attachments — mirrors
/// `ironhermes_gateway::multimodal::NONIMAGE_MAX_BYTES` (10 MiB), the same
/// constant Plan 02's turn-assembly pipeline enforces, so upload-time and
/// turn-time caps never disagree.
#[cfg(feature = "server")]
const NONIMAGE_ATTACHMENT_CAP: usize = ironhermes_gateway::multimodal::NONIMAGE_MAX_BYTES;

/// Images may be uploaded up to this larger accept ceiling (D-09) — they are
/// downscaled/re-encoded to `IMAGE_SEND_MAX_BYTES` later at turn-assembly
/// time by `fit_image_for_vision`, so the upload-time cap only needs to bound
/// the raw upload, not the eventual vision-send payload.
#[cfg(feature = "server")]
const IMAGE_ATTACHMENT_ACCEPT_CAP: usize = 20 * 1024 * 1024;

/// Phase 46.7 Plan 04 (D-09/T-46.7-10/T-46.7-11): upload one file into a
/// web-chat session's attachment store.
///
/// Enforcement order (all BEFORE any filesystem write, per T-46.7-11/T-46.7-10):
/// 1. base64 decode
/// 2. empty-payload reject
/// 3. cap reject (image vs non-image ceiling, by sniffed content-type)
/// 4. traversal reject (`ironhermes_core::safe_attachment_leaf`)
///
/// Bytes are written under
/// `session_attachments_dir(session_id)/<opaque-id>/<safe-leaf>` and a
/// `chat_attachments` row is inserted with `stored_rel_path` relative to
/// that dir (D-21 redirect-safety). The client-side cap (Plan 05) is UX-only
/// — THIS server fn is the authoritative enforcement point (mirrors the
/// `kanban_api.rs:792` comment).
#[server]
pub async fn upload_attachment(
    session_id: String,
    filename: String,
    content_b64: String,
) -> Result<ChatAttachmentRow, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let state_store = crate::server::state::global_app_state().state_store.clone();
        let row = tokio::task::spawn_blocking(move || {
            upload_attachment_impl(&state_store, &session_id, &filename, &content_b64)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(row)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (session_id, filename, content_b64);
        Err(ServerFnError::new(
            "upload_attachment unavailable without `server` feature",
        ))
    }
}

/// Phase 46.7 Plan 04 (D-09/D-11): read-side list of a session's attachments
/// — the authoritative retrieval fn for the composer/history render (Plan 05).
/// Ordered by `created_at` ASC (oldest-first), matching `list_chat_attachments`.
#[server]
pub async fn fetch_session_attachments(
    session_id: String,
) -> Result<Vec<ChatAttachmentRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let state_store = crate::server::state::global_app_state().state_store.clone();
        let rows = tokio::task::spawn_blocking(move || {
            fetch_session_attachments_impl(&state_store, &session_id)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(rows)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = session_id;
        Err(ServerFnError::new(
            "fetch_session_attachments unavailable without `server` feature",
        ))
    }
}

/// Dependency-injected core of [`upload_attachment`] — no `global_app_state()`,
/// no dioxus server-fn machinery, so unit tests can drive it directly against
/// a fresh tempdir `StateStore` with `IRONHERMES_HOME` redirected (T-46.7-13:
/// attachment bytes are written and read only under
/// `session_attachments_dir(session_id)` for the CURRENT session — a foreign
/// session's bytes are structurally unreachable from this fn).
#[cfg(feature = "server")]
fn upload_attachment_impl(
    state_store: &Arc<Mutex<ironhermes_state::StateStore>>,
    session_id: &str,
    filename: &str,
    content_b64: &str,
) -> Result<ChatAttachmentRow, String> {
    // CR-01 (traversal): the client-supplied session_id is joined into a
    // filesystem path by session_attachments_dir below — reject any id that
    // could escape sessions/ BEFORE any fs/DB op.
    if ironhermes_core::safe_session_id(session_id).is_none() {
        return Err(format!("invalid session id: {session_id:?}"));
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content_b64.as_bytes())
        .map_err(|e| format!("upload_attachment: invalid base64 payload: {e}"))?;

    // T-46.7-11 (DoS): reject empty BEFORE any fs write.
    if bytes.is_empty() {
        return Err(format!("{filename}: empty attachment — not uploaded"));
    }

    let content_type = content_type_from_ext(filename);
    let is_image = content_type
        .as_deref()
        .is_some_and(|c| c.starts_with("image/"));
    let cap = if is_image {
        IMAGE_ATTACHMENT_ACCEPT_CAP
    } else {
        NONIMAGE_ATTACHMENT_CAP
    };
    if bytes.len() > cap {
        return Err(format!(
            "{filename} exceeds the {}MB attachment limit",
            cap / (1024 * 1024)
        ));
    }

    // T-46.7-10 (traversal): reject BEFORE any fs op. safe_attachment_leaf
    // rejects empty/".."/separators — "../evil" and "a/b" both fail here.
    let safe_leaf = ironhermes_core::safe_attachment_leaf(filename)
        .ok_or_else(|| format!("{filename}: invalid filename"))?
        .to_string();

    let opaque_id = uuid::Uuid::new_v4().to_string();
    let attachment_dir = ironhermes_core::session_attachments_dir(session_id).join(&opaque_id);
    std::fs::create_dir_all(&attachment_dir).map_err(|e| format!("create attachment dir: {e}"))?;
    let file_path = attachment_dir.join(&safe_leaf);
    std::fs::write(&file_path, &bytes).map_err(|e| format!("write attachment: {e}"))?;

    let stored_rel_path = format!("{opaque_id}/{safe_leaf}");

    let mut store = state_store
        .lock()
        .map_err(|_| "state store mutex poisoned".to_string())?;
    let row = store
        .add_chat_attachment(
            session_id,
            None,
            filename,
            content_type.as_deref(),
            bytes.len() as i64,
            &stored_rel_path,
        )
        .map_err(|e| format!("add_chat_attachment: {e}"))?;

    Ok(ChatAttachmentRow {
        id: row.id,
        session_id: row.session_id,
        filename: row.filename,
        content_type: row.content_type,
        size_bytes: row.size_bytes,
        message_id: row.message_id,
    })
}

/// Dependency-injected core of [`fetch_session_attachments`] — see
/// [`upload_attachment_impl`] doc for why this bypasses `global_app_state()`.
#[cfg(feature = "server")]
fn fetch_session_attachments_impl(
    state_store: &Arc<Mutex<ironhermes_state::StateStore>>,
    session_id: &str,
) -> Result<Vec<ChatAttachmentRow>, String> {
    // CR-01 (traversal): validate the client-supplied session_id before it is
    // used to key any lookup. A malformed id has no attachments anyway; reject
    // it explicitly rather than round-tripping it through the store.
    if ironhermes_core::safe_session_id(session_id).is_none() {
        return Err(format!("invalid session id: {session_id:?}"));
    }
    let store = state_store
        .lock()
        .map_err(|_| "state store mutex poisoned".to_string())?;
    let rows = store
        .list_chat_attachments(session_id)
        .map_err(|e| format!("list_chat_attachments: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|m| ChatAttachmentRow {
            id: m.id,
            session_id: m.session_id,
            filename: m.filename,
            content_type: m.content_type,
            size_bytes: m.size_bytes,
            message_id: m.message_id,
        })
        .collect())
}

/// Best-effort MIME sniff from the filename extension — mirrors
/// `kanban_api.rs::content_type_from_ext` exactly (deliberate copy, not a
/// shared helper — this crate has no shared mime-sniff utility and the table
/// is 8 lines). Display/classification metadata only; unknown extensions
/// return `None` and fall back to the non-image cap.
#[cfg(feature = "server")]
fn content_type_from_ext(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "txt" | "md" => "text/plain",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "zip" => "application/zip",
        _ => return None,
    };
    Some(mime.to_string())
}

#[cfg(all(test, feature = "server"))]
mod chat_attachment_upload_tests {
    use super::*;
    use tempfile::tempdir;

    // Tests that mutate IRONHERMES_HOME must hold the crate-wide
    // `test_support::env_lock()` — a module-local mutex cannot serialize env
    // mutation against other modules' tests in the same binary (the runner
    // process-isolates per-binary, not per-test-fn).

    fn fresh_store() -> Arc<Mutex<ironhermes_state::StateStore>> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = ironhermes_state::StateStore::new(&path).unwrap();
        store
            .create_session("sess-up-1", "web", None, None, None, None)
            .unwrap();
        // Leak the tempdir so the sqlite file outlives the test body (the
        // Arc<Mutex<StateStore>> holds the open connection, not the path).
        std::mem::forget(dir);
        Arc::new(Mutex::new(store))
    }

    #[test]
    fn valid_upload_writes_under_session_attachments_dir_and_inserts_a_row() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: env var mutation is guarded by test_support::env_lock() above.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let store = fresh_store();
        let content_b64 =
            base64::engine::general_purpose::STANDARD.encode(b"hello attachment bytes");
        let row = upload_attachment_impl(&store, "sess-up-1", "a.png", &content_b64)
            .expect("valid upload must succeed");

        assert_eq!(row.session_id, "sess-up-1");
        assert_eq!(row.filename, "a.png");
        assert!(!row.id.is_empty(), "id must be non-empty");
        assert_eq!(row.content_type.as_deref(), Some("image/png"));

        // The file must actually exist under session_attachments_dir.
        let att_dir = ironhermes_core::session_attachments_dir("sess-up-1");
        let mut found = false;
        for entry in std::fs::read_dir(&att_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_dir() {
                let leaf = entry.path().join("a.png");
                if leaf.is_file() {
                    found = true;
                    assert_eq!(std::fs::read(&leaf).unwrap(), b"hello attachment bytes");
                }
            }
        }
        assert!(
            found,
            "uploaded file must exist under session_attachments_dir"
        );

        let listed = fetch_session_attachments_impl(&store, "sess-up-1").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "a.png");

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn empty_payload_returns_err_and_no_file_created() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let store = fresh_store();
        let empty_b64 = base64::engine::general_purpose::STANDARD.encode(b"");
        let result = upload_attachment_impl(&store, "sess-up-1", "empty.txt", &empty_b64);
        assert!(result.is_err(), "empty payload must be rejected");

        let att_dir = ironhermes_core::session_attachments_dir("sess-up-1");
        assert!(
            !att_dir.exists() || std::fs::read_dir(&att_dir).unwrap().next().is_none(),
            "no file must be created for an empty payload"
        );
        assert!(fetch_session_attachments_impl(&store, "sess-up-1")
            .unwrap()
            .is_empty());

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn traversal_session_id_is_rejected_and_writes_nothing_outside_sessions() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let store = fresh_store();
        let content_b64 = base64::engine::general_purpose::STANDARD.encode(b"malicious bytes");
        // CR-01: a traversal session_id must be rejected before any fs write.
        let result = upload_attachment_impl(&store, "../../../../evil", "pwned.txt", &content_b64);
        assert!(result.is_err(), "traversal session_id must be rejected");

        // Nothing may have been written outside sessions/ (the tempdir HOME).
        let escaped = home_dir.path().join("evil");
        assert!(
            !escaped.exists(),
            "no dir/file may be created outside sessions/"
        );
        // The read side rejects it too.
        assert!(fetch_session_attachments_impl(&store, "../../../../evil").is_err());

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn oversize_payload_returns_err_and_no_file_created() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let store = fresh_store();
        // Non-image extension -> NONIMAGE_ATTACHMENT_CAP (10 MiB). One byte
        // over is enough to trip the cap without allocating a huge buffer
        // twice over (base64 inflates ~4/3x, still cheap at this size).
        let oversized = vec![0u8; NONIMAGE_ATTACHMENT_CAP + 1];
        let oversized_b64 = base64::engine::general_purpose::STANDARD.encode(&oversized);
        let result = upload_attachment_impl(&store, "sess-up-1", "big.bin", &oversized_b64);
        assert!(result.is_err(), "oversize payload must be rejected");
        let err = result.unwrap_err();
        assert!(err.contains("MB"), "error must be sized (got: {err})");

        let att_dir = ironhermes_core::session_attachments_dir("sess-up-1");
        assert!(
            !att_dir.exists() || std::fs::read_dir(&att_dir).unwrap().next().is_none(),
            "no file must be created for an oversize payload"
        );

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn traversal_filename_dotdot_returns_err() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let store = fresh_store();
        let content_b64 = base64::engine::general_purpose::STANDARD.encode(b"payload");
        let result = upload_attachment_impl(&store, "sess-up-1", "../evil", &content_b64);
        assert!(result.is_err(), "'../evil' filename must be rejected");

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn traversal_filename_separator_returns_err() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let store = fresh_store();
        let content_b64 = base64::engine::general_purpose::STANDARD.encode(b"payload");
        let result = upload_attachment_impl(&store, "sess-up-1", "a/b", &content_b64);
        assert!(result.is_err(), "'a/b' filename must be rejected");

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn fetch_returns_all_rows_for_session_oldest_first() {
        let _lock = crate::server::test_support::env_lock();
        let home_dir = tempdir().unwrap();
        // SAFETY: guarded by test_support::env_lock().
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home_dir.path());
        }

        let store = fresh_store();
        let b64_a = base64::engine::general_purpose::STANDARD.encode(b"aaa");
        let b64_b = base64::engine::general_purpose::STANDARD.encode(b"bbb");
        upload_attachment_impl(&store, "sess-up-1", "first.txt", &b64_a).unwrap();
        upload_attachment_impl(&store, "sess-up-1", "second.txt", &b64_b).unwrap();

        let listed = fetch_session_attachments_impl(&store, "sess-up-1").unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].filename, "first.txt");
        assert_eq!(listed[1].filename, "second.txt");

        // SAFETY: still holding test_support::env_lock() from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    /// Source-invariant assertion (acceptance criteria): NO arg-bearing fn in
    /// this file uses the GET-method server-fn attribute — both public
    /// entrypoints must be POST (arg-bearing) `#[server]` fns, mirroring the
    /// round-5 lesson (an arg-bearing `#[get]` server fn silently 500s).
    #[test]
    fn no_arg_bearing_fn_uses_get_method_attribute() {
        let source = include_str!("chat_attachments_api.rs");
        // Built via format! (not a literal attribute string) so this
        // assertion's own source line can never trip itself via the
        // include_str! self-reference above.
        let get_attr_needle = format!("#{}get(", '[');
        assert!(
            !source.contains(&get_attr_needle),
            "chat_attachments_api.rs must not use the #[get] server-fn attribute on any arg-bearing fn"
        );
        // Match the attribute immediately followed by its fn signature (not
        // prose mentions in doc comments, which don't have "\npub async fn"
        // right after the backticked `#[server]` text).
        assert_eq!(
            source.matches("#[server]\npub async fn").count(),
            2,
            "expected exactly 2 #[server]-annotated pub async fns (upload_attachment, fetch_session_attachments)"
        );
    }
}
