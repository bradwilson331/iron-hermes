//! Phase 47.6 Plan 01 (P0-3): the Buzz platform adapter.
//!
//! `BuzzAdapter` is a real, production `PlatformAdapter` implementation over
//! the Nostr protocol. Plan 01's slice covered NIP-29 channels only (one
//! channel, one mention, one reply). Plan 05 (this file, expanded) adds the
//! NIP-17 gift-wrapped DM path in both directions.
//! Gated end-to-end on `#[cfg(feature = "buzz")]`: declared behind the same
//! gate in `lib.rs`, so a default build (no `buzz` feature) never compiles
//! this module or pulls in `nostr-sdk`.

use crate::adapter::{MessageHandler, PlatformAdapter};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use ironhermes_core::{
    ChannelTrust, MessageEvent, MessageResponse, Platform, PlatformGatewayConfig, current_profile,
};
use nostr_sdk::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// =============================================================================
// BoundedIdSet — in-memory FIFO dedup set (D-11: no new persistent storage)
// =============================================================================

/// A small bounded FIFO set of event ids. Used both for "have we already
/// dispatched this inbound event id" (P0-3 idempotency — a relay re-serves
/// stored events after the automatic reconnect, D-12) and for "which event
/// ids did we ourselves publish" (the reply-in-thread mention check, D-16).
/// Process-lifetime only; nothing here is persisted (D-11).
struct BoundedIdSet {
    order: VecDeque<EventId>,
    seen: HashSet<EventId>,
    cap: usize,
}

impl BoundedIdSet {
    fn new(cap: usize) -> Self {
        Self {
            order: VecDeque::with_capacity(cap),
            seen: HashSet::with_capacity(cap),
            cap,
        }
    }

    fn insert(&mut self, id: EventId) {
        if self.seen.contains(&id) {
            return;
        }
        if self.order.len() >= self.cap
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        self.order.push_back(id);
        self.seen.insert(id);
    }

    fn contains(&self, id: &EventId) -> bool {
        self.seen.contains(id)
    }

    fn len(&self) -> usize {
        self.seen.len()
    }

    /// Returns `true` if `id` was already present (a duplicate); inserts
    /// `id` into the set regardless.
    fn contains_or_insert(&mut self, id: EventId) -> bool {
        let was_present = self.contains(&id);
        self.insert(id);
        was_present
    }

    /// Snapshot of every id currently held, oldest first — the exact order
    /// [`persist_seen_events`] serializes to disk (Phase 47.6 Plan 08,
    /// T-47.6-08-RESTART).
    fn iter_ordered(&self) -> impl Iterator<Item = &EventId> {
        self.order.iter()
    }
}

// =============================================================================
// BoundedSenderMap — in-memory FIFO map: inbound channel event id -> sender
// (Phase 47.6 Plan 08, T-47.6-08-REPLY)
// =============================================================================

/// A small bounded FIFO map from an inbound CHANNEL message's event id to its
/// sender's pubkey. Populated only for channel-sourced messages that reach
/// the mention gate and get dispatched to the handler (never for DMs — a DM
/// reply never carries a `p` tag). Read by [`BuzzAdapter::send_message`]'s
/// `Channel` arm to attach a `p` tag naming the original sender when
/// replying in the same thread (D-16-adjacent, but this is a NEW behavior,
/// not a pre-existing convention).
///
/// A miss here (evicted, or the id was never a channel dispatch — e.g. a
/// thread_id supplied from somewhere else entirely) simply omits the `p`
/// tag; it never blocks the send. Mirrors [`BuzzAdapter::own_events`]'s
/// "false negative is an accepted trade" posture, for the same reason: this
/// is best-effort thread context, not a correctness-critical gate.
struct BoundedSenderMap {
    order: VecDeque<EventId>,
    map: HashMap<EventId, PublicKey>,
    cap: usize,
}

impl BoundedSenderMap {
    fn new(cap: usize) -> Self {
        Self {
            order: VecDeque::with_capacity(cap),
            map: HashMap::with_capacity(cap),
            cap,
        }
    }

    fn insert(&mut self, id: EventId, sender: PublicKey) {
        if self.map.contains_key(&id) {
            return;
        }
        if self.order.len() >= self.cap
            && let Some(oldest) = self.order.pop_front()
        {
            self.map.remove(&oldest);
        }
        self.order.push_back(id);
        self.map.insert(id, sender);
    }

    fn get(&self, id: &EventId) -> Option<PublicKey> {
        self.map.get(id).copied()
    }
}

// =============================================================================
// Persisted inbound dedupe — survives a gateway restart (Phase 47.6 Plan 08,
// T-47.6-08-RESTART)
// =============================================================================

/// Filename of the persisted Buzz inbound-dedupe event-id set, profile-scoped
/// under `get_hermes_home()/state/` (same profile-pivot `get_hermes_home`
/// already applies for every other per-profile file in this codebase).
const BUZZ_SEEN_EVENTS_FILENAME: &str = "buzz_seen_events.json";

/// Most recent N event ids retained on disk (and, since `run_buzz_adapter`
/// loads the file straight into its in-memory `seen_inbound` set, in memory
/// too). Bounds the file's growth across the adapter's entire lifetime —
/// without this, a long-lived gateway would grow the file unboundedly.
const BUZZ_SEEN_EVENTS_DISK_CAP: usize = 2048;

/// Path to the persisted dedupe file: `get_hermes_home()/state/buzz_seen_events.json`.
fn buzz_seen_events_path() -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join("state")
        .join(BUZZ_SEEN_EVENTS_FILENAME)
}

/// Load the persisted set of previously-dispatched inbound event ids from
/// disk into a fresh [`BoundedIdSet`] (cap [`BUZZ_SEEN_EVENTS_DISK_CAP`]), for
/// [`run_buzz_adapter`] to seed its `seen_inbound` set at loop start
/// (T-47.6-08-RESTART) — this is what makes a gateway restart stop
/// re-answering mentions the previous process already handled.
///
/// A missing file (first run, fresh profile, or a profile that has never
/// used Buzz) is the expected common case and is NOT logged as a warning — it
/// is not a degraded condition. A present-but-unreadable-or-unparseable file
/// IS logged at `warn!` and treated identically to a missing file (empty
/// set): corruption here must never crash the adapter (dedupe state is a
/// convenience, not a correctness-critical store — the in-memory
/// `BoundedIdSet` still protects the CURRENT process's own runtime even if
/// disk state is lost).
fn load_persisted_seen_events(path: &Path) -> BoundedIdSet {
    let mut set = BoundedIdSet::new(BUZZ_SEEN_EVENTS_DISK_CAP);
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return set,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "Buzz persisted dedupe file unreadable ({e}); starting with empty dedupe state"
            );
            return set;
        }
    };
    match serde_json::from_str::<Vec<String>>(&content) {
        Ok(ids) => {
            for id_hex in ids {
                match EventId::from_hex(&id_hex) {
                    Ok(id) => set.insert(id),
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            "Buzz persisted dedupe file contains an invalid event id ({e}); \
                             skipping entry"
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "Buzz persisted dedupe file is present but unparseable ({e}); starting with \
                 empty dedupe state"
            );
        }
    }
    set
}

