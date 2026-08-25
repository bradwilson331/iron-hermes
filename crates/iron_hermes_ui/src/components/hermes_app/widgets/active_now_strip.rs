//! Phase 50.1 Plan 08 (D-19): the Active-now presence strip — a read-only
//! summary rail mounted above the roster grid, showing which bots are
//! doing something right now.
//!
//! Two bots qualify: the live profile when a turn is in flight, and any
//! bot whose bot-meta store write timestamp (`BotMeta::updated_at_ms`,
//! refreshed on every `save_bot_meta` call including the D-13 preview
//! write-back on turn completion) falls inside a rolling
//! [`ACTIVE_WINDOW_SECONDS`] window. Live turn events are the stated end
//! goal for presence; the timestamp window is the staging step, and
//! [`PresenceSource`] is the swap seam so that later swap changes one
//! construction site — not every caller of [`active_bots`].
//!
//! The membership decision itself ([`active_bots`]) is a pure function:
//! no disk/DOM I/O, an explicit `now_ms` parameter rather than a clock
//! read, and it never reorders or mutates the roster it is given — the
//! strip's guarantee that it "never reorders the roster below it" is true
//! by construction here, not merely a promise the render layer keeps.
//!
//! Rendering: [`ActiveNowStrip`] renders NOTHING at all — not an
//! empty-state message, not a zero-height placeholder element — when zero
//! bots qualify. That is also how the loading and enumeration-failure
//! cases get their behavior for free: `bot_roster.rs`'s joined roster is
//! empty during a fetch and after a failure, so `active_bots` naturally
//! returns nothing to render.
//!
//! This file declares NO context provider (`widgets.rs`'s own module
//! doc). Every signal value is extracted BEFORE the `rsx!` block; `.read()`
//! is used in render, `.peek()` is never needed here since nothing is
//! mutated inside a closure that outlives a render.

use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::BotRosterEntry;
use crate::server::api::turns_list;
use dioxus::prelude::*;

// Under `--all-features`, `legacy-shell` swaps the root to WarpHermes,
// leaving HermesApp (and therefore this whole module) unreferenced from
// `main()` — a binary crate's unreferenced item does not suppress
// dead_code, so the `-D warnings` clippy gate fires there even though
// these are live in the default (non-legacy-shell) build. Same pattern as
// `routines.rs`'s own pure functions in this plan.

/// D-19's own value: a bot-meta write inside this many seconds of "now"
/// counts as active.
#[allow(dead_code)]
pub const ACTIVE_WINDOW_SECONDS: i64 = 90;

/// Phase 50.1 Plan 08 (D-19): the presence-membership swap seam. This
/// phase ships [`PresenceSource::BotMetaTimestamp`]; a later phase adds a
/// live-turn-events source and swaps the ONE construction site in
/// [`ActiveNowStrip`] — [`active_bots`]'s signature (a `PresenceSource`
/// parameter, not a global) means no caller needs to change.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PresenceSource {
    /// This phase's shipped source: membership is decided from each bot's
    /// stored bot-meta write timestamp.
    BotMetaTimestamp,
    /// Reserved swap target (D-19 deferred idea: "Live turn events as the
    /// presence source, replacing bot-meta timestamps"). Not read by
    /// [`active_bots`] in this phase — no call site constructs this
    /// variant yet; it exists so the seam compiles today.
    LiveTurnEvents,
}

/// Phase 50.1 Plan 08 (D-19): the pure membership decision. Preserves
/// `roster`'s input order exactly and never mutates it (`roster: &[..]`,
/// an immutable borrow, and the result is built via `.filter().cloned()`
/// — no sort, no swap-remove).
///
/// A bot qualifies when EITHER:
/// - it is the live profile (`entry.is_live`) and `live_profile_busy` is
///   `true` — even before that bot has ever written a bot-meta timestamp,
///   so a bot's very first turn shows it active before any response
///   lands; or
/// - under [`PresenceSource::BotMetaTimestamp`], its stored
///   `BotMeta::updated_at_ms` falls within [`ACTIVE_WINDOW_SECONDS`] of
///   `now_ms` — a future-dated timestamp (clock skew, a write racing this
///   read) still counts as active rather than producing a negative
///   interval, since the comparison is a plain `<=`, never an absolute
///   value.
#[allow(dead_code)]
pub fn active_bots(
    roster: &[BotRosterEntry],
    live_profile_busy: bool,
    source: PresenceSource,
    now_ms: i64,
) -> Vec<BotRosterEntry> {
    roster
        .iter()
        .filter(|entry| is_active_member(entry, live_profile_busy, source, now_ms))
        .cloned()
        .collect()
}

