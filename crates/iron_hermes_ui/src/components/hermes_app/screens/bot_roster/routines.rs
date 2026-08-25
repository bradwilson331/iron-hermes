//! Phase 50.1 Plan 08 (D-22): the drawer's per-bot Routines section — lists
//! that bot's `[bot:<name>] `-namespaced scheduled jobs over the existing
//! `ironhermes-cron` scheduler (`server::schedules_api`), and deep-links
//! creation into the existing Schedules screen rather than duplicating its
//! create form.
//!
//! Binding: a routine IS a scheduled job whose `name` starts with this
//! bot's namespace prefix (`routine_prefix_for`) — verbatim plugin
//! convention. The scheduled-job record has no profile field, and adding
//! one is deferred, so the binding is a name convention: it stays visible
//! in `ironhermes cron list` and the existing Schedules screen for free,
//! at the accepted cost that a renamed profile or a hand-edited job name
//! orphans the routine (D-22).
//!
//! No new scheduler endpoint, no schema change: this file reads
//! `get_schedules` (the same whole-list read the Schedules screen itself
//! uses) and filters client-side, and its row actions call the existing
//! `run_schedule_now`/`delete_schedule` server fns verbatim.
//!
//! Creation is a deep-link, never a duplicated form: the `+ NEW ROUTINE`
//! action sets `ScheduleNamePrefillCtx` to this bot's prefix and switches
//! `active_screen` to `Screen::Schedules`; `screens/schedules.rs` reads the
//! prefill and opens its own inline editor pre-filled — `schedules.rs`
//! itself is never restyled or restructured by this file, and this file
//! never mounts a second create form of its own.
//!
//! Signal-borrow discipline (clippy.toml): every value read from a signal
//! is extracted into an owned local BEFORE the `rsx!` block. No context
//! provider is introduced here — the one new provider this plan adds
//! (`ScheduleNamePrefillCtx`) lives only in `hermes_app/mod.rs` (D-10).
//!
//! **Callers MUST mount this keyed by bot name**
//! (`BotRoutinesSection { key: "{bot_name}", bot_name: bot_name.clone() }`)
//! — same `key`-forces-remount requirement `advanced.rs`/`npub_row.rs`'s
//! own `bot_name: String` props document, so switching which bot the
//! drawer shows re-issues the fetch against the new name.

use crate::server::schedules_api::{delete_schedule, get_schedules, run_schedule_now, ScheduleRow};
use crate::state::{Screen, ScheduleNamePrefillCtx};
use dioxus::prelude::*;

// Under `--all-features`, `legacy-shell` swaps the root to WarpHermes,
// leaving HermesApp (and therefore this whole module) unreferenced from
// `main()` — a binary crate's unreferenced item does not suppress
// dead_code, so the `-D warnings` clippy gate fires there even though
// these are live in the default (non-legacy-shell) build. Same pattern as
// `DELETION_PROTECTED_PROFILE_NAME` in `edit_dialog.rs` and
// `NPUB_UNAVAILABLE_TEXT` in `npub_row.rs`.

/// Phase 50.1 Plan 08 (D-22): a bot's routine-namespace prefix — verbatim
/// plugin convention `"[bot:<name>] "`, always ending in a single trailing
/// space so a created job name reads naturally (e.g.
/// `"[bot:scout] Daily digest"`).
#[allow(dead_code)]
pub fn routine_prefix_for(name: &str) -> String {
    format!("[bot:{name}] ")
}

/// Phase 50.1 Plan 08 (D-22): true when `job_name` carries `name`'s
/// namespace prefix. Anchored at the start AND exact on the bot name — the
/// prefix's own closing `] ` delimiter makes this exact by construction: a
/// bot whose name is a prefix of another bot's name never captures the
/// other's jobs, because the longer name's next character after the
/// shorter name is never `]`. This is the one non-obvious failure this
/// convention has (T-50.1-08-01), and the test module below covers it.
#[allow(dead_code)]
pub fn is_routine_of(name: &str, job_name: &str) -> bool {
    job_name.starts_with(&routine_prefix_for(name))
}

/// Phase 50.1 Plan 08 (D-22): the display name with `name`'s namespace
/// prefix removed. Returns `job_name` unchanged when the prefix is absent
/// (never a blank row), and returns an empty string — never panics — when
/// `job_name` IS exactly the prefix with nothing after it.
#[allow(dead_code)]
pub fn strip_routine_prefix(name: &str, job_name: &str) -> String {
    job_name
        .strip_prefix(&routine_prefix_for(name))
        .unwrap_or(job_name)
        .to_string()
}

/// OF-7 gap task: the display name `RoutineRow` renders, with the
/// exact-prefix-empty-remainder case given a sensible label rather than a
/// blank row — a job saved from the deep-link with the prefilled name left
/// untouched (e.g. `"[bot:zig] "`) strips down to `""` via
/// `strip_routine_prefix`, matching the operator's OF-7 evidence (the row's
/// title rendered as `[bot:zig]` with what looked like an empty second
/// line). Same "—"-for-absent-value idiom this file already uses for
/// `next_run_display`, but with an explicit label since a bare "—" here
/// would read as "no schedule" rather than "no name given".
#[allow(dead_code)]
pub fn display_name_for_routine(bot_name: &str, job_name: &str) -> String {
    let stripped = strip_routine_prefix(bot_name, job_name);
    if stripped.is_empty() {
        "(unnamed routine)".to_string()
    } else {
        stripped
    }
}