/// Rewrite the persisted dedupe file from `set`'s current contents (oldest
/// first), via a temp-file-then-rename so a crash mid-write never leaves a
/// half-written file at the real path (same atomic-write posture as
/// `ApprovalsStore::save_to_disk` / `AuthStore::save_to_disk`, simplified —
/// this file holds no secret material, so it does not need the 0600
/// mode-before-write step those two use).
///
/// Called ONLY for event ids that pass every gate and are actually
/// dispatched to a spawned handler task — never for a relay self-loop echo
/// or a rejected/filtered event (see both call sites in
/// [`run_buzz_adapter`]) — so the file only ever grows for ids that
/// genuinely need cross-restart dedupe protection.
///
/// A write failure is logged and swallowed, never propagated: losing this
/// one persistence write degrades back to "in-memory-only for this event"
/// (the pre-fix behavior), never a reason to crash the adapter loop.
fn persist_seen_events(path: &Path, set: &BoundedIdSet) {
    let ids: Vec<String> = set.iter_ordered().map(|id| id.to_hex()).collect();
    let json = match serde_json::to_string(&ids) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("Buzz dedupe persistence: failed to serialize state: {e}");
            return;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            path = %parent.display(),
            "Buzz dedupe persistence: failed to create state dir ({e}); dedupe write skipped"
        );
        return;
    }
    let tmp_path = path.with_extension("json.tmp");
    let write_result =
        std::fs::write(&tmp_path, json.as_bytes()).and_then(|()| std::fs::rename(&tmp_path, path));
    if let Err(e) = write_result {
        tracing::warn!(
            path = %path.display(),
            "Buzz dedupe persistence: failed to write state ({e}); dedupe write skipped"
        );
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// Truncate a hex pubkey for logging — deny-gate log lines never print the
/// full sender identity.
fn truncate_pubkey(hex: &str) -> String {
    if hex.len() > 12 {
        format!("{}…", &hex[..12])
    } else {
        hex.to_string()
    }
}

/// Deny-all/whitelist gate shared by the channel access check (D-08, applied
/// only when `channel_trust` is `Closed`) and the DM access check (D-08,
/// applied unconditionally in BOTH trust modes — plan 05 task 1). One
/// implementation, so the two call sites can never drift apart.
///
/// `context` is a short noun phrase used only in the log line (e.g.
/// `"messages"`, `"DMs"`). Returns `true` if `sender_hex` is authorized.
fn whitelist_allows(whitelist: &[String], sender_hex: &str, context: &str) -> bool {
    if whitelist.is_empty() {
        tracing::warn!("Buzz whitelist is empty — denying all {context} (D-08 deny-all)");
        return false;
    }
    if !whitelist.iter().any(|w| w == sender_hex) {
        tracing::warn!(
            sender = %truncate_pubkey(sender_hex),
            "Buzz {context} sender not in whitelist, ignoring"
        );
        return false;
    }
    true
}

// =============================================================================
// Approval-command guard (Phase 47.6 Plan 06 Task 2, D-08/D-14)
// =============================================================================

/// True only when the first whitespace-delimited token of `content` is
/// EXACTLY `/approve` or `/deny`. Mirrors `handler.rs`'s shared `/approve`/
/// `/deny` intercept (`command_input.split_whitespace().next()`) exactly —
/// this guard and that intercept must never classify a message differently.
/// A guard laxer than the intercept blocks legitimate approval traffic; a
/// guard stricter than the intercept lets a command through unchecked,
/// which is the dangerous direction.
pub fn is_approval_command(content: &str) -> bool {
    matches!(
        content.split_whitespace().next(),
        Some("/approve") | Some("/deny")
    )
}

/// Why a Buzz approval command was rejected before reaching the handler
/// (Phase 47.6 Plan 06 Task 2). A typed enum rather than a free-text string
/// so tests assert on a value rather than on log formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuzzApprovalRejection {
    /// The command arrived on a channel event. D-14 specifies the approval
    /// prompt arrives as a DM and is answered as a DM; honouring a
    /// channel-posted approval command would let any channel participant
    /// release a gated command under `channel_trust: open`.
    Channel,
    /// The sender's pubkey is not in the whitelist. Checked in EVERY trust
    /// mode (D-08) — `channel_trust: open` widens ordinary channel-message
    /// access but must never widen approval authority.
    NotWhitelisted,
}

/// Guard applied only when [`is_approval_command`] is true, run AFTER the
/// existing access gate and BEFORE the mention gate / handler dispatch in
/// both the channel and DM inbound branches (Phase 47.6 Plan 06 Task 2):
///
/// 1. **DM only** — `is_channel: true` is always rejected.
/// 2. **Whitelist always** — re-validated here even on the DM branch (which
///    is already whitelist-gated above by [`whitelist_allows`]) because D-08
///    requires approvals to ALWAYS re-validate the sender regardless of
///    mode, and the channel branch's ordinary access gate skips the
///    whitelist entirely under `channel_trust: open`. Running the same
///    guard call at the same relative position in both branches is what
///    keeps them from silently drifting apart on this rule.
///
/// A rejection here is dropped by the caller — never answered, so an
/// attacker cannot use the response to confirm a pending approval exists —
/// and logged at `warn!` with the stable `T-47.6-06-SPOOF` marker
/// (mirroring the Telegram `callback_query` spoof guard's `T-36.3.8-SPOOF`
/// marker) so an operator can grep both surfaces with one pattern family.
fn approval_command_guard(
    is_channel: bool,
    whitelist: &[String],
    sender_hex: &str,
) -> Result<(), BuzzApprovalRejection> {
    if is_channel {
        tracing::warn!(
            sender = %truncate_pubkey(sender_hex),
            "Buzz approval command rejected: posted in a channel, DM-only (T-47.6-06-SPOOF)"
        );
        return Err(BuzzApprovalRejection::Channel);
    }
    if !whitelist_allows(whitelist, sender_hex, "approval commands") {
        tracing::warn!(
            sender = %truncate_pubkey(sender_hex),
            "Buzz approval command rejected: sender not in whitelist (T-47.6-06-SPOOF)"
        );
        return Err(BuzzApprovalRejection::NotWhitelisted);
    }
    Ok(())
}

/// The gift-wrap (`kind:1059`) DM subscription filter: constrained by a `#p`
/// tag equal to `own_pubkey`.
///
/// This constraint is not optional and not an optimization — Buzz's relay
/// enforces it at the wire level for p-gated kinds (including `kind:1059`)
/// and returns a `restricted:` rejection for a filter without it. A
/// subscription rejected this way produces zero events, which is
/// indistinguishable from "nobody sent a DM" unless the rejection is
/// surfaced (see [`run_buzz_adapter`]'s `warn!` on subscribe failure).
/// Exposed as its own function so it is directly testable without needing a
/// live relay round trip.
pub fn buzz_dm_subscription_filter(own_pubkey: PublicKey) -> Filter {
    Filter::new().kind(Kind::GiftWrap).pubkey(own_pubkey)
}

// =============================================================================
// BuzzChatTarget — outbound routing rule (plan 05 task 2)
// =============================================================================

/// Which outbound path a `chat_id` routes to: an encrypted direct message to
/// a specific Nostr identity, or a NIP-29 channel event.
///
/// **Routing rule** (the one convention every call site relies on
/// implicitly): a `chat_id` that parses as a bech32 `npub1…` public key, or
/// as a bare 64-character hex pubkey, is a direct message to that key;
/// anything else is a NIP-29 channel/group identifier. A malformed
/// `npub1`-prefixed string is a typed error from [`parse_buzz_chat_target`],
/// never a silent fall-through to `Channel` — posting a private reply into a
/// public channel by accident is the worst available failure here
/// (T-47.6-05-03).
///
/// **This routing rule is load-bearing for plan 06's approvals.** The shared
/// `/approve`/`/deny` intercept in `handler.rs` replies via
/// `adapter.send_message(&event.chat_id, ...)`. Task 1 sets an inbound DM's
/// `MessageEvent.chat_id` to the sender's bech32 npub, so that reply
/// round-trips through this same parser straight back into the
/// `DirectMessage` branch, automatically landing back in the DM instead of
/// some public channel. Do not change this routing rule without checking
/// that call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuzzChatTarget {
    /// A NIP-29 channel/group identifier (`#h` tag value).
    Channel(String),
    /// A direct message target, resolved to the peer's Nostr public key.
    DirectMessage(PublicKey),
}