#[allow(dead_code)]
fn is_active_member(
    entry: &BotRosterEntry,
    live_profile_busy: bool,
    source: PresenceSource,
    now_ms: i64,
) -> bool {
    if entry.is_live && live_profile_busy {
        return true;
    }
    match source {
        PresenceSource::BotMetaTimestamp => entry
            .meta
            .as_ref()
            .map(|meta| (now_ms - meta.updated_at_ms) / 1000 <= ACTIVE_WINDOW_SECONDS)
            .unwrap_or(false),
        // Reserved: no call site constructs this variant this phase.
        PresenceSource::LiveTurnEvents => false,
    }
}

/// Phase 50.1 Plan 08: current wall-clock time in milliseconds. Mirrors
/// `bot_roster/card.rs`'s own `now_ms` helper (same `web_time::SystemTime`
/// shim, native `std::time::SystemTime` drop-in / wasm `Performance`
/// backing) — duplicated rather than shared because `card.rs`'s copy is
/// module-private.
#[allow(dead_code)]
fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.1 Plan 08: the strip itself. `roster` is the SAME joined
/// `Vec<BotRosterEntry>` `bot_roster.rs` already builds for the grid below
/// — empty during a fetch and after an enumeration failure, which is what
/// gives the loading/error states their "renders nothing" behavior for
/// free.
///
/// Sources its own live-profile-busy flag by polling `turns_list()` — the
/// same in-flight-turns server fn the Agents screen already polls for its
/// In-Flight Turns section — on the same 3s cadence `agents.rs` uses for
/// that section. Reuses a completed capability rather than adding one; per
/// D-04 the embedded runtime only ever carries the LIVE profile's turns,
/// so a non-empty result means the live profile is busy with no further
/// filtering needed.
#[component]
pub fn ActiveNowStrip(roster: Vec<BotRosterEntry>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut turns_resource =
        use_resource(move || async move { turns_list().await });
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(3_000).await;
            turns_resource.restart();
        }
    });

    // Extract BEFORE rsx! — signal-borrow discipline (clippy.toml).
    let live_profile_busy = matches!(turns_resource(), Some(Ok(ref turns)) if !turns.is_empty());
    let members = active_bots(
        &roster,
        live_profile_busy,
        PresenceSource::BotMetaTimestamp,
        now_ms(),
    );

    if members.is_empty() {
        // Zero height, no placeholder — this is the one surface in this
        // phase where "empty" means "absent" (UI-SPEC E1, D-19 binding).
        return rsx! {};
    }

    rsx! {
        div { class: "kn-bot-presence-strip",
            for entry in members.iter().cloned() {
                PresenceChip {
                    key: "{entry.row.name}",
                    is_running: entry.is_live && live_profile_busy,
                    entry,
                }
            }
        }
    }
}

