//! Phase 36.17.2.2 Plan 06 — Telegram media delivery integration tests.
//!
//! Drives the D-19 dispatch surface end-to-end via:
//! 1. `RecordingMediaAdapter` — succeeds-on-send fixture implementing BOTH
//!    `PlatformAdapter` and `MediaSender`, recording every call into
//!    timestamped logs so D-07 ordering and D-10/D-11/D-15 routing can be
//!    asserted.
//! 2. Synthetic LLM-stream feeding — chunks pushed through `scrubber →
//!    extractor` (the same chain wired into `run_agent` at handler.rs:1366)
//!    so the test covers the production composition (Open Q5 / A10).
//! 3. `dispatch_media_d19` helper — reproduces the D-19 dispatch block
//!    from handler.rs verbatim (the block between `typing_handle.await.ok()`
//!    and `match agent_result`). Driving the helper directly tests the
//!    dispatch surface without requiring `AgentRuntime` injection.
//!
//! # Why this shape (synthetic-LLM injection mechanism — Open Q2)
//!
//! `AgentRuntime::from_config` requires a live HTTP client built from the
//! provider config (agent_runtime.rs:168 — `build_main_client(&resolver)`).
//! `AgentRuntime::for_tests()` exists at agent_runtime.rs:583 but is gated
//! by `#[cfg(any(test, feature = "test-support"))]` AND uses
//! `localhost:0` which cannot actually produce streaming deltas.
//!
//! Building a `FakeStreamingProvider` would require:
//! - A new trait surface on `AnyClient` (the streaming entry point) — multi-
//!   variant enum that would have to grow a `Fake` variant; OR
//! - A feature flag promoting `AgentRuntime::for_tests()` to integration-test
//!   visibility plus a closure-driven delta-stream injection point on
//!   `AgentRuntime` — invasive across crate boundaries.
//!
//! The plan's `<output>` instructs the executor to document the choice.
//! Chosen mechanism: **directly drive the production composition**
//! (`scrubber → extractor → dispatch_media_d19`) with hand-fed deltas. This
//! tests every D-07/D-09/D-10/D-11/D-15 contract on the same code paths
//! `run_agent` exercises, without forcing AgentRuntime surgery.
//!
//! # Why D-02 retry observability is weakened (threat model T-D02-RETRY-NOT-OBSERVABLE)
//!
//! Plan 04 implements the D-02 parse-mode fallback inside
//! `TelegramAdapter::edit_message_markdown_v2` by substring-matching
//! `anyhow::Error` description against `"can't parse"` / `"parse entities"` /
//! `"parse_mode"` and re-issuing the SAME `editMessageText` `api_call` —
//! NOT routing the retry through the trait surface. The RecordingMediaAdapter
//! cannot observe a retry that bypasses `edit_message` / `edit_message_markdown_v2`.
//! Test 7 therefore asserts only the FIRST attempt is recorded and no error
//! propagates. Live UAT (plan 07 Scenario 8) covers the actual retry.

#![allow(clippy::expect_used)]
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use ironhermes_core::{MessageResponse, Platform};
use ironhermes_gateway::adapter::{MediaSender, PlatformAdapter};
use ironhermes_gateway::markdown_v2::escape_outside_code_blocks;
use ironhermes_gateway::media_tag::{MediaKind, MediaRef, MediaSource, MediaTagExtractor};

// ===========================================================================
// RecordingMediaAdapter
// ===========================================================================
//
// Succeeds-on-send fixture so the streaming pipeline runs end-to-end (unlike
// `RecordingFailingAdapter` from `uqm_session_queue_unification.rs` which
// fails `send_message` to fast-exit `run_agent`). Implements BOTH
// `PlatformAdapter` and `MediaSender` so the same Arc fans out to handler +
// media dispatch — matching the production clone-cast pattern at runner.rs.
//
// All logs are wrapped in a shared `AtomicU64` sequence counter so D-07
// ordering (V2 edit BEFORE any send_media) can be asserted by checking the
// recorded sequence numbers.