/// Classify `chat_id` per [`BuzzChatTarget`]'s routing rule. See that type's
/// doc comment for the rule itself and why a malformed npub is a typed error
/// rather than a silent channel fall-through.
pub fn parse_buzz_chat_target(chat_id: &str) -> Result<BuzzChatTarget> {
    if chat_id.is_empty() {
        return Err(anyhow::anyhow!("Buzz: chat_id must not be empty"));
    }
    if chat_id.starts_with("npub1") {
        let pk = PublicKey::from_bech32(chat_id)
            .map_err(|e| anyhow::anyhow!("Buzz: malformed npub chat_id '{chat_id}': {e}"))?;
        return Ok(BuzzChatTarget::DirectMessage(pk));
    }
    if chat_id.len() == 64 && chat_id.chars().all(|c| c.is_ascii_hexdigit()) {
        let pk = PublicKey::from_hex(chat_id)
            .map_err(|e| anyhow::anyhow!("Buzz: invalid hex pubkey chat_id '{chat_id}': {e}"))?;
        return Ok(BuzzChatTarget::DirectMessage(pk));
    }
    Ok(BuzzChatTarget::Channel(chat_id.to_string()))
}

// =============================================================================
// TLS crypto provider (Phase 47.6 Plan 08 — production bug fix)
// =============================================================================

/// Install the process-level rustls `CryptoProvider` before this adapter's
/// first TLS (`wss://`) connect to the Buzz relay.
///
/// The workspace dependency graph enables BOTH rustls 0.23 crypto provider
/// features at once — `aws-lc-rs` (via reqwest -> hyper-rustls -> rustls,
/// pulled in elsewhere in the workspace) and `ring` (via the nostr-sdk
/// stack). With both features compiled in, rustls refuses to silently pick
/// one and panics the first time anything asks for the process-level
/// default (`rustls-0.23.40/src/crypto/mod.rs:249`) — for this adapter,
/// that's nostr-sdk's own `wss://` TLS connect reached from
/// [`BuzzAdapter::connect`]. Every in-process test drives a plain `ws://`
/// mock relay, so this never fired in CI; it only surfaces against a real
/// `wss://` relay. Mirrors the identical dual-provider fix already applied
/// for Edge TTS's websocket connect in
/// `ironhermes_tools::tts::edge::EdgeTtsProvider::synthesize`.
///
/// `install_default()` errors if a provider was already installed earlier
/// in the process — expected and fine (the goal is "some provider is
/// installed", not "this call installed it"), so the result is
/// intentionally discarded with `let _`. Always installs `aws-lc-rs` (never
/// `ring`) so the two existing install sites and this one agree on a single
/// provider — a mixed pair would let call order decide which one wins.
fn ensure_tls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

// =============================================================================
// BuzzAdapter — PlatformAdapter impl
// =============================================================================

/// Platform adapter for Buzz (Nostr) channels.
///
/// Holds the `nostr_sdk::Client` (relay pool, reconnect/backoff, NIP-42 AUTH
/// all owned by the SDK — see [`BuzzAdapter::connect`]) and the profile's own
/// `Keys` (identity — resolved once via [`crate::buzz_identity::resolve_buzz_nsec`]
/// at construction time by the caller).
pub struct BuzzAdapter {
    client: Arc<Client>,
    keys: Keys,
    relay_url: String,
    running: Arc<AtomicBool>,
    /// Event ids this adapter has itself published — bounded FIFO (cap 512,
    /// same bound as `run_buzz_adapter`'s inbound dedup set), process-
    /// lifetime only (D-11: no new persistent storage). Populated eagerly at
    /// publish time in [`Self::send_message`]/[`Self::send_dm`], and
    /// redundantly via the inbound self-loop echo in [`run_buzz_adapter`]
    /// (harmless double-insert — `BoundedIdSet::insert` is idempotent).
    /// Read by [`Self::delete_message`] (T-47.6-05-09: never issue a
    /// deletion for an event this adapter did not publish) and by
    /// `run_buzz_adapter`'s reply-in-thread mention-gate check (D-16).
    own_events: std::sync::Mutex<BoundedIdSet>,
    /// Inbound-channel-event-id -> sender pubkey (Phase 47.6 Plan 08,
    /// T-47.6-08-REPLY). See [`BoundedSenderMap`]'s own doc comment for
    /// scope and the accepted false-negative trade.
    inbound_senders: std::sync::Mutex<BoundedSenderMap>,
}

impl BuzzAdapter {
    /// Construct a new (not-yet-connected) adapter for `keys`, targeting
    /// `relay_url`. Call [`Self::connect`] to actually dial the relay.
    pub fn new(keys: Keys, relay_url: impl Into<String>) -> Self {
        let authenticator = SignerAuthenticator::new(keys.clone());
        let client = Client::builder().authenticator(authenticator).build();
        Self {
            client: Arc::new(client),
            keys,
            relay_url: relay_url.into(),
            running: Arc::new(AtomicBool::new(false)),
            own_events: std::sync::Mutex::new(BoundedIdSet::new(512)),
            inbound_senders: std::sync::Mutex::new(BoundedSenderMap::new(512)),
        }
    }

