//! Phase 50.1 Plan 07 (D-02/D-14): the read-only public identity (npub) row
//! inside the bot detail drawer's Identity section.
//!
//! `BotNpubRow` is the entire client-side surface for this plan — it
//! resolves its value through [`fetch_bot_npub`] and renders exactly one
//! interactive control: a copy affordance. It NEVER renders a control that
//! would generate, import, or otherwise bring an identity into existence,
//! edit an allow list, choose a channel, or configure a transport endpoint
//! — every one of those is out of this phase by ruling (D-02).
//!
//! # State matrix (UI-SPEC E7, amended by MI-08 below)
//!
//! `empty` (not yet resolvable) and `loading` (in flight) still render the
//! SAME inline explanatory text, deliberately — there is no third spinner
//! state. As originally shipped, a resolve `error` collapsed onto that same
//! text too ("no distinct failure state"); MI-08 (below) carves out ONE
//! exception — an unreadable/malformed OWN `.env` — that now renders a
//! distinct message via the row's error idiom rather than the generic empty
//! text, because collapsing it silently substituted a DIFFERENT identity
//! (see MI-08). Only `resolved` otherwise differs: the full value, unmasked
//! (it is public by design), plus the copy control.
//!
//! # OF-8a (gap task): inherited-npub display fallback
//!
//! [`fetch_bot_npub`] now returns `BotNpubResult { npub, inherited,
//! own_env_unreadable }` (widened from plan 07's plain `Option<String>`).
//! When `inherited` is `true` — the bot's own `.env` had no identity secret,
//! so the value shown is the MAIN profile's public key, derived
//! server-side — this row adds a small `.kn-npub-inherited-badge` label
//! next to the value. `.kn-npub-inherited-badge` is this row's OWN class
//! (`bots.css`), not `.kn-chip` (kanban.css): kanban.css IS loaded
//! unconditionally (`ScreenKanban` is always mounted — `bots.css:245`'s own
//! comment already establishes this for three OTHER reused classes,
//! correcting an earlier claim in this same doc block that it was loaded
//! lazily), so the reason is the row's "own class family" convention, not a
//! load-order risk — a status/priority/assignee/age-flavored chip is
//! simply the wrong semantic fit for an inherited-provenance label. Never
//! composes onto the provider-key status classes either, per this module
//! doc's "own class family" rule above.
//!
//! # MI-08 (gap task, `50.1-REVIEW.md`): own-`.env`-unreadable suppresses the fallback
//!
//! The reviewer found that a bot's own `.env` being MALFORMED (exists, but
//! fails to parse) previously collapsed onto the same `Unavailable` outcome
//! as "no file"/"no entry" and fell through to the root-npub fallback —
//! silently showing the MAIN profile's identity in place of a bot whose OWN
//! `BUZZ_NSEC` might still be sitting, unreadable, in that broken file. An
//! operator who then whitelists the displayed (inherited) npub for this bot
//! would be whitelisting the WRONG identity. `buzz_npub_api.rs`'s
//! `NpubResolveOutcome::OwnEnvUnreadable` variant (content-free — no path,
//! no failing line) now distinguishes this case and SUPPRESSES the root
//! fallback entirely (absent-file/absent-entry/empty-value still fall back
//! normally — only "exists but unreadable/unparseable" does not).
//! `BotNpubResult.own_env_unreadable` carries this to the wire; this row
//! renders it via `.kn-modal-error` (kanban.css) — the SAME error-row class
//! `create_dialog.rs`, `edit_dialog.rs`, `advanced.rs`, `switcher.rs`,
//! `delete_confirm.rs` and `routines.rs` already use throughout this screen
//! family (e.g. `create_dialog.rs`'s own `.env`-read-failure copy is the
//! direct precedent for this row's wording) — never the plain
//! `NPUB_UNAVAILABLE_TEXT`/`.kn-drawer-empty` treatment, so the operator can
//! tell "not configured yet" apart from "configured, but broken".
//!
//! # Styling discipline (UI-SPEC Token & Class Reuse Map)
//!
//! This row NEVER wears the provider-key table's secret-status class
//! family — those classes signal inherited/manually-set/missing SECRET
//! state, and a public identity is neither secret nor settable here.
//! `.kn-npub-row`/`.kn-npub-value`/`.kn-npub-copy` are this row's own
//! classes (`bots.css`).
//!
//! Every signal value is extracted BEFORE the `rsx!` block; `.read()` is
//! used in render, `.peek()` only inside the copy closure. No context
//! provider is introduced here — matches every other `profile_shared`/
//! `bot_roster` component's module-doc discipline.
//!
//! **Callers MUST mount this keyed by bot name**
//! (`BotNpubRow { key: "{bot_name}", bot_name: bot_name.clone() }`) — same
//! `key`-forces-remount requirement `advanced.rs`'s own module doc
//! documents for its `bot_name: String` prop, so switching which bot the
//! drawer shows re-issues the fetch against the new name.