/// One presence chip: a 20px `BotFace`, a 6px status dot inset at its
/// lower right, and a truncated name. `is_running` selects the dot's two
/// possible states — success token (this bot IS the live profile and a
/// turn is in flight) or accent token (it merely wrote recently) — no
/// third state.
#[component]
fn PresenceChip(entry: BotRosterEntry, is_running: bool) -> Element {
    let name = entry
        .meta
        .as_ref()
        .and_then(|m| m.title.clone())
        .unwrap_or_else(|| entry.row.name.clone());
    let avatar = entry.meta.as_ref().and_then(|m| m.avatar.clone());
    let avatar_shape = avatar.as_ref().and_then(|a| a.shape.clone());
    let avatar_color = avatar.as_ref().and_then(|a| a.color.clone());
    let avatar_image_id = avatar.as_ref().and_then(|a| a.image_id.clone());
    // D-12: every rendered instance of a working live bot's avatar
    // (roster card, drawer header, presence chip) carries
    // `data-mood="work"` — the same rule `bot_roster/card.rs` applies.
    let mood = if is_running {
        Some("work".to_string())
    } else {
        None
    };
    let dot_class = if is_running {
        "kn-bot-presence-dot kn-bot-presence-dot--running"
    } else {
        "kn-bot-presence-dot"
    };

    rsx! {
        div { class: "kn-bot-presence-chip",
            div { class: "kn-bot-presence-avatar-wrap",
                BotFace {
                    name: entry.row.name.clone(),
                    size: 20u32,
                    shape: avatar_shape,
                    color_token: avatar_color,
                    mood,
                    image_id: avatar_image_id,
                }
                span { class: dot_class }
            }
            span { class: "kn-bot-presence-name", "{name}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{BotMeta, ProfileHealth, ProfileRow};

    fn entry(name: &str, is_live: bool, meta: Option<BotMeta>) -> BotRosterEntry {
        BotRosterEntry {
            row: ProfileRow {
                name: name.to_string(),
                health: ProfileHealth::Configured,
                gaps: vec![],
                provider: None,
                model_default: None,
                key_count: 0,
            },
            meta,
            is_live,
            is_kanban_worker: false,
        }
    }

    fn meta_at(updated_at_ms: i64) -> BotMeta {
        BotMeta {
            updated_at_ms,
            ..Default::default()
        }
    }

    const NOW: i64 = 1_700_000_000_000;

    #[test]
    fn active_window_seconds_is_90() {
        assert_eq!(ACTIVE_WINDOW_SECONDS, 90);
    }

    #[test]
    fn includes_live_profile_when_busy_even_without_a_stored_timestamp() {
        let roster = vec![entry("scout", true, None)];
        let members = active_bots(&roster, true, PresenceSource::BotMetaTimestamp, NOW);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].row.name, "scout");
    }

    #[test]
    fn excludes_live_profile_when_not_busy_and_no_recent_timestamp() {
        let roster = vec![entry("scout", true, None)];
        let members = active_bots(&roster, false, PresenceSource::BotMetaTimestamp, NOW);
        assert!(members.is_empty());
    }

    #[test]
    fn includes_nonlive_bot_with_a_stored_timestamp_89_seconds_old() {
        let roster = vec![entry("scout", false, Some(meta_at(NOW - 89_000)))];
        let members = active_bots(&roster, false, PresenceSource::BotMetaTimestamp, NOW);
        assert_eq!(members.len(), 1);
    }

    #[test]
    fn excludes_nonlive_bot_with_a_stored_timestamp_91_seconds_old() {
        let roster = vec![entry("scout", false, Some(meta_at(NOW - 91_000)))];
        let members = active_bots(&roster, false, PresenceSource::BotMetaTimestamp, NOW);
        assert!(members.is_empty());
    }

    #[test]
    fn preserves_input_roster_order_and_never_mutates_it() {
        let roster = vec![
            entry("alpha", false, Some(meta_at(NOW - 10_000))),
            entry("bravo", true, None),
            entry("charlie", false, Some(meta_at(NOW - 20_000))),
        ];
        let original = roster.clone();
        let members = active_bots(&roster, true, PresenceSource::BotMetaTimestamp, NOW);
        assert_eq!(
            members.iter().map(|e| e.row.name.clone()).collect::<Vec<_>>(),
            vec!["alpha", "bravo", "charlie"]
        );
        assert_eq!(roster, original, "active_bots must never mutate its input");
    }

    #[test]
    fn empty_roster_returns_empty_result() {
        let roster: Vec<BotRosterEntry> = vec![];
        let members = active_bots(&roster, true, PresenceSource::BotMetaTimestamp, NOW);
        assert!(members.is_empty());
    }

    #[test]
    fn future_stored_timestamp_is_treated_as_active_not_a_negative_interval() {
        // 30 seconds in the future — clock skew or a write racing this read.
        let roster = vec![entry("scout", false, Some(meta_at(NOW + 30_000)))];
        let members = active_bots(&roster, false, PresenceSource::BotMetaTimestamp, NOW);
        assert_eq!(members.len(), 1, "a future-dated write must still count as active");
    }

    #[test]
    fn presence_source_takes_a_parameter_and_reserved_variant_matches_nothing() {
        // LiveTurnEvents is reserved — not read this phase; a bot that
        // WOULD qualify under BotMetaTimestamp must not qualify under the
        // reserved variant, proving `active_bots` genuinely branches on
        // its `source` parameter rather than ignoring it.
        let roster = vec![entry("scout", false, Some(meta_at(NOW - 10_000)))];
        let members = active_bots(&roster, false, PresenceSource::LiveTurnEvents, NOW);
        assert!(members.is_empty());
    }
}