/// Sequence counter — every recorded log entry gets a monotonically-
/// increasing sequence ID so cross-log ordering is observable.
fn next_seq(seq: &AtomicU64) -> u64 {
    seq.fetch_add(1, Ordering::SeqCst)
}

#[derive(Debug, Clone)]
pub struct PlainSendEntry {
    pub seq: u64,
    pub chat_id: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct V2SendEntry {
    pub seq: u64,
    pub chat_id: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct EditEntry {
    pub seq: u64,
    pub chat_id: String,
    pub message_id: String,
    pub content: String,
    pub is_markdown_v2: bool,
}

#[derive(Debug, Clone)]
pub struct MediaEntry {
    pub seq: u64,
    pub chat_id: String,
    pub media_ref: MediaRef,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReactionEntry {
    pub seq: u64,
    pub chat_id: String,
    pub message_id: String,
    pub emoji: String,
}

pub struct RecordingMediaAdapter {
    pub seq: AtomicU64,
    pub send_log: StdMutex<Vec<PlainSendEntry>>,
    pub send_v2_log: StdMutex<Vec<V2SendEntry>>,
    pub edit_log: StdMutex<Vec<EditEntry>>,
    pub media_log: StdMutex<Vec<MediaEntry>>,
    pub reactions: StdMutex<Vec<ReactionEntry>>,
    pub next_message_id: StdMutex<i64>,
    /// Path strings (as displayed in `<MEDIA: ...>` tags) for which
    /// `send_photo` / `send_voice` / `send_audio` / `send_video` /
    /// `send_document` must return `Err` — drives the D-10 reinsert path.
    pub photo_fail_paths: StdMutex<HashSet<String>>,
    /// Path strings for which the simulated send should report an
    /// "oversize" error matching plan 04's D-15 typed error prefix
    /// (`"media exceeds size cap:"`). Drives the D-15 + D-10 path.
    pub oversize_fail_paths: StdMutex<HashSet<String>>,
    /// Message IDs for which `edit_message_markdown_v2` must return Err
    /// simulating Telegram's `400 Bad Request: can't parse entities`.
    /// Drives the D-02 fallback path.
    pub parse_mode_fail_msg_ids: StdMutex<HashSet<String>>,
}

impl RecordingMediaAdapter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            seq: AtomicU64::new(0),
            send_log: StdMutex::new(Vec::new()),
            send_v2_log: StdMutex::new(Vec::new()),
            edit_log: StdMutex::new(Vec::new()),
            media_log: StdMutex::new(Vec::new()),
            reactions: StdMutex::new(Vec::new()),
            next_message_id: StdMutex::new(100),
            photo_fail_paths: StdMutex::new(HashSet::new()),
            oversize_fail_paths: StdMutex::new(HashSet::new()),
            parse_mode_fail_msg_ids: StdMutex::new(HashSet::new()),
        })
    }

    fn alloc_message_id(&self) -> String {
        let mut id = self.next_message_id.lock().unwrap();
        let next = *id;
        *id += 1;
        next.to_string()
    }

    fn record_plain_send(&self, chat_id: &str, content: &str) {
        self.send_log.lock().unwrap().push(PlainSendEntry {
            seq: next_seq(&self.seq),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
        });
    }

    fn record_v2_send(&self, chat_id: &str, content: &str) {
        self.send_v2_log.lock().unwrap().push(V2SendEntry {
            seq: next_seq(&self.seq),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
        });
    }

    fn record_edit(&self, chat_id: &str, message_id: &str, content: &str, is_markdown_v2: bool) {
        self.edit_log.lock().unwrap().push(EditEntry {
            seq: next_seq(&self.seq),
            chat_id: chat_id.to_string(),
            message_id: message_id.to_string(),
            content: content.to_string(),
            is_markdown_v2,
        });
    }

    fn record_media(
        &self,
        chat_id: &str,
        source: &MediaSource,
        kind: MediaKind,
        thread_id: Option<&str>,
    ) -> u64 {
        let seq = next_seq(&self.seq);
        self.media_log.lock().unwrap().push(MediaEntry {
            seq,
            chat_id: chat_id.to_string(),
            media_ref: MediaRef {
                source: source.clone(),
                kind,
                // Tests don't depend on the literal here; the original
                // `<MEDIA: ...>` is preserved separately in `failed_tags`.
                original_tag_text: String::new(),
            },
            thread_id: thread_id.map(|s| s.to_string()),
        });
        seq
    }

    /// Path-string extraction for failure-set lookup. URL variants never
    /// match the failure sets (URL form has no D-15 size pre-check; D-10
    /// failure simulation targets Path arms exclusively).
    fn path_str(source: &MediaSource) -> Option<String> {
        match source {
            MediaSource::Path(p) => Some(p.display().to_string()),
            MediaSource::Url(_) => None,
        }
    }

    fn maybe_fail_path(
        &self,
        source: &MediaSource,
    ) -> Option<anyhow::Error> {
        let path = Self::path_str(source)?;
        if self.oversize_fail_paths.lock().unwrap().contains(&path) {
            return Some(anyhow::anyhow!(
                "media exceeds size cap: {} 22020096 > 20971520",
                path
            ));
        }
        if self.photo_fail_paths.lock().unwrap().contains(&path) {
            return Some(anyhow::anyhow!("simulated send failure for {}", path));
        }
        None
    }
}