    /// Send a NIP-17 gift-wrapped direct message to `receiver`, recording
    /// the published event id in [`Self::own_events`]. Returns the
    /// published event's id — [`Self::send_message`]'s `DirectMessage`
    /// branch builds the full [`MessageResponse`] from this id plus its own
    /// (caller-supplied) `chat_id` string.
    ///
    /// Uses the SDK's own `PrivateDirectMessageBuilder` (Don't-Hand-Roll,
    /// RESEARCH.md "Don't Hand-Roll" row 2) — ephemeral-key wrapping, NIP-44
    /// versioned payload encryption and the padding scheme are never
    /// reimplemented here (T-47.6-05-04).
    async fn send_dm(
        &self,
        receiver: PublicKey,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<EventId> {
        let mut builder = PrivateDirectMessageBuilder::new(receiver, content);
        if let Some(thread) = thread_id {
            let thread_event_id = EventId::from_hex(thread).map_err(|e| {
                anyhow::anyhow!("Buzz send_dm: invalid thread_id '{thread}': {e}")
            })?;
            builder = builder.rumor_extra_tags([Tag::event(thread_event_id)]);
        }
        let event = builder
            .finalize(&self.keys)
            .map_err(|e| anyhow::anyhow!("Buzz send_dm failed to build gift wrap: {e}"))?;
        let output = self
            .client
            .send_event(&event)
            .await
            .map_err(|e| anyhow::anyhow!("Buzz send_dm failed: {e}"))?;
        // CR-02 fix (47.6 code review): mirror send_message's channel-branch
        // rejection check (buzz.rs:667-687 / the comment there explains the
        // history). A NIP-01 relay REJECTION (`["OK", <id>, false,
        // "<reason>"]`) of a gift-wrapped DM is not surfaced by `nostr-sdk`
        // as an `Err` from `send_event` — it comes back via
        // `output.success`/`output.failed` on an otherwise-`Ok` result.
        // Every Buzz DM (approval prompts, expiry notices, operator
        // confirmations, agent replies) routes through this function, so an
        // unchecked rejection here silently drops all of them while
        // reporting `delivered=true`.
        if output.success.is_empty() {
            let rejections: Vec<String> = output
                .failed
                .iter()
                .map(|(relay, reason)| format!("{relay}: {reason}"))
                .collect();
            let rejection_summary = if rejections.is_empty() {
                "no relay accepted the event (no rejection reason reported)".to_string()
            } else {
                rejections.join("; ")
            };
            tracing::warn!(
                event_id = %output.id().to_hex(),
                rejections = %rejection_summary,
                "Buzz send_dm: relay rejected DM publish — not recording as delivered"
            );
            return Err(anyhow::anyhow!(
                "Buzz send_dm: relay rejected the event: {rejection_summary}"
            ));
        }
        self.own_events
            .lock()
            .expect("own_events mutex poisoned")
            .insert(*output.id());
        Ok(*output.id())
    }

    /// Publish a NIP-01 `kind:0` metadata event carrying a readable display
    /// name for the agent identity, so it never appears in the Buzz member
    /// list as a bare npub (the ADR's headline value prop is "a member in
    /// the sidebar, @-mentionable"). `kind:0` is a NIP-01 replaceable event,
    /// so calling this on every connect is correct and needs no persisted
    /// state — the relay keeps only the latest one per author.
    pub async fn publish_profile_metadata(&self, display_name: &str) -> Result<()> {
        let event = Metadata::new()
            .name(display_name)
            .finalize(&self.keys)
            .map_err(|e| anyhow::anyhow!("Buzz profile metadata build failed: {e}"))?;
        self.client
            .send_event(&event)
            .await
            .map_err(|e| anyhow::anyhow!("Buzz profile metadata publish failed: {e}"))?;
        Ok(())
    }

    /// Dial the configured relay and mark the adapter running.
    ///
    /// NIP-42 AUTH (D-09) is handled entirely by `nostr-sdk`'s relay pool via
    /// the `SignerAuthenticator` installed in [`Self::new`] — this function
    /// does not, and must not, hand-roll the challenge/response. Reconnect
    /// with exponential backoff (D-12) is likewise owned by the SDK's own
    /// relay pool, not a custom retry loop here. A relay outage surfaces as
    /// a `tracing::warn!` degraded-state log naming the relay URL, matching
    /// how Telegram polling failures behave (see [`run_buzz_adapter`]'s own
    /// notification-stream-ended branch) — it does not abort this function
    /// or the caller's `start()`.
    pub async fn connect(&self) -> Result<()> {
        // Phase 47.6 Plan 08: must run before nostr-sdk's `wss://` connect
        // below — see `ensure_tls_crypto_provider`'s doc comment for why.
        ensure_tls_crypto_provider();
        self.client
            .add_relay(&self.relay_url)
            .await
            .map_err(|e| anyhow::anyhow!("Buzz add_relay({}) failed: {e}", self.relay_url))?;
        self.client.connect().await;
        self.running.store(true, Ordering::SeqCst);
        tracing::info!(relay_url = %self.relay_url, "Buzz adapter connected");
        Ok(())
    }

    /// The adapter's own Nostr public key, hex-encoded.
    pub fn pubkey_hex(&self) -> String {
        self.keys.public_key().to_hex()
    }

    /// Test-only observability hook: current size of the in-memory
    /// "own events" dedup set. Used by CR-02/WR-01 regression tests
    /// (`buzz_dm_roundtrip.rs`) to assert that a relay-rejected `send_dm`/
    /// `delete_message` publish does NOT get recorded as "our own event" —
    /// `own_events` itself has no other public accessor, and integration
    /// tests compile as a separate crate so `#[cfg(test)]` on the library
    /// side is not visible to them.
    #[doc(hidden)]
    pub fn own_events_len_for_test(&self) -> usize {
        self.own_events
            .lock()
            .expect("own_events mutex poisoned")
            .len()
    }
}

#[async_trait]
impl PlatformAdapter for BuzzAdapter {
    fn platform(&self) -> Platform {
        Platform::Buzz
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        match parse_buzz_chat_target(chat_id)? {
            BuzzChatTarget::Channel(channel) => {
                let mut builder = EventBuilder::new(Kind::Custom(9), content)
                    .tag(Tag::custom("h", [channel]));
                if let Some(thread) = thread_id {
                    let thread_event_id = EventId::from_hex(thread).map_err(|e| {
                        anyhow::anyhow!("Buzz send_message: invalid thread_id '{thread}': {e}")
                    })?;
                    // T-47.6-08-REPLY: Buzz's UI and Buzz's own agents both
                    // require the NIP-10 reply marker `["e", <id>, "",
                    // "reply"]` — verified live against real relay traffic.
                    // The bare `["e", <id>]` `Tag::event` composes is
                    // neither threadable in Buzz's UI nor ever matched by
                    // our own `replies_to_us` e-tag mention-gate check
                    // (`run_buzz_adapter`), so a plain `Tag::event` here
                    // silently produced un-threadable replies before this
                    // fix.
                    builder = builder.tag(
                        Tag::parse([
                            "e".to_string(),
                            thread_event_id.to_hex(),
                            String::new(),
                            "reply".to_string(),
                        ])
                        .map_err(|e| {
                            anyhow::anyhow!("Buzz send_message: failed to build reply e-tag: {e}")
                        })?,
                    );
                    // Best-effort `p` tag naming the original sender, when
                    // we recorded one for this event id (channel dispatches
                    // only — see `BoundedSenderMap`'s doc comment). A miss
                    // is not an error: the reply still publishes, just
                    // without the `p` tag.
                    if let Some(sender_pk) = self
                        .inbound_senders
                        .lock()
                        .expect("inbound_senders mutex poisoned")
                        .get(&thread_event_id)
                    {
                        builder = builder.tag(Tag::public_key(sender_pk));
                    }
                }
                let event = builder
                    .finalize(&self.keys)
                    .map_err(|e| anyhow::anyhow!("Buzz send_message failed to sign event: {e}"))?;
                let output = self
                    .client
                    .send_event(&event)
                    .await
                    .map_err(|e| anyhow::anyhow!("Buzz send_message failed: {e}"))?;
                // Live UAT fix (47.6 plan 08): a relay REJECTION (NIP-01
                // `["OK", <id>, false, "<reason>"]`) is NOT a transport
                // error — `Client::send_event`'s default `AckPolicy::all`
                // waits for the relay's OK and reports it via
                // `output.success`/`output.failed`, never as an `Err` here.
                // Before this fix, a rejected event still recorded itself in
                // `own_events` and returned `Ok(MessageResponse { .. })`,
                // which is exactly why the gateway logged
                // `turn-end: delivered via stream ... delivered=true` for a
                // reply the relay never actually accepted. `success.is_empty()`
                // means no relay accepted this publish — surface it as a
                // real failure instead.
                if output.success.is_empty() {
                    let rejections: Vec<String> = output
                        .failed
                        .iter()
                        .map(|(relay, reason)| format!("{relay}: {reason}"))
                        .collect();
                    let rejection_summary = if rejections.is_empty() {
                        "no relay accepted the event (no rejection reason reported)".to_string()
                    } else {
                        rejections.join("; ")
                    };
                    tracing::warn!(
                        event_id = %output.id().to_hex(),
                        chat_id = %chat_id,
                        rejections = %rejection_summary,
                        "Buzz send_message: relay rejected channel publish — not recording as delivered"
                    );
                    return Err(anyhow::anyhow!(
                        "Buzz send_message: relay rejected the event: {rejection_summary}"
                    ));
                }
                self.own_events
                    .lock()
                    .expect("own_events mutex poisoned")
                    .insert(*output.id());
                Ok(MessageResponse {
                    message_id: output.id().to_hex(),
                    chat_id: chat_id.to_string(),
                    platform: Platform::Buzz,
                })
            }
            BuzzChatTarget::DirectMessage(receiver) => {
                let event_id = self.send_dm(receiver, content, thread_id).await?;
                Ok(MessageResponse {
                    message_id: event_id.to_hex(),
                    chat_id: chat_id.to_string(),
                    platform: Platform::Buzz,
                })
            }
        }
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        // Buzz has no MarkdownV2 dialect — delegate to plain send_message
        // (the trait doc explicitly permits implementors that do not
        // distinguish the two to delegate).
        self.send_message(chat_id, content, thread_id).await
    }