/// Phase 50.1 Plan 08: the drawer's Routines section. Fills the
/// header-only stub plan 50.1-02 left, in the bot context only.
#[component]
pub fn BotRoutinesSection(bot_name: String) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut refresh_tick: Signal<u32> = use_signal(|| 0);

    // Read BEFORE the resource below so its value can be subscribed to in
    // the resource's sync prefix (OF-7 gap task, see comment there).
    let mut prefill_ctx = use_context::<ScheduleNamePrefillCtx>().0;
    let mut active_screen = use_context::<Signal<Screen>>();

    let name_for_fetch = bot_name.clone();
    let schedules_resource = use_resource(move || {
        // `refresh_tick` READ in the SYNC prefix (call syntax subscribes)
        // before the `async move` — the crate's proven refresh idiom
        // (`profile_switcher.rs:55-58`), reused here so a run-now/delete
        // bumps this section's own fetch without a page reload.
        let _tick = refresh_tick();
        // OF-7 gap task (root cause): `ScreenRouter` mounts every screen
        // simultaneously and only toggles an `is-active` CSS class
        // (`screen_router.rs`) — it never unmounts them. The `+ NEW
        // ROUTINE` deep-link (`open_create_deep_link` below) switches
        // `active_screen` to `Screen::Schedules` WITHOUT closing this
        // drawer (`drawer_target`/`profile_id` is untouched), so this
        // component never unmounts and this `use_resource` never re-runs
        // on its own. Before this fix, a job created via the deep-linked
        // Schedules editor was invisible in the drawer until some
        // unrelated action bumped `refresh_tick` (e.g. RUN NOW/DELETE on
        // an already-listed row) — matching the operator's exact report
        // (OF-7): the job exists on the Schedules screen, but the
        // already-mounted, already-fetched drawer never learns about it.
        // Reading `active_screen()` here re-subscribes this resource to
        // every screen switch, so navigating back to this bot's drawer
        // after creating a routine elsewhere refetches the list.
        let _screen = active_screen();
        let name = name_for_fetch.clone();
        async move {
            let rows = get_schedules().await?;
            Ok::<Vec<ScheduleRow>, ServerFnError>(
                rows.into_iter()
                    .filter(|row| is_routine_of(&name, &row.name))
                    .collect(),
            )
        }
    });

    // Extract data BEFORE rsx! — signal-borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let is_loading = schedules_resource().is_none();
    let load_error = matches!(schedules_resource(), Some(Err(_)));
    let bot_rows: Vec<ScheduleRow> = match schedules_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };
    let is_empty = bot_rows.is_empty();

    // The deep-link: set the prefill to this bot's prefix, switch to the
    // existing Schedules screen, and stop — this file never renders a
    // create form of its own.
    let bot_name_for_deep_link = bot_name.clone();
    let open_create_deep_link = move |_| {
        prefill_ctx.set(Some(routine_prefix_for(&bot_name_for_deep_link)));
        active_screen.set(Screen::Schedules);
    };

    rsx! {
        if is_loading {
            div { class: "kn-drawer-loading", "Loading routines…" }
        } else if load_error {
            div { class: "kn-modal-error",
                "Could not load routines. Check the server connection and retry."
            }
        } else if is_empty {
            div { class: "kn-drawer-empty",
                div { "No routines for this bot." }
                div {
                    "Create one to run this bot on a schedule — press + NEW ROUTINE."
                }
                button {
                    class: "kn-action-btn",
                    onclick: open_create_deep_link,
                    "+ NEW ROUTINE"
                }
            }
        } else {
            div { class: "kn-routine-list",
                for row in bot_rows.iter().cloned() {
                    RoutineRow {
                        key: "{row.id}",
                        bot_name: bot_name.clone(),
                        row,
                        on_changed: move |_| {
                            let cur = *refresh_tick.read();
                            refresh_tick.set(cur + 1);
                        },
                    }
                }
                button {
                    class: "kn-action-btn",
                    onclick: open_create_deep_link,
                    "+ NEW ROUTINE"
                }
            }
        }
    }
}