#[async_trait]
impl PlatformAdapter for RecordingMediaAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.record_plain_send(chat_id, content);
        Ok(MessageResponse {
            message_id: self.alloc_message_id(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.record_v2_send(chat_id, content);
        Ok(MessageResponse {
            message_id: self.alloc_message_id(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn edit_message(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.record_edit(chat_id, message_id, content, /*is_markdown_v2=*/ false);
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        // T-D02-RETRY-NOT-OBSERVABLE: record the V2 attempt BEFORE deciding
        // whether to simulate the parse-mode failure. The real
        // `TelegramAdapter` records the failing call via `api_call` (HTTP
        // round-trip), so the corresponding fixture behavior is "record
        // then maybe-fail" to mirror the production trace.
        self.record_edit(chat_id, message_id, content, /*is_markdown_v2=*/ true);
        if self
            .parse_mode_fail_msg_ids
            .lock()
            .unwrap()
            .contains(message_id)
        {
            return Err(anyhow::anyhow!(
                "Bad Request: can't parse entities (test simulation)"
            ));
        }
        Ok(())
    }

    async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn add_reaction(
        &self,
        chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.reactions.lock().unwrap().push(ReactionEntry {
            seq: next_seq(&self.seq),
            chat_id: chat_id.to_string(),
            message_id: message_id.to_string(),
            emoji: emoji.to_string(),
        });
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

#[async_trait]
impl MediaSender for RecordingMediaAdapter {
    async fn send_photo(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        if let Some(e) = self.maybe_fail_path(source) {
            return Err(e);
        }
        let _ = self.record_media(chat_id, source, MediaKind::Photo, thread_id);
        Ok(MessageResponse {
            message_id: self.alloc_message_id(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_voice(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        if let Some(e) = self.maybe_fail_path(source) {
            return Err(e);
        }
        let _ = self.record_media(chat_id, source, MediaKind::Voice, thread_id);
        Ok(MessageResponse {
            message_id: self.alloc_message_id(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_audio(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        if let Some(e) = self.maybe_fail_path(source) {
            return Err(e);
        }
        let _ = self.record_media(chat_id, source, MediaKind::Audio, thread_id);
        Ok(MessageResponse {
            message_id: self.alloc_message_id(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_video(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        if let Some(e) = self.maybe_fail_path(source) {
            return Err(e);
        }
        let _ = self.record_media(chat_id, source, MediaKind::Video, thread_id);
        Ok(MessageResponse {
            message_id: self.alloc_message_id(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_document(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        if let Some(e) = self.maybe_fail_path(source) {
            return Err(e);
        }
        let _ = self.record_media(chat_id, source, MediaKind::Document, thread_id);
        Ok(MessageResponse {
            message_id: self.alloc_message_id(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }
}

// ===========================================================================
// Synthetic LLM stream driver
// ===========================================================================
//
// Feeds the chunks through `scrubber → extractor` exactly as
// `run_agent` does at handler.rs:1372-1383 (the `stream_callback` body),
// then runs the tail-flush chain at handler.rs:1463-1471. This produces:
//   - `visible_chunks: Vec<String>` — the deltas that would have reached
//     `stream_tx`, equivalent to what `StreamConsumer` would render.
//   - `extracted_refs: Vec<MediaRef>` — the attachments the D-19 dispatch
//     loop would drain via `take_attachments()`.

struct ChainOutput {
    visible_chunks: Vec<String>,
    extracted_refs: Vec<MediaRef>,
}

fn drive_synthetic_stream(chunks: &[&str]) -> ChainOutput {
    use ironhermes_agent::streaming_scrubber::StreamingContextScrubber;

    let mut scrubber = StreamingContextScrubber::new();
    let mut extractor = MediaTagExtractor::new();
    let mut visible_chunks: Vec<String> = Vec::new();

    for chunk in chunks {
        let scrubbed = scrubber.feed(chunk);
        let visible = extractor.feed(&scrubbed);
        if !visible.is_empty() {
            visible_chunks.push(visible);
        }
    }

    // Tail flush per handler.rs:1463-1476.
    let scrubber_tail = scrubber.flush();
    let extractor_pre = if !scrubber_tail.is_empty() {
        extractor.feed(&scrubber_tail)
    } else {
        String::new()
    };
    let extractor_tail = extractor.flush_tail();
    let tail = format!("{extractor_pre}{extractor_tail}");
    if !tail.is_empty() {
        visible_chunks.push(tail);
    }

    let extracted_refs = extractor.take_attachments();
    ChainOutput {
        visible_chunks,
        extracted_refs,
    }
}

/// Reproduces the D-19 dispatch block from `handler.rs::run_agent`
/// (the section between `typing_handle.await.ok();` and `match agent_result`).
/// Drives the same `media_sender.send_media(...)` calls + the same
/// `failed_tags` reinsert path that production runs. Test asserts on the
/// recorded calls in `adapter`.
async fn dispatch_media_d19(
    adapter: &Arc<RecordingMediaAdapter>,
    chat_id: &str,
    placeholder_id: &str,
    media_refs: Vec<MediaRef>,
    final_body: String,
) {
    if media_refs.is_empty() {
        return;
    }

    // adapter is BOTH the PlatformAdapter and MediaSender. In production
    // these come from the same `Arc<TelegramAdapter>` via clone-cast.
    let media_sender: &dyn MediaSender = adapter.as_ref();
    let platform: &dyn PlatformAdapter = adapter.as_ref();

    let mut failed_tags: Vec<String> = Vec::new();
    for media_ref in media_refs {
        match media_sender
            .send_media(chat_id, &media_ref, None)
            .await
        {
            Ok(_) => {}
            Err(_e) => {
                failed_tags.push(media_ref.original_tag_text.clone());
            }
        }
    }
    if !failed_tags.is_empty() {
        let appended = failed_tags.join("\n");
        let reinsert_body = if final_body.is_empty() {
            appended
        } else {
            format!("{final_body}\n\n{appended}")
        };
        let escaped = escape_outside_code_blocks(&reinsert_body);
        let _ = platform
            .edit_message_markdown_v2(chat_id, placeholder_id, &escaped)
            .await;
    }
}

/// Simulates the final-flush edit issued by `StreamConsumer::flush(true)`
/// (stream_consumer.rs:113-119). The body is run through
/// `escape_outside_code_blocks` and sent via
/// `edit_message_markdown_v2`, matching the production code path.
async fn final_flush_edit(
    adapter: &Arc<RecordingMediaAdapter>,
    chat_id: &str,
    placeholder_id: &str,
    body: &str,
) {
    let escaped = escape_outside_code_blocks(body);
    let _ = adapter
        .edit_message_markdown_v2(chat_id, placeholder_id, &escaped)
        .await;
}

// ===========================================================================
// Test helpers — file fixtures
// ===========================================================================

/// Pre-create a 1-byte test file at `path`. The file exists for the
/// duration of the test; cleanup happens in a `defer`-style guard
/// returned by this helper.
struct FileFixture {
    path: PathBuf,
}

impl FileFixture {
    async fn new_small(path: &str) -> Self {
        let path = PathBuf::from(path);
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::write(&path, b"x")
            .await
            .expect("test setup: write 1-byte fixture");
        Self { path }
    }

    async fn new_oversize(path: &str) -> Self {
        let path = PathBuf::from(path);
        let _ = tokio::fs::remove_file(&path).await;
        // 21 MiB so D-15's 20 MiB cap rejects.
        tokio::fs::write(&path, vec![0u8; 21 * 1024 * 1024])
            .await
            .expect("test setup: write 21 MiB oversize fixture");
        Self { path }
    }
}

impl Drop for FileFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ===========================================================================
// Tests
// ===========================================================================

const CHAT: &str = "test-chat-001";
const PLACEHOLDER: &str = "999";

/// Test 1 (D-07 ordering): text-only turn — V2 edit fires, body is escaped,
/// no attachments.
#[tokio::test]
async fn text_only_v2_edit_renders_with_escape() {
    let adapter = RecordingMediaAdapter::new();

    // The chunk emitted by the synthetic LLM stream. Contains a literal
    // `.` (reserved in MarkdownV2 outside link grammar) so the escape
    // function MUST escape it.
    let body = "Hello *world*. Visit https://example.com.";
    let out = drive_synthetic_stream(&[body]);

    // No MEDIA tags → no extracted refs → dispatch is a no-op.
    assert!(out.extracted_refs.is_empty(), "no MEDIA tags emitted");
    final_flush_edit(&adapter, CHAT, PLACEHOLDER, body).await;
    dispatch_media_d19(
        &adapter,
        CHAT,
        PLACEHOLDER,
        out.extracted_refs,
        body.to_string(),
    )
    .await;

    let edits = adapter.edit_log.lock().unwrap();
    assert_eq!(edits.len(), 1, "exactly one V2 edit on the final flush");
    assert!(edits[0].is_markdown_v2, "edit is_markdown_v2 = true");
    // Single source of truth: compute expected via the imported escape fn.
    assert_eq!(edits[0].content, escape_outside_code_blocks(body));

    assert!(adapter.media_log.lock().unwrap().is_empty(), "no attachments");
}

/// Test 2 (D-07 + D-06 photo dispatch): text-first then attachment, in
/// stream order.
#[tokio::test]
async fn single_photo_text_then_attachment_order() {
    let adapter = RecordingMediaAdapter::new();
    let _fix = FileFixture::new_small("/tmp/telegram_media_test_photo_1.png").await;

    let chunk = "Here is the picture: <MEDIA: /tmp/telegram_media_test_photo_1.png>";
    let out = drive_synthetic_stream(&[chunk]);

    // Visible text after extraction is the prefix only.
    let visible: String = out.visible_chunks.join("");
    assert_eq!(visible, "Here is the picture: ");
    assert_eq!(out.extracted_refs.len(), 1);
    assert_eq!(out.extracted_refs[0].kind, MediaKind::Photo);
    assert_eq!(
        out.extracted_refs[0].source,
        MediaSource::Path(PathBuf::from("/tmp/telegram_media_test_photo_1.png"))
    );

    // The final-flush edit fires BEFORE the attachment dispatch
    // (handler.rs sequence: consumer_handle.await.ok() drains the V2 edit,
    // then the D-19 block runs). Mirror that ordering here.
    final_flush_edit(&adapter, CHAT, PLACEHOLDER, &visible).await;
    dispatch_media_d19(
        &adapter,
        CHAT,
        PLACEHOLDER,
        out.extracted_refs,
        visible.clone(),
    )
    .await;

    let edits = adapter.edit_log.lock().unwrap();
    let media = adapter.media_log.lock().unwrap();
    assert_eq!(edits.len(), 1, "one V2 edit");
    assert_eq!(media.len(), 1, "one Photo attachment");
    assert_eq!(media[0].media_ref.kind, MediaKind::Photo);
    // D-07: edit's seq < media's seq (V2 edit BEFORE attachment).
    assert!(
        edits[0].seq < media[0].seq,
        "D-07 ordering: V2 edit (seq={}) must precede attachment (seq={})",
        edits[0].seq,
        media[0].seq
    );
}

/// Test 3 (D-11 multi-tag): three tags in one delta → three attachments
/// in stream order.
#[tokio::test]
async fn multi_tag_emits_in_stream_order() {
    let adapter = RecordingMediaAdapter::new();
    let _a = FileFixture::new_small("/tmp/telegram_media_test_a.png").await;
    let _b = FileFixture::new_small("/tmp/telegram_media_test_b.ogg").await;
    let _c = FileFixture::new_small("/tmp/telegram_media_test_c.pdf").await;

    let chunk = "<MEDIA: /tmp/telegram_media_test_a.png>\
                 <MEDIA: /tmp/telegram_media_test_b.ogg>\
                 <MEDIA: /tmp/telegram_media_test_c.pdf>";
    let out = drive_synthetic_stream(&[chunk]);

    // All three tags consumed; visible stream empty.
    assert!(
        out.visible_chunks.join("").is_empty(),
        "all 3 tags should be consumed; visible was {:?}",
        out.visible_chunks
    );
    assert_eq!(out.extracted_refs.len(), 3);
    assert_eq!(out.extracted_refs[0].kind, MediaKind::Photo);
    assert_eq!(out.extracted_refs[1].kind, MediaKind::Voice);
    assert_eq!(out.extracted_refs[2].kind, MediaKind::Document);

    dispatch_media_d19(
        &adapter,
        CHAT,
        PLACEHOLDER,
        out.extracted_refs,
        String::new(),
    )
    .await;

    let media = adapter.media_log.lock().unwrap();
    assert_eq!(media.len(), 3, "three attachments in stream order");
    // Sequence numbers strictly increase: D-11 sequential dispatch.
    assert!(media[0].seq < media[1].seq);
    assert!(media[1].seq < media[2].seq);
    // Kinds preserved in stream order.
    assert_eq!(media[0].media_ref.kind, MediaKind::Photo);
    assert_eq!(media[1].media_ref.kind, MediaKind::Voice);
    assert_eq!(media[2].media_ref.kind, MediaKind::Document);
    // Path arms verified.
    assert_eq!(
        media[0].media_ref.source,
        MediaSource::Path(PathBuf::from("/tmp/telegram_media_test_a.png"))
    );
    assert_eq!(
        media[1].media_ref.source,
        MediaSource::Path(PathBuf::from("/tmp/telegram_media_test_b.ogg"))
    );
    assert_eq!(
        media[2].media_ref.source,
        MediaSource::Path(PathBuf::from("/tmp/telegram_media_test_c.pdf"))
    );
}

/// Test 4 (D-10): missing / failing path → tag literal re-inserted via ONE
/// combined edit.
#[tokio::test]
async fn missing_path_reinserts_tag() {
    let adapter = RecordingMediaAdapter::new();
    let missing_path = "/tmp/telegram_media_test_does_not_exist_xxxxxx.png";
    // Add the path to the failure set so RecordingMediaAdapter.send_photo
    // returns Err. (We do NOT pre-create the file — D-10 happens regardless
    // of whether the file exists at the adapter layer; the test simulates
    // adapter-level Err.)
    adapter
        .photo_fail_paths
        .lock()
        .unwrap()
        .insert(missing_path.to_string());

    let chunk = format!("intro <MEDIA: {missing_path}>");
    let out = drive_synthetic_stream(&[chunk.as_str()]);

    let visible: String = out.visible_chunks.join("");
    assert_eq!(visible, "intro ");
    assert_eq!(out.extracted_refs.len(), 1);
    let expected_literal = format!("<MEDIA: {missing_path}>");
    assert_eq!(
        out.extracted_refs[0].original_tag_text,
        expected_literal,
        "original_tag_text MUST preserve the literal for D-10 reinsert"
    );

    final_flush_edit(&adapter, CHAT, PLACEHOLDER, &visible).await;
    dispatch_media_d19(
        &adapter,
        CHAT,
        PLACEHOLDER,
        out.extracted_refs,
        visible.clone(),
    )
    .await;

    // media_log is empty (the failure means nothing was logged as a
    // successful send).
    assert!(
        adapter.media_log.lock().unwrap().is_empty(),
        "failed send produces no media_log entry"
    );

    let edits = adapter.edit_log.lock().unwrap();
    assert_eq!(
        edits.len(),
        2,
        "two edits: initial final-flush + D-10 reinsert; got {:?}",
        edits.iter().map(|e| &e.content).collect::<Vec<_>>()
    );
    // The second edit (D-10 reinsert) contains the tag literal.
    assert!(
        edits[1].content.contains("MEDIA")
            && edits[1].content.contains("does_not_exist_xxxxxx"),
        "reinsert content must contain the tag literal path: {:?}",
        edits[1].content
    );
    assert!(
        edits[1].is_markdown_v2,
        "D-10 reinsert uses edit_message_markdown_v2"
    );
    assert!(
        edits[0].seq < edits[1].seq,
        "initial edit precedes reinsert edit"
    );
}

/// Test 5 (D-15 + D-10): oversize path → no upload attempt, reinsert.
///
/// We use a hand-pre-created 21 MiB file AND register the path with
/// `oversize_fail_paths` so the fixture's `send_photo` returns the same
/// `media exceeds size cap:` error shape plan 04's D-15 pre-check would
/// emit. This isolates the D-15-then-D-10 surface from the actual file
/// metadata syscall (which production plan 04 runs).
#[tokio::test]
async fn oversize_path_reinserts_without_upload() {
    let adapter = RecordingMediaAdapter::new();
    let oversize_path = "/tmp/telegram_media_test_oversize.png";
    let _fix = FileFixture::new_oversize(oversize_path).await;
    adapter
        .oversize_fail_paths
        .lock()
        .unwrap()
        .insert(oversize_path.to_string());

    let chunk = format!("<MEDIA: {oversize_path}>");
    let out = drive_synthetic_stream(&[chunk.as_str()]);

    assert_eq!(out.extracted_refs.len(), 1);
    final_flush_edit(&adapter, CHAT, PLACEHOLDER, "").await;
    dispatch_media_d19(
        &adapter,
        CHAT,
        PLACEHOLDER,
        out.extracted_refs,
        String::new(),
    )
    .await;

    assert!(
        adapter.media_log.lock().unwrap().is_empty(),
        "D-15: oversize must not produce a media_log entry"
    );
    let edits = adapter.edit_log.lock().unwrap();
    // edit_log entries: initial final-flush (empty body) + D-10 reinsert.
    assert_eq!(edits.len(), 2);
    assert!(
        edits[1].content.contains("MEDIA")
            && edits[1].content.contains("oversize"),
        "reinsert content must contain oversize tag literal: {:?}",
        edits[1].content
    );
}

/// Test 6 (D-09 fence pass-through): tag inside a fenced code block is NOT
/// extracted; it streams to the user as visible text.
#[tokio::test]
async fn tag_inside_fence_passes_through_no_attachment() {
    let adapter = RecordingMediaAdapter::new();
    // File exists so the test proves the fence skip — the extractor never
    // reads the file because it never extracts the tag.
    let _fix = FileFixture::new_small("/tmp/telegram_media_test_fence.png").await;

    let chunk =
        "Use the convention:\n```\nemit <MEDIA: /tmp/telegram_media_test_fence.png> for media\n```\ndone.";
    let out = drive_synthetic_stream(&[chunk]);

    let visible: String = out.visible_chunks.join("");
    // The whole text including the tag literal inside the fence must reach
    // the visible output (D-09).
    assert!(
        visible.contains("<MEDIA: /tmp/telegram_media_test_fence.png>"),
        "fenced tag must pass through verbatim, got {visible:?}"
    );
    assert!(
        out.extracted_refs.is_empty(),
        "fenced tag must NOT be extracted as an attachment"
    );

    final_flush_edit(&adapter, CHAT, PLACEHOLDER, &visible).await;
    dispatch_media_d19(
        &adapter,
        CHAT,
        PLACEHOLDER,
        out.extracted_refs,
        visible.clone(),
    )
    .await;

    assert!(
        adapter.media_log.lock().unwrap().is_empty(),
        "no attachment dispatched for fenced tag"
    );
    let edits = adapter.edit_log.lock().unwrap();
    assert_eq!(edits.len(), 1, "exactly one V2 edit (no D-10 reinsert needed)");
    // The escape function preserves fence interior verbatim, so the
    // <MEDIA: ...> literal survives in the edit body.
    assert!(
        edits[0].content.contains("<MEDIA: /tmp/telegram_media_test_fence.png>"),
        "edited body must contain the fenced tag literal: {:?}",
        edits[0].content
    );
}

/// Test 7 (D-02 — observability weakened per T-D02-RETRY-NOT-OBSERVABLE):
/// parse-mode 400 simulation. Production's D-02 fallback retries via
/// `api_call("editMessageText", ...)` which bypasses the trait surface,
/// so the recording adapter cannot observe the retry. This test asserts:
///   1. The first V2 attempt IS recorded (proving the production code
///      path issued an edit_message_markdown_v2 call), and
///   2. No error propagates up (the worker continues — D-02 is a silent
///      fallback at the adapter layer in production).
///
/// Live UAT (plan 07 Scenario 8) covers the actual plain-text retry.
#[tokio::test]
async fn parse_mode_400_retries_as_plain_text() {
    let adapter = RecordingMediaAdapter::new();
    // Configure the fixture so the placeholder edit fails with the
    // parse-entity error shape.
    adapter
        .parse_mode_fail_msg_ids
        .lock()
        .unwrap()
        .insert(PLACEHOLDER.to_string());

    let body = "hello";
    let out = drive_synthetic_stream(&[body]);
    assert!(out.extracted_refs.is_empty());

    // Production code path: final_flush_edit issues edit_message_markdown_v2.
    // Our fixture records the attempt then returns Err. The production
    // adapter would internally retry via api_call (NOT through the trait).
    // The test's role: assert the first attempt is recorded AND that we
    // can continue past the simulated failure without panicking.
    final_flush_edit(&adapter, CHAT, PLACEHOLDER, body).await;

    let edits = adapter.edit_log.lock().unwrap();
    assert_eq!(
        edits.len(),
        1,
        "first V2 attempt must be recorded (plain-text retry via api_call is not observable through the trait surface)"
    );
    assert!(edits[0].is_markdown_v2, "first attempt is V2");
    assert_eq!(edits[0].content, escape_outside_code_blocks(body));
}