    async fn edit_message(&self, chat_id: &str, message_id: &str, _content: &str) -> Result<()> {
        // D-13 — this is GENUINE new code, not copied from Discord/Slack:
        // both of those adapters perform a REAL native edit, so there is no
        // existing no-op to reuse here (RESEARCH.md Pitfall 1). Nostr events
        // are immutable — that is stated explicitly in the log line below so
        // a future reader does not mistake this for an unfinished stub.
        tracing::info!(
            chat_id,
            message_id,
            "Buzz edit_message: no-op (Nostr events are immutable)"
        );
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        self.edit_message(chat_id, message_id, content).await
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()> {
        let event_id = EventId::from_hex(message_id).map_err(|e| {
            anyhow::anyhow!("Buzz delete_message: invalid message_id '{message_id}': {e}")
        })?;

        // T-47.6-05-09: only ever issue a deletion for an event id THIS
        // adapter itself published.
        //
        // SCOPE OF THE GUARD (documented here, not discovered in
        // production): `own_events` is in-memory, bounded (cap 512, the
        // same bound `run_buzz_adapter`'s inbound dedup set uses), and
        // scoped to the CURRENT PROCESS LIFETIME (D-11: no new persistent
        // storage). It therefore covers only events published during this
        // run, and only the most recent 512 of them. Deleting a message this
        // same gateway published before a restart, or far enough back to
        // have been evicted, returns this same "not our event" error even
        // though the agent genuinely authored it. That is an accepted
        // trade: D-11 rules out new storage, and a false negative here
        // costs a failed delete, while a false positive would have the
        // agent issuing deletion requests for other people's events — a
        // strictly worse failure mode. Do NOT add persistence to widen this
        // window; that is a scope increase D-11 forecloses.
        if !self
            .own_events
            .lock()
            .expect("own_events mutex poisoned")
            .contains(&event_id)
        {
            return Err(anyhow::anyhow!(
                "Buzz delete_message: event {message_id} was not published by this adapter \
                 (chat_id={chat_id}, or falls outside the in-memory bounded own-event window); \
                 refusing to issue a deletion for a foreign/unknown event"
            ));
        }

        let deletion_event = EventDeletionRequest::new()
            .id(event_id)
            .finalize(&self.keys)
            .map_err(|e| anyhow::anyhow!("Buzz delete_message failed to sign deletion event: {e}"))?;
        let output = self
            .client
            .send_event(&deletion_event)
            .await
            .map_err(|e| anyhow::anyhow!("Buzz delete_message failed: {e}"))?;
        // WR-01 fix (47.6 code review): same unchecked-rejection gap as
        // CR-02's send_dm — a relay REJECTION of the deletion event is not
        // surfaced as an `Err` from `send_event`, only via
        // `output.success`/`output.failed`. Callers already treat
        // `delete_message` as best-effort (`let _ = ...`), so this widens
        // the caller-visible error rather than changing call sites.
        if output.success.is_empty() {
            let rejections: Vec<String> = output
                .failed
                .iter()
                .map(|(relay, reason)| format!("{relay}: {reason}"))
                .collect();
            let rejection_summary = if rejections.is_empty() {
                "no relay accepted the event (no rejection reason reported)".to_string()
            } else {
                rejections.join("; ")
            };
            tracing::warn!(
                event_id = %event_id.to_hex(),
                chat_id = %chat_id,
                rejections = %rejection_summary,
                "Buzz delete_message: relay rejected deletion — not reporting as successful"
            );
            return Err(anyhow::anyhow!(
                "Buzz delete_message: relay rejected the event: {rejection_summary}"
            ));
        }
        Ok(())
    }

    // add_reaction and send_chat_action: use the trait default no-op (same
    // convention as discord.rs/slack.rs).

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    // supports_in_place_edits: false — Nostr events are immutable, matching
    // the edit_message no-op directly above. See both this method's and
    // PlatformAdapter::supports_in_place_edits' own doc comments for why
    // these two must never drift apart.
    fn supports_in_place_edits(&self) -> bool {
        false
    }
}

// =============================================================================
// run_buzz_adapter — inbound loop: subscribe, gate, dispatch
// =============================================================================

/// Replay window applied ONLY to Buzz channel (`kind:9`) subscription
/// filters — each channel filter carries `.since(Timestamp::now() -
/// CHANNEL_REPLAY_GRACE)`.
///
/// **Root cause this bounds:** the Buzz relay's REQ-time historical delivery
/// is a direct DB query, not the live fan-out path (buzz repo:
/// `crates/buzz-relay/src/handlers/req.rs`), so on every (re)connect the
/// relay serves the FULL matching backlog regardless of subscription scope.
/// Combined with this adapter's in-memory (per-process, non-persistent)
/// `seen_inbound` dedupe set, every gateway restart previously re-fetched and
/// re-answered the entire channel history — the duplicate-reply defect
/// observed live (five simultaneous "Starting agent loop" answers to old
/// mentions on one restart).
///
/// **Trade-off:** a mention sent while the gateway was down longer than this
/// grace window is NOT replayed on the next reconnect. 15 minutes covers
/// ordinary restarts/redeploys without re-answering unbounded history.
/// Reconnects narrower than this window still replay cleanly because
/// `nostr-sdk` re-sends the identical filter (including this `since`) on
/// every automatic reconnect (D-12).
///
/// Deliberately NOT applied to the DM gift-wrap filter — see
/// [`buzz_dm_subscription_filter`]'s call site comment in
/// [`run_buzz_adapter`] for why a `since` there would silently drop DMs.
const CHANNEL_REPLAY_GRACE: Duration = Duration::from_secs(15 * 60);

/// Resolve the effective NIP-10 thread root to reply onto for an inbound
/// CHANNEL event (live UAT defect, 47.6 plan 08 restart — the relay rejected
/// a threaded reply with `"invalid: root tag does not match thread
/// ancestry"`).
///
/// Per the Buzz relay's own validation (buzz repo:
/// `crates/buzz-relay/src/handlers/ingest.rs`, `resolve_nip10_thread_meta`,
/// ~line 634): a lone `["e", X, "", "reply"]` tag (no `"root"` marker) is
/// interpreted as "X is both root AND parent". If X itself carries thread
/// metadata — i.e. X is itself a reply, as with Slack-style flat threading —
/// the relay computes a *different* effective root and rejects the publish.
/// Any event the relay has already ACCEPTED whose own `e`-tag carries the
/// `"reply"` marker therefore necessarily points at the TRUE thread root
/// already (same for `"root"`); replying onto that resolved id instead of
/// onto `event`'s own id reproduces the flat-thread convention Buzz's own
/// desktop client and agents use.
///
/// Precedence (checked across every `e`-tag on `event`, order-independent):
/// 1. an `e`-tag marked `"root"` — its id is the thread root, verbatim.
/// 2. else an `e`-tag marked `"reply"` — per the relay semantics above, that
///    id IS the thread root when no explicit `"root"` tag is also present.
/// 3. else `event`'s own id — this message is top-level, so it becomes the
///    thread root for whatever replies to it later.
///
/// A malformed/foreign `e`-tag (invalid hex, wrong shape) is skipped, not
/// fatal — it just doesn't contribute a candidate, same as "no e-tag at
/// all" falls through to (3).
fn resolve_thread_root(event: &Event) -> EventId {
    let mut reply_marker_fallback: Option<EventId> = None;
    for tag in event.tags.iter() {
        if tag.single_letter_tag() != Some(SingleLetterTag::LOWERCASE_E) {
            continue;
        }
        let Ok(Nip10Tag::Event { id, marker, .. }) = Nip10Tag::parse(tag.as_slice()) else {
            // Malformed hex or otherwise unparseable e-tag — not a candidate,
            // fall through to the next tag (and ultimately to event.id).
            continue;
        };
        match marker {
            Some(Marker::Root) => return id,
            Some(Marker::Reply) if reply_marker_fallback.is_none() => {
                reply_marker_fallback = Some(id);
            }
            _ => {}
        }
    }
    reply_marker_fallback.unwrap_or(event.id)
}

/// Start the Buzz adapter's inbound loop: subscribe to the configured
/// channels AND to gift-wrapped DMs addressed to this identity, apply the
/// deny-all/mention gates (D-08/D-16), and dispatch accepted messages to
/// `handler`.
///
/// Blocks until either the relay's notification stream ends (connection lost
/// for good) or `cancel` is signalled, which returns `Ok(())` for clean
/// shutdown. Every accepted message is dispatched on its OWN spawned task
/// (mirrors `slack.rs`'s push-event callback) so a slow agent turn never
/// stalls the relay subscription (P0-3 concurrency); handler errors are
/// logged, never propagated.
pub async fn run_buzz_adapter(
    adapter: Arc<BuzzAdapter>,
    config: PlatformGatewayConfig,
    handler: Arc<dyn MessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    if config.channel_trust == ChannelTrust::Open {
        // D-08: Open is never silently the default — enabling it must be
        // loud. `warn!`, not `debug!`, naming the relay URL.
        tracing::warn!(
            relay_url = %adapter.relay_url,
            "Buzz channel_trust is OPEN — channel membership alone now implies interaction rights (D-08)"
        );
    }

    // MUST subscribe to notifications BEFORE issuing the relay `subscribe()`
    // call. A `tokio::sync::broadcast` receiver created after a message is
    // already in flight never sees it — verified directly against this exact
    // nostr-sdk version (see buzz_relay_roundtrip.rs's harness doc comment
    // for the full root-cause writeup; plan 01's SUMMARY records both the
    // failure and the fix). Getting this ordering right in production, not
    // just in the test harness, is why this call comes first here too.
    let mut notifications = adapter.client.notifications();

    let own_pubkey = adapter.keys.public_key();

    // Profile metadata (COVERAGE.md kind:0 row, plan 05 task 3): publish a
    // readable display name so the agent never appears in the Buzz member
    // list as a bare npub. `connect()` itself does not take a config
    // parameter (its call site in runner.rs predates this task and is out
    // of scope to change), so this — the adapter's actual startup sequence
    // — is where "on successful connect" is implemented in practice. Prefer
    // an operator-configured name (`platforms.buzz.display_name` in
    // config.yaml, landing in `PlatformGatewayConfig.extra` via its
    // `#[serde(flatten)]` catch-all) over a hardcoded string; fall back to
    // the active profile name so the field is never empty. A publish
    // failure is logged and non-fatal — it must not take down the whole
    // adapter loop over a cosmetic issue.
    let display_name = config
        .extra
        .get("display_name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(current_profile);
    if let Err(e) = adapter.publish_profile_metadata(&display_name).await {
        tracing::warn!("Buzz profile metadata publish failed (non-fatal): {e}");
    }

    // ONE subscribe() call PER channel — never a single REQ whose `#h`
    // tag lists every configured channel. Buzz's relay derives a
    // subscription's channel scope via `extract_channel_id_from_filters`
    // (buzz repo: `crates/buzz-relay/src/handlers/req.rs:1029`): when a
    // filter's `#h` values resolve to MORE THAN ONE distinct channel UUID,
    // that function returns `None` and the subscription is registered as
    // GLOBAL instead of channel-scoped. The relay's fan-out invariant
    // (buzz repo: `crates/buzz-relay/src/subscription.rs`, closing comment on
    // `fan_out_scoped`) is explicit: "Global subscriptions (channel_id =
    // None) do NOT receive channel-scoped events" — live `kind:9` channel
    // events are only ever delivered via `channel_kind_index` /
    // `channel_wildcard_index`, which only a channel-scoped subscription
    // joins (`register_scoped`). A single multi-channel REQ therefore never
    // receives live events at all; only the REQ-time historical DB query
    // (unaffected by fan-out scoping) made it look like the subscription
    // worked. Each `client.subscribe()` call gets its own `nostr-sdk`
    // subscription id, so N single-channel REQs register as N distinct
    // channel-scoped subscriptions on the relay — this is the fix.
    if !config.channels.is_empty() {
        for channel in &config.channels {
            let filter = Filter::new()
                .kind(Kind::Custom(9))
                .custom_tag(SingleLetterTag::LOWERCASE_H, channel.clone())
                .since(Timestamp::now() - CHANNEL_REPLAY_GRACE);
            let sub_output = adapter
                .client
                .subscribe(filter)
                .await
                .map_err(|e| anyhow::anyhow!("Buzz subscribe failed for channel {channel}: {e}"))?;
            // Per-channel failure surfacing (T-47.6-05-07 precedent, same as
            // the DM subscription just below): a rejected channel
            // subscription produces zero events, indistinguishable from "no
            // activity in this channel" unless logged.
            for (relay_url, reason) in &sub_output.failed {
                tracing::warn!(
                    relay_url = %relay_url,
                    channel = %channel,
                    "Buzz channel subscription rejected by relay (a rejected #h-scoped \
                     subscription looks identical to \"no channel activity\" unless logged): {reason}"
                );
            }
        }
    } else {
        tracing::warn!("Buzz adapter has no configured channels — nothing to subscribe to");
    }

    // DM subscription (plan 05 task 1) — MUST carry the #p constraint or
    // Buzz's relay rejects it outright (see buzz_dm_subscription_filter's
    // doc comment). A relay-side rejection is surfaced at `warn!` because a
    // rejected subscription produces zero events, which is otherwise
    // indistinguishable from "nobody DMed us" (T-47.6-05-07).
    //
    // MUST NEVER gain a `.since(...)` bound (unlike the channel filters
    // above): NIP-17 gift wraps carry a randomized `created_at` (up to ~2
    // days of skew) specifically to defeat timestamp correlation attacks —
    // bounding this filter by time would silently drop legitimate DMs whose
    // gift-wrap timestamp happens to fall outside the window.
    let dm_filter = buzz_dm_subscription_filter(own_pubkey);
    let dm_sub_output = adapter
        .client
        .subscribe(dm_filter)
        .await
        .map_err(|e| anyhow::anyhow!("Buzz DM subscribe failed: {e}"))?;
    for (relay_url, reason) in &dm_sub_output.failed {
        tracing::warn!(
            relay_url = %relay_url,
            "Buzz DM subscription rejected by relay (a rejected #p-gated subscription looks \
             identical to \"no DMs\" unless logged): {reason}"
        );
    }

    // T-47.6-08-RESTART: seed the in-memory idempotency set from disk so a
    // gateway restart does not re-dispatch mentions the previous process
    // already answered (the live-observed defect: a restart replayed 4
    // already-answered mentions and started 4 agent loops simultaneously).
    // Cap raised from the pre-fix 512 to `BUZZ_SEEN_EVENTS_DISK_CAP` (2048)
    // to match the on-disk bound below — one structure, one cap, no risk of
    // the in-memory and on-disk views silently drifting apart.
    let seen_events_path = buzz_seen_events_path();
    let mut seen_inbound = load_persisted_seen_events(&seen_events_path);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("Buzz adapter loop cancelled");
                return Ok(());
            }
            notification = notifications.next() => {
                let Some(notification) = notification else {
                    tracing::warn!(
                        relay_url = %adapter.relay_url,
                        "Buzz notification stream ended — relay connection lost (degraded state)"
                    );
                    return Ok(());
                };
                let ClientNotification::Message { message, .. } = notification else {
                    continue;
                };
                let RelayMessage::Event { event, .. } = message.as_ref() else {
                    continue;
                };
                let event = event.clone().into_owned();

                // Self-loop guard (T-47.6-06): events authored by our own
                // pubkey are never re-dispatched to the handler — an
                // attacker replaying the agent's own signed output could
                // otherwise drive a self-amplifying loop. Reuse this branch
                // to feed the reply-in-thread tracker: our own successfully
                // published events are exactly the ids a later `e`-tag reply
                // check needs to recognise. (Gift-wrap events we send are
                // signed by a fresh ephemeral key each time, never our own
                // pubkey, so this branch is a no-op for outbound DMs.)
                if event.pubkey == own_pubkey {
                    adapter
                        .own_events
                        .lock()
                        .expect("own_events mutex poisoned")
                        .insert(event.id);
                    continue;
                }

                // Idempotency (P0-3): a relay re-serves stored events after
                // the automatic reconnect (D-12) — without this the agent
                // answers the same message again every time the connection
                // drops. Applies to both channel messages and gift-wrapped
                // DMs (keyed on the outer event id in both cases).
                if seen_inbound.contains_or_insert(event.id) {
                    continue;
                }

                if event.kind == Kind::GiftWrap {
                    // ---------------------------------------------------
                    // DM receive path (plan 05 task 1)
                    // ---------------------------------------------------
                    // Unwrap FIRST — nothing below this point may inspect
                    // `event.content` (the outer content is double-encrypted
                    // rumor -> seal -> gift wrap with a one-time ephemeral
                    // signing key, specifically so relay operators cannot
                    // read content or correlate sender/receiver).
                    let unwrapped = match UnwrappedGift::from_gift_wrap(&adapter.keys, &event) {
                        Ok(u) => u,
                        Err(e) => {
                            // A message we cannot decrypt is normal on a
                            // public relay (someone may address a gift wrap
                            // to us that we are not the true recipient of).
                            // Log the outer event id ONLY — never content,
                            // never key material — and keep the loop alive.
                            tracing::debug!(
                                event_id = %event.id.to_hex(),
                                "Buzz DM gift-wrap unwrap failed (not fatal): {e}"
                            );
                            continue;
                        }
                    };

                    // The useful sender identity is the RUMOR's author
                    // (`unwrapped.sender`, verified equal to the seal's
                    // author internally by the SDK), never the outer
                    // gift-wrap event's author — the latter is a one-time
                    // ephemeral key and is meaningless for access control
                    // (T-47.6-05-01).
                    let sender_hex = unwrapped.sender.to_hex();
                    let sender_npub = unwrapped
                        .sender
                        .to_bech32()
                        .unwrap_or_else(|_| sender_hex.clone());

                    // D-08 (T-47.6-05-02): the whitelist gates DMs in BOTH
                    // trust modes — channel_trust never widens DM access.
                    // Same shared helper the channel gate below uses.
                    if !whitelist_allows(&config.whitelist, &sender_hex, "DMs") {
                        continue;
                    }

                    // T-47.6-06 (D-08/D-14 approval guard): re-validate an
                    // approval command's sender even on the DM branch, which
                    // is already whitelist-gated above — D-08 requires
                    // approvals to ALWAYS re-validate regardless of mode,
                    // and running this one guard call at the same relative
                    // position as the channel branch below is what keeps
                    // the two paths from silently drifting apart on that
                    // rule. Ordinary (non-approval) DM traffic is
                    // unaffected — is_approval_command is false for it.
                    if is_approval_command(&unwrapped.rumor.content)
                        && approval_command_guard(false, &config.whitelist, &sender_hex).is_err()
                    {
                        continue;
                    }

                    // D-16: no mention gate for DMs — every whitelisted DM
                    // reaches the handler.
                    let rumor = unwrapped.rumor;
                    let replied_to_id = rumor.tags.event_ids().next().map(|id| id.to_hex());
                    let message_id = rumor
                        .id
                        .map(|id| id.to_hex())
                        .unwrap_or_else(|| event.id.to_hex());

                    let msg_event = MessageEvent {
                        platform: Platform::Buzz,
                        message_id,
                        chat_id: sender_npub,
                        sender_id: sender_hex,
                        content: rumor.content.clone(),
                        attachments: vec![],
                        thread_id: replied_to_id.clone(),
                        chat_type: "dm".to_string(),
                        chat_name: None,
                        sender_name: None,
                        replied_to_id,
                    };

                    // T-47.6-08-RESTART: persist NOW, right before dispatch
                    // — never for a relay echo or a filtered/rejected event
                    // (both continue above, well before this point). Uses
                    // the current in-memory `seen_inbound` snapshot, which
                    // already contains `event.id` (inserted unconditionally
                    // above at the idempotency check) plus every other id
                    // still within the bounded window; a full-set rewrite is
                    // simpler and no less correct than tracking "dispatched
                    // only" separately — the extra remembered ids (rejected/
                    // filtered) are harmless, since replaying them again
                    // would be rejected identically either way.
                    persist_seen_events(&seen_events_path, &seen_inbound);

                    let handler = handler.clone();
                    let adapter_for_task: Arc<dyn PlatformAdapter> = adapter.clone();
                    let child_cancel = cancel.child_token();
                    tokio::spawn(async move {
                        if let Err(e) = handler.handle(&msg_event, adapter_for_task, child_cancel).await {
                            tracing::error!("Buzz DM handler error: {e:#}");
                        }
                    });
                    continue;
                }

                // ---------------------------------------------------
                // Channel receive path (plan 01, unchanged)
                // ---------------------------------------------------
                // Access gate (D-08): Closed (default) requires the sender's
                // pubkey hex to be in the whitelist; empty whitelist denies
                // all. Open treats channel membership as sufficient.
                let sender_hex = event.pubkey.to_hex();
                match config.channel_trust {
                    ChannelTrust::Open => {}
                    ChannelTrust::Closed => {
                        if !whitelist_allows(&config.whitelist, &sender_hex, "messages") {
                            continue;
                        }
                    }
                }

                // T-47.6-06 (D-08/D-14 approval guard): approval commands
                // are DM-only and always re-validated against the
                // whitelist, in EVERY trust mode — this is the check that
                // matters most here, since `channel_trust: open` above
                // skips the whitelist entirely for ordinary channel
                // messages. Must run BEFORE the mention gate below: a
                // DM-shaped approval command posted with a valid mention
                // would otherwise pass straight through to dispatch.
                if is_approval_command(&event.content)
                    && approval_command_guard(true, &config.whitelist, &sender_hex).is_err()
                {
                    continue;
                }

                // Mention gate (D-16): proceed only if the event carries a
                // `p`-tag naming us, OR an `e`-tag referencing an event id we
                // previously published (replied-to-in-thread). NOT literal
                // "@name" substring matching — Buzz's mention convention is
                // a tag, not text (RESEARCH Anti-Patterns).
                let mentions_us = event.tags.public_keys().any(|pk| pk == own_pubkey);
                let replies_to_us = event.tags.event_ids().any(|id| {
                    adapter
                        .own_events
                        .lock()
                        .expect("own_events mutex poisoned")
                        .contains(&id)
                });
                if !mentions_us && !replies_to_us {
                    continue;
                }

                let chat_id = event
                    .tags
                    .iter()
                    .find(|t| t.single_letter_tag() == Some(SingleLetterTag::LOWERCASE_H))
                    .and_then(|t| t.content())
                    .unwrap_or_default()
                    .to_string();
                // T-47.6-08-RESTART (live UAT defect, plan 08): resolve the
                // THREAD ROOT, not just the first e-tag id — see
                // `resolve_thread_root`'s doc comment for why a bare "first
                // e-tag" reply target gets rejected by the relay whenever the
                // triggering message is itself mid-thread (Slack-style flat
                // threading, e.g. Buzz desktop clients). `handler.rs` reads
                // this back off `MessageEvent::thread_id` to build the
                // `StreamConsumer::with_reply_to` value that ultimately
                // becomes the outbound reply's `e`-tag.
                let thread_root = resolve_thread_root(&event);
                let replied_to_id = Some(thread_root.to_hex());

                // T-47.6-08-REPLY: record this event's sender BEFORE
                // dispatch so a later reply threaded onto the resolved
                // thread root (via `send_message`'s `thread_id`) can attach a
                // `p` tag naming the sender of whichever message triggered
                // that reply. Channel-only — DMs never populate this map
                // (see `BoundedSenderMap`'s doc comment).
                //
                // T-47.6-08-RESTART (thread-root fix): keyed by the
                // RESOLVED THREAD ROOT, not this event's own id — a later
                // `send_message` call's `thread_id` is now always the
                // resolved root (never a mid-thread parent), so the
                // sender-map's insert key must match that exact lookup key
                // or every lookup silently misses. This also keeps the
                // recorded sender correct across an entire flat thread:
                // every message in the same thread resolves to the same
                // root id, so whoever most recently posted into the thread
                // is who gets p-tagged on our next reply — an accepted
                // best-effort approximation (a miss here never blocks the
                // send; see `BoundedSenderMap`'s own doc comment).
                adapter
                    .inbound_senders
                    .lock()
                    .expect("inbound_senders mutex poisoned")
                    .insert(thread_root, event.pubkey);

                let msg_event = MessageEvent {
                    platform: Platform::Buzz,
                    message_id: event.id.to_hex(),
                    chat_id,
                    sender_id: sender_hex,
                    content: event.content.clone(),
                    attachments: vec![],
                    thread_id: replied_to_id.clone(),
                    chat_type: "channel".to_string(),
                    chat_name: None,
                    sender_name: None,
                    replied_to_id,
                };

                // T-47.6-08-RESTART: see the identical comment on the DM
                // dispatch path above — same trigger point, same rationale.
                persist_seen_events(&seen_events_path, &seen_inbound);

                let handler = handler.clone();
                let adapter_for_task: Arc<dyn PlatformAdapter> = adapter.clone();
                let child_cancel = cancel.child_token();
                tokio::spawn(async move {
                    if let Err(e) = handler.handle(&msg_event, adapter_for_task, child_cancel).await {
                        tracing::error!("Buzz handler error: {e:#}");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 47.6 Plan 08 regression test: a live gateway run against a real
    /// `wss://` Buzz relay panicked at `rustls-0.23.40/src/crypto/mod.rs:249`
    /// because the workspace enables both the `aws-lc-rs` and `ring` rustls
    /// crypto provider features, and nothing had installed a process-level
    /// default before nostr-sdk's first TLS connect. Asserts
    /// `ensure_tls_crypto_provider` actually leaves a default installed, so
    /// this incident can't silently regress.
    #[test]
    fn tls_crypto_provider_installed_before_buzz_connect() {
        ensure_tls_crypto_provider();
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "expected a process-level rustls CryptoProvider to be installed"
        );
    }

    /// Phase 47.6 Plan 08 (T-47.6-08-RESTART): a missing dedupe file (first
    /// run / fresh profile) must load as an empty set with no warning.
    #[test]
    fn load_persisted_seen_events_missing_file_is_empty() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("does-not-exist.json");
        let set = load_persisted_seen_events(&path);
        assert_eq!(set.iter_ordered().count(), 0);
    }

    /// Phase 47.6 Plan 08 (T-47.6-08-RESTART): a present-but-unparseable
    /// dedupe file must degrade to an empty set (warn, never panic/crash the
    /// adapter) — this is the plan's explicit "corrupt state file" test.
    #[test]
    fn load_persisted_seen_events_corrupt_file_degrades_to_empty() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("buzz_seen_events.json");
        std::fs::write(&path, b"not valid json{{{").expect("write corrupt file");
        let set = load_persisted_seen_events(&path);
        assert_eq!(set.iter_ordered().count(), 0);
    }

    /// A valid persisted file round-trips through load/persist unchanged.
    #[test]
    fn persist_and_reload_seen_events_round_trips() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("buzz_seen_events.json");

        let mut set = BoundedIdSet::new(BUZZ_SEEN_EVENTS_DISK_CAP);
        let ids: Vec<EventId> = (0..3)
            .map(|_| {
                EventBuilder::new(Kind::TextNote, "x")
                    .finalize(&Keys::generate())
                    .expect("sign failed")
                    .id
            })
            .collect();
        for id in &ids {
            set.insert(*id);
        }
        persist_seen_events(&path, &set);

        let reloaded = load_persisted_seen_events(&path);
        let reloaded_ids: Vec<EventId> = reloaded.iter_ordered().copied().collect();
        assert_eq!(reloaded_ids, ids);
    }

    // -------------------------------------------------------------------
    // resolve_thread_root — live UAT fix, 47.6 plan 08 restart. The relay
    // rejected a threaded reply with "invalid: root tag does not match
    // thread ancestry" whenever the triggering message was itself mid-
    // thread; these pin the fixed precedence: root marker > reply marker >
    // event's own id, with a malformed e-tag never panicking.
    // -------------------------------------------------------------------

    /// Build a signed `kind:9` channel event carrying exactly the given raw
    /// `e`-tags (each a `["e", id_hex, relay_hint, marker]` quadruple) plus
    /// an `#h` tag, so `resolve_thread_root` can be exercised against a real
    /// signed `Event` without needing the relay harness.
    fn build_event_with_e_tags(e_tags: &[[&str; 4]]) -> Event {
        let mut builder =
            EventBuilder::new(Kind::Custom(9), "test").tag(Tag::custom("h", ["chan".to_string()]));
        for tag in e_tags {
            builder = builder.tag(
                Tag::parse([tag[0], tag[1], tag[2], tag[3]]).expect("failed to build e-tag"),
            );
        }
        builder.finalize(&Keys::generate()).expect("event signing failed")
    }

    /// (a) An `e`-tag marked `"root"` resolves to that id, verbatim.
    #[test]
    fn resolve_thread_root_uses_root_marker() {
        let root_id = EventBuilder::new(Kind::TextNote, "root")
            .finalize(&Keys::generate())
            .expect("sign failed")
            .id;
        let event =
            build_event_with_e_tags(&[["e", &root_id.to_hex(), "", "root"]]);
        assert_eq!(resolve_thread_root(&event), root_id);
    }

    /// (b) Only a `"reply"` marker present (no `"root"`) resolves to that
    /// id — per the Buzz relay's own semantics, a lone reply-marked e-tag IS
    /// the thread root when no explicit root tag also exists.
    #[test]
    fn resolve_thread_root_falls_back_to_reply_marker() {
        let reply_id = EventBuilder::new(Kind::TextNote, "reply-target")
            .finalize(&Keys::generate())
            .expect("sign failed")
            .id;
        let event =
            build_event_with_e_tags(&[["e", &reply_id.to_hex(), "", "reply"]]);
        assert_eq!(resolve_thread_root(&event), reply_id);
    }

    /// Root marker takes precedence over a reply marker regardless of tag
    /// order — the real NIP-10 nested-reply shape carries both.
    #[test]
    fn resolve_thread_root_prefers_root_over_reply() {
        let root_id = EventBuilder::new(Kind::TextNote, "root")
            .finalize(&Keys::generate())
            .expect("sign failed")
            .id;
        let parent_id = EventBuilder::new(Kind::TextNote, "parent")
            .finalize(&Keys::generate())
            .expect("sign failed")
            .id;
        let event = build_event_with_e_tags(&[
            ["e", &parent_id.to_hex(), "", "reply"],
            ["e", &root_id.to_hex(), "", "root"],
        ]);
        assert_eq!(resolve_thread_root(&event), root_id);
    }

    /// (c) No `e`-tags at all — a top-level message becomes its own thread
    /// root.
    #[test]
    fn resolve_thread_root_no_e_tags_falls_back_to_own_id() {
        let event = build_event_with_e_tags(&[]);
        assert_eq!(resolve_thread_root(&event), event.id);
    }

    /// (d) A malformed (non-hex) `e`-tag id must never panic — it is simply
    /// not a candidate, and resolution falls through to the event's own id.
    #[test]
    fn resolve_thread_root_malformed_hex_falls_back_to_own_id() {
        let event = build_event_with_e_tags(&[["e", "not-a-valid-hex-id", "", "reply"]]);
        assert_eq!(resolve_thread_root(&event), event.id);
    }
}