/// One routine row: display name (prefix stripped), schedule expression,
/// next-run time (absent for a disabled job — `ScheduleRow::next_run_at`
/// is already gated server-side, see `schedules_api.rs`), a RUN NOW action
/// and a DELETE action. No status badge — routines have no inherited or
/// missing state, unlike the drawer's Keys table this layout is modeled
/// on.
#[component]
fn RoutineRow(bot_name: String, row: ScheduleRow, on_changed: EventHandler<()>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut running = use_signal(|| false);
    let mut deleting = use_signal(|| false);
    let running_val = *running.read();
    let deleting_val = *deleting.read();

    let display_name = display_name_for_routine(&bot_name, &row.name);
    let next_run_display = row.next_run_at.clone().unwrap_or_else(|| "—".to_string());
    let id_for_run = row.id.clone();
    let id_for_delete = row.id.clone();

    rsx! {
        div { class: "kn-routine-row",
            div { class: "kn-routine-row-main",
                div { class: "kn-routine-row-title", "{display_name}" }
                div { class: "kn-routine-row-sub", "{row.schedule_display}" }
            }
            div { class: "kn-routine-row-next", "{next_run_display}" }
            div { class: "kn-routine-row-actions",
                button {
                    class: "kn-action-btn",
                    disabled: running_val,
                    onclick: move |_| {
                        if running_val {
                            return;
                        }
                        let id = id_for_run.clone();
                        running.set(true);
                        spawn(async move {
                            let _ = run_schedule_now(id).await;
                            running.set(false);
                            on_changed.call(());
                        });
                    },
                    if running_val { "…" } else { "RUN NOW" }
                }
                button {
                    class: "kn-action-btn",
                    disabled: deleting_val,
                    onclick: move |_| {
                        if deleting_val {
                            return;
                        }
                        let id = id_for_delete.clone();
                        deleting.set(true);
                        spawn(async move {
                            let _ = delete_schedule(id).await;
                            deleting.set(false);
                            on_changed.call(());
                        });
                    },
                    if deleting_val { "…" } else { "DELETE" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // routine_prefix_for
    // -------------------------------------------------------------------

    #[test]
    fn routine_prefix_for_scout_ends_with_single_trailing_space() {
        let prefix = routine_prefix_for("scout");
        assert_eq!(prefix, "[bot:scout] ");
        assert!(prefix.ends_with(' '));
        assert!(!prefix.ends_with("  "));
    }

    // -------------------------------------------------------------------
    // is_routine_of
    // -------------------------------------------------------------------

    #[test]
    fn is_routine_of_true_for_matching_prefix() {
        assert!(is_routine_of("scout", "[bot:scout] Daily digest"));
    }

    #[test]
    fn is_routine_of_false_for_another_bots_prefix() {
        assert!(!is_routine_of("scout", "[bot:otherbot] Daily digest"));
    }

    #[test]
    fn is_routine_of_false_for_a_bare_name() {
        assert!(!is_routine_of("scout", "scout"));
    }

    #[test]
    fn is_routine_of_false_when_prefix_appears_in_the_middle() {
        assert!(!is_routine_of("scout", "X [bot:scout] Y"));
    }

    /// T-50.1-08-01: a shorter bot name must never capture a longer-named
    /// bot's jobs. `"sco"`'s prefix is `"[bot:sco] "`; `"scout"`'s job name
    /// `"[bot:scout] ..."` has `u` (not `]`) right after `sco`, so the
    /// anchored-and-exact match fails as designed.
    #[test]
    fn is_routine_of_false_when_bot_name_is_prefix_of_another_bot_name() {
        assert!(!is_routine_of("sco", "[bot:scout] Daily digest"));
    }

    /// OF-7 regression: a job saved with the deep-link's prefilled name
    /// left completely untouched (operator's exact reproduction) IS a
    /// match — `job_name` equals the prefix exactly, with no suffix.
    #[test]
    fn is_routine_of_true_for_exact_prefix_with_empty_remainder() {
        assert!(is_routine_of("zig", "[bot:zig] "));
    }

    /// OF-7 regression: a prefix+suffix-named job (the normal case) still
    /// matches its own bot and no other.
    #[test]
    fn is_routine_of_true_for_prefix_plus_suffix_and_false_for_other_bot() {
        assert!(is_routine_of("zig", "[bot:zig] Nightly digest"));
        assert!(!is_routine_of("otherbot", "[bot:zig] Nightly digest"));
    }

    // -------------------------------------------------------------------
    // strip_routine_prefix
    // -------------------------------------------------------------------

    #[test]
    fn strip_routine_prefix_removes_the_prefix() {
        assert_eq!(
            strip_routine_prefix("scout", "[bot:scout] Daily digest"),
            "Daily digest"
        );
    }

    #[test]
    fn strip_routine_prefix_returns_whole_name_unchanged_when_prefix_absent() {
        assert_eq!(strip_routine_prefix("scout", "unrelated job"), "unrelated job");
    }

    #[test]
    fn strip_routine_prefix_on_exact_prefix_returns_empty_without_panicking() {
        assert_eq!(strip_routine_prefix("scout", "[bot:scout] "), "");
    }

    // -------------------------------------------------------------------
    // display_name_for_routine (OF-7: empty-remainder display fallback)
    // -------------------------------------------------------------------

    #[test]
    fn display_name_for_routine_falls_back_when_remainder_is_empty() {
        assert_eq!(
            display_name_for_routine("zig", "[bot:zig] "),
            "(unnamed routine)"
        );
    }

    #[test]
    fn display_name_for_routine_returns_stripped_name_when_remainder_present() {
        assert_eq!(
            display_name_for_routine("zig", "[bot:zig] Nightly digest"),
            "Nightly digest"
        );
    }
}
