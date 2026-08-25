//! Phase 50.1 Plan 01 (D-04/D-09): one roster card — head (40px avatar
//! placeholder, title, LIVE badge), a provider/model meta line, a preview
//! line, and an action row (`CHAT`/`EDIT`/overflow).
//!
//! **Resolved UI-SPEC assumption** (see `50.1-01-PLAN.md` objective):
//! `CHAT` navigates to the Chat screen only when the card's bot IS the live
//! profile. No live-profile-switch primitive exists in this crate today
//! (`profile_switcher.rs` is a kanban assignee lens, not a process-level
//! switch) — D-04 is authoritative over the UI-SPEC's premise.
//!
//! Phase 50.1 Plan 03 (D-04/D-20) / OF-5 (gap task): for every OTHER bot,
//! `CHAT` opens the floating [`super::chat_window::BotChatWindow`] targeted
//! at this card's bot — the window IS the non-live path this crate has for
//! `CHAT`, since there is still no live-profile-switch primitive. OF-5
//! replaced plan 03's bare inline `BotHandoffComposer` (deleted) with this
//! visible floating chat window; `chat_target` is owned by `bot_roster.rs`
//! (the roster screen level, mirroring the pre-existing `drawer_target`
//! idiom) and threaded down as a plain `Signal<Option<String>>` prop, never
//! a context provider.
//!
//! `EDIT` and the overflow button are rendered but inert in this task; a
//! later plan wires `EDIT` and the overflow menu.
//!
//! Phase 50.1 Plan 01 Task 2: the preview line is composed by two plain,
//! disk/DOM-I/O-free free functions ([`format_relative_timestamp`] and
//! [`compose_preview_line`]) so the UI-SPEC State Matrix's populated/
//! partial/long-text preview behaviors are directly unit-testable without a
//! renderer. The preview is read strictly from the joined `BotMeta` already
//! in hand — no new fetch of per-profile session/conversation state (that
//! write-back lands in plan 50.1-03).
//!
//! Signal-borrow discipline (clippy.toml): every value read from `entry` is
//! extracted BEFORE the `rsx!` block — `entry` itself is an owned prop, not
//! a signal, so no `GenerationalRef` is held here at all.

use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{BotMeta, BotRosterEntry};
use dioxus::prelude::*;

/// Phase 50.1 Plan 01 Task 2 (UI-SPEC E2 partial): the exact copy rendered
/// when a bot has no cached preview yet — never a blank line.
//
// Used only inside BotCard's rsx! below (via `compose_preview_line`).
// Under `--all-features`, `legacy-shell` swaps the root to WarpHermes,
// leaving HermesApp (and therefore this whole module) unreferenced from
// `main()` — a binary crate's unreferenced item does not suppress
// dead_code, so the `-D warnings` clippy gate fires there even though this
// is live in the default (non-legacy-shell) build. Same pattern as
// `KANBAN_CSS` in `screens/kanban.rs` and `ARTIFACT_SANDBOX` in
// `screens/artifact_viewer.rs`.
#[allow(dead_code)]
const NO_CONVERSATIONS_YET: &str = "No conversations yet.";

/// Phase 50.1 Plan 01 Task 2: current wall-clock time in milliseconds.
/// `web_time::SystemTime` (already a crate dependency for `Instant`, see
/// `agents.rs`'s own doc note on `std::time::Instant` panicking on
/// `wasm32-unknown-unknown`) is a drop-in `std::time::SystemTime` shim on
/// native and a `Performance`-backed one on wasm — this avoids the
/// target-arch-gated `js_sys::Date::now()` duplication `sessions.rs`'s own
/// `format_relative` needs, since this crate already depends on `web-time`.
#[allow(dead_code)] // see NO_CONVERSATIONS_YET's doc note above (legacy-shell reachability)
fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.1 Plan 01 Task 2: relative-timestamp formatter. Pure and
/// disk/DOM-I/O-free — takes `now_ms` as an explicit parameter so it is
/// directly unit-testable without a clock. A `then_ms` in the future (clock
/// skew, or a write racing a read) clamps to the "just now" form rather
/// than ever emitting a negative interval.
#[allow(dead_code)] // see NO_CONVERSATIONS_YET's doc note above (legacy-shell reachability)
pub(crate) fn format_relative_timestamp(then_ms: i64, now_ms: i64) -> String {
    let diff_ms = (now_ms - then_ms).max(0);
    let diff_secs = diff_ms / 1000;
    match diff_secs {
        s if s < 60 => "just now".to_string(),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s if s < 604_800 => format!("{}d", s / 86_400),
        _ => format!("{}w", diff_secs / 604_800),
    }
}