use crate::server::buzz_npub_api::{fetch_bot_npub, BotNpubResult};
use dioxus::prelude::*;

/// Copywriting Contract, verbatim — covers the `empty`, `loading` and
/// `error` states identically (module doc).
// Under `--all-features`, `legacy-shell` swaps the reachable root component
// to `WarpHermes`, which the native dead-code lint sees as making this
// const unreferenced from `main()` — even though it is live in the default
// (non-legacy-shell) `HermesApp` build. Same precedent as
// `DELETION_PROTECTED_PROFILE_NAME` in `edit_dialog.rs`.
#[allow(dead_code)]
const NPUB_UNAVAILABLE_TEXT: &str =
    "Npub not available yet — it resolves once the profile finishes setup.";

/// MI-08 (`50.1-REVIEW.md`, gap task): distinct copy for the
/// own-`.env`-unreadable case — deliberately NOT the same string as
/// [`NPUB_UNAVAILABLE_TEXT`], and rendered through `.kn-modal-error`
/// (module doc) rather than `.kn-drawer-empty`, so an operator can tell
/// "not configured yet" apart from "configured, but broken". Wording
/// mirrors `create_dialog.rs`'s own `.env`-read-failure copy ("Could not
/// read ~/.ironhermes/.env. Check permissions and retry.").
// Same legacy-shell dead-code-lint reachability quirk as
// `NPUB_UNAVAILABLE_TEXT` immediately above.
#[allow(dead_code)]
const NPUB_OWN_ENV_UNREADABLE_TEXT: &str =
    "Could not read this bot's .env — check it for a malformed line and retry.";

/// Fire-and-forget clipboard write, mirroring `kanban/card.rs`'s
/// `write_output_path_to_clipboard` / `chat.rs`'s
/// `write_workspace_path_to_clipboard` (no toast/modal feedback beyond the
/// label flip this row's copy affordance already provides).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn write_npub_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(text);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

/// Phase 50.1 Plan 07: the read-only npub row. `bot_name` is a plain
/// `String` prop (module doc: callers must mount this keyed by name).
#[component]
pub fn BotNpubRow(bot_name: String) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut copied: Signal<bool> = use_signal(|| false);

    let name_for_fetch = bot_name.clone();
    let npub_resource = use_resource(move || {
        let name = name_for_fetch.clone();
        async move { fetch_bot_npub(name).await }
    });

    // Single capture of the resource's current value — both `resolved` and
    // `own_env_unreadable` below are derived from this SAME snapshot rather
    // than calling `npub_resource()` twice.
    let npub_result = npub_resource();
    let resolved: Option<(String, bool)> = match &npub_result {
        Some(Ok(BotNpubResult {
            npub: Some(npub),
            inherited,
            ..
        })) => Some((npub.clone(), *inherited)),
        // `loading` (still `None`), `error` (an `Err`) and `resolved-but-
        // neither-key` (`Ok(BotNpubResult { npub: None, .. })`) all collapse
        // onto the same inline text below — UI-SPEC E7 deliberately has no
        // third spinner state and no distinct failure state EXCEPT the
        // MI-08 own_env_unreadable case handled separately below.
        _ => None,
    };
    // MI-08 (`50.1-REVIEW.md`, gap task): the ONE outcome that gets its own
    // distinct rendering — checked only when `resolved` above is `None`, so
    // a resolved value (own OR inherited) always takes precedence.
    let own_env_unreadable = matches!(
        &npub_result,
        Some(Ok(BotNpubResult {
            own_env_unreadable: true,
            ..
        }))
    );
    let is_copied = *copied.read();

    rsx! {
        div { class: "kn-npub-row",
            label { class: "kn-modal-label", "NPUB" }
            if let Some((npub, inherited)) = resolved.clone() {
                div { class: "kn-npub-value", "{npub}" }
                if inherited {
                    span {
                        class: "kn-npub-inherited-badge",
                        title: "This bot has no identity key of its own yet — showing the main profile's public key.",
                        "INHERITED"
                    }
                }
                button {
                    class: "kn-npub-copy kn-action-btn",
                    onclick: move |_| {
                        write_npub_to_clipboard(&npub);
                        copied.set(true);
                        spawn(async move {
                            // ~1.5s clipboard-confirmation hold — the same
                            // transient-label-flip convention the Agents
                            // screen's interrupt/cancel controls use, at a
                            // slightly longer duration appropriate to a
                            // clipboard confirmation (module doc).
                            gloo_timers::future::TimeoutFuture::new(1_500).await;
                            copied.set(false);
                        });
                    },
                    if is_copied { "COPIED" } else { "COPY" }
                }
            } else if own_env_unreadable {
                div { class: "kn-modal-error", "{NPUB_OWN_ENV_UNREADABLE_TEXT}" }
            } else {
                div { class: "kn-drawer-empty", "{NPUB_UNAVAILABLE_TEXT}" }
            }
        }
    }
}