/// Phase 50.1 Plan 01 Task 2 (UI-SPEC E2 populated/partial): compose the
/// card's preview line. No cached preview renders [`NO_CONVERSATIONS_YET`]
/// — never a blank line. A cached preview with a timestamp joins the text
/// to [`format_relative_timestamp`]'s output with a middle-dot separator; a
/// cached preview with no timestamp (a shape the write-back path should
/// not produce, but this composer degrades gracefully rather than
/// panicking) renders the text alone.
#[allow(dead_code)] // see NO_CONVERSATIONS_YET's doc note above (legacy-shell reachability)
pub(crate) fn compose_preview_line(meta: Option<&BotMeta>, now_ms: i64) -> String {
    let Some(meta) = meta else {
        return NO_CONVERSATIONS_YET.to_string();
    };
    let Some(text) = meta.preview.as_deref() else {
        return NO_CONVERSATIONS_YET.to_string();
    };
    match meta.preview_at_ms {
        Some(at_ms) => format!("{text} · {}", format_relative_timestamp(at_ms, now_ms)),
        None => text.to_string(),
    }
}

/// Phase 50.1 Plan 01: one bot roster card. Phase 50.1 Plan 02 Task 3:
/// `EDIT`, inert since the tracer, now sets `drawer_target` to this card's
/// bot name — opening the shared detail drawer `BotRoster` mounts.
#[component]
pub fn BotCard(
    entry: BotRosterEntry,
    drawer_target: Signal<Option<String>>,
    chat_target: Signal<Option<String>>,
) -> Element {
    let name = entry.row.name.clone();
    let is_live = entry.is_live;
    // OF-4 (gap task): the roster join (`bot_roster.rs::build_roster_entries`)
    // already resolved this from the kanban board's assignee set — this
    // card renders it, it never re-derives or re-fetches it.
    let is_kanban_worker = entry.is_kanban_worker;
    let provider = entry.row.provider.clone().unwrap_or_default();
    let model = entry.row.model_default.clone().unwrap_or_default();
    let meta_line = format!("{provider} · {model}");
    let title = entry
        .meta
        .as_ref()
        .and_then(|m| m.title.clone())
        .unwrap_or_else(|| name.clone());
    let preview_text = compose_preview_line(entry.meta.as_ref(), now_ms());
    let overflow_label = format!("More actions for {name}");

    // Phase 50.1 Plan 04 (D-12): the stored avatar descriptor, if any.
    // `BotFace` itself resolves the deterministic seeded default when
    // `shape`/`color_token` are absent, so this card never needs its own
    // fallback logic.
    let avatar = entry.meta.as_ref().and_then(|m| m.avatar.clone());
    let avatar_shape = avatar.as_ref().and_then(|a| a.shape.clone());
    let avatar_color = avatar.as_ref().and_then(|a| a.color.clone());
    let avatar_image_id = avatar.as_ref().and_then(|a| a.image_id.clone());

    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    // Phase 50.1 Plan 04 (D-04/D-12): a turn is in flight on the embedded
    // runtime exactly when `streaming_id` is `Some` — and per D-04 the
    // embedded runtime only ever carries the LIVE profile's turn, so this
    // card's face only ever widens its eyes when it is BOTH the live bot
    // AND a reply is currently streaming.
    let streaming_id = use_context::<Signal<Option<u64>>>();
    let avatar_mood = if is_live && streaming_id.read().is_some() {
        Some("work".to_string())
    } else {
        None
    };

    // Phase 50.1 Plan 06 (D-17/D-18): local, per-card overflow-menu-open
    // state — never a context provider, never shared across cards.
    let mut overflow_open: Signal<bool> = use_signal(|| false);

    rsx! {
        div { class: "kn-bot-card",
            div { class: "kn-bot-card-head",
                BotFace {
                    name: name.clone(),
                    size: 40u32,
                    shape: avatar_shape,
                    color_token: avatar_color,
                    mood: avatar_mood,
                    image_id: avatar_image_id,
                }
                div { class: "kn-bot-card-title", "{title}" }
                if is_live {
                    span { class: "kn-bot-badge--live", "LIVE" }
                }
                // OF-4 (gap task): flags profiles that are current
                // assignees on the root kanban board — pre-existing kanban
                // worker profiles, not created by the bot wizard.
                if is_kanban_worker {
                    span { class: "kn-bot-badge--kanban", "KANBAN" }
                }
            }
            div { class: "kn-bot-card-meta", "{meta_line}" }
            div { class: "kn-bot-card-preview", "{preview_text}" }
            div { class: "kn-bot-card-actions",
                button {
                    class: "btn btn--sm",
                    onclick: {
                        let name_for_chat = name.clone();
                        move |_| {
                            if is_live {
                                active_screen.set(crate::state::Screen::Chat);
                            } else {
                                let mut target = chat_target;
                                target.set(Some(name_for_chat.clone()));
                            }
                        }
                    },
                    "CHAT"
                }
                button {
                    class: "btn btn--sm btn--ghost",
                    onclick: {
                        let name_for_edit = name.clone();
                        move |_| drawer_target.set(Some(name_for_edit.clone()))
                    },
                    "EDIT"
                }
                span { class: "kn-bot-card-overflow",
                    button {
                        class: "btn btn--sm btn--ghost",
                        "aria-label": "{overflow_label}",
                        onclick: move |_| {
                            let cur = *overflow_open.read();
                            overflow_open.set(!cur);
                        },
                        "⋯"
                    }
                    if *overflow_open.read() {
                        div { class: "kn-profile-menu", role: "menu",
                            // Phase 50.1 Plan 06 (D-17/D-18): both entries
                            // route through the shared detail drawer's own
                            // Danger zone rather than acting directly —
                            // matches EDIT's own wiring and keeps this a
                            // multi-step flow, not a one-click action from
                            // the card. Duplicate is warn-tinted (a
                            // data-copying action, undoable by deleting the
                            // copy); Delete is danger-tinted and, once in
                            // the drawer, sits behind a typed confirmation.
                            button {
                                class: "kn-profile-menu-item",
                                role: "menuitem",
                                style: "color: var(--warn);",
                                onclick: {
                                    let name_for_menu = name.clone();
                                    move |_| {
                                        overflow_open.set(false);
                                        drawer_target.set(Some(name_for_menu.clone()));
                                    }
                                },
                                "DUPLICATE"
                            }
                            button {
                                class: "kn-profile-menu-item",
                                role: "menuitem",
                                style: "color: var(--danger);",
                                onclick: {
                                    let name_for_menu = name.clone();
                                    move |_| {
                                        overflow_open.set(false);
                                        drawer_target.set(Some(name_for_menu.clone()));
                                    }
                                },
                                "DELETE"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // format_relative_timestamp
    // -------------------------------------------------------------------

    #[test]
    fn format_relative_timestamp_ninety_seconds_ago_contains_1m() {
        let now = 1_700_000_000_000i64;
        let then = now - 90_000;
        let result = format_relative_timestamp(then, now);
        assert!(result.contains("1m"), "expected '1m' in {result:?}");
    }

    #[test]
    fn format_relative_timestamp_two_hours_ago_contains_2h() {
        let now = 1_700_000_000_000i64;
        let then = now - 7_200_000;
        let result = format_relative_timestamp(then, now);
        assert!(result.contains("2h"), "expected '2h' in {result:?}");
    }

    #[test]
    fn format_relative_timestamp_future_timestamp_clamps_to_just_now() {
        let now = 1_700_000_000_000i64;
        let then_in_future = now + 60_000;
        let result = format_relative_timestamp(then_in_future, now);
        assert_eq!(result, "just now");
        assert!(
            !result.contains('-'),
            "must never emit a negative interval: {result:?}"
        );
    }

    // -------------------------------------------------------------------
    // compose_preview_line
    // -------------------------------------------------------------------

    #[test]
    fn compose_preview_line_none_returns_no_conversations_yet() {
        let result = compose_preview_line(None, 1_700_000_000_000);
        assert_eq!(result, NO_CONVERSATIONS_YET);
    }

    #[test]
    fn compose_preview_line_meta_with_no_cached_preview_returns_no_conversations_yet() {
        let meta = BotMeta {
            preview: None,
            ..Default::default()
        };
        let result = compose_preview_line(Some(&meta), 1_700_000_000_000);
        assert_eq!(result, NO_CONVERSATIONS_YET);
    }

    #[test]
    fn compose_preview_line_with_preview_and_timestamp_joins_with_middle_dot() {
        let now = 1_700_000_000_000i64;
        let meta = BotMeta {
            preview: Some("hey there".to_string()),
            preview_at_ms: Some(now - 90_000),
            ..Default::default()
        };
        let result = compose_preview_line(Some(&meta), now);
        assert!(result.starts_with("hey there"));
        assert!(result.contains(" · "), "expected middle-dot separator: {result:?}");
        assert!(result.contains("1m"));
    }
}
