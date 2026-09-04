//! Schedules screen — Phase 46.9 Plan 04 (D-06): live-wired to
//! `ironhermes-cron`'s `JobStore` via `server::schedules_api`, replacing the
//! prior pure mock data source with full CRUD + enable/disable + run-now.
//!
//! Read side: `use_server_future(get_schedules)` seeds a local
//! `Signal<Vec<ScheduleRow>>` once (mirrors `providers.rs`'s optimistic
//! working-copy pattern — `use_server_future(...)?` early-returns while
//! loading, so calling the resource restart method would break hook ordering for signals declared
//! after it; a successful write instead calls `get_schedules()` directly to
//! refresh).
//!
//! Write side: the NEW JOB and EDIT actions open an inline
//! `schedule-editor-form` panel. SAVE JOB calls `create_schedule` or
//! `update_schedule` and disables itself, reading "SAVING…" while in
//! flight. The `.tgl` toggle in the STATE column calls
//! `set_schedule_enabled` directly (no form). RUN NOW calls
//! `run_schedule_now`. DELETE opens an inline red confirmation panel
//! ("Delete job") before calling `delete_schedule`.
//!
//! Schedules is the ONE surface in this phase with NO restart-required
//! banner — `JobStore::reload()` re-reads `jobs.json` every tick, so writes
//! apply live (D-10 schedule exemption). Do not add one here.
//!
//! # Phase 49.4 Plan 09 (D-11): four-mode schedule editor
//!
//! The single free-text `editor_schedule` signal is replaced by a
//! mode-picker cluster (`editor_mode`/`editor_date`/`editor_time`/
//! `editor_preset`/`editor_weekday`/`editor_interval_count`/
//! `editor_interval_unit`/`editor_cron`). SAVE calls plan 04's pure,
//! wasm-compilable `build_schedule_string` DIRECTLY on the client as a
//! pre-flight check — on `Err` the message renders inline at the offending
//! field and no server call is made; on `Ok` the produced string is passed
//! to the existing `create_schedule`/`update_schedule` unchanged. Opening
//! the editor on an existing job pre-fills from `schedule_raw` (never
//! `schedule_display` — see `schedules_api.rs`'s doc comment on that field)
//! via [`detect_editor_prefill`], the reverse of `build_schedule_string`'s
//! three writable shapes.
//!
//! ## Weekday numbering (49.4-04-SUMMARY.md "Known Issue", resolved here)
//!
//! `ironhermes_cron::parse_schedule`'s underlying `cron` crate (0.13) numbers
//! weekdays 1 = Sunday .. 7 = Saturday — NOT the POSIX/vixie-cron convention
//! (0/7 = Sunday, 1 = Monday) `schedules_api.rs`'s own `humanize_schedule`
//! uses for display. `build_schedule_string` passes `weekday` straight
//! through with no remapping, so plan 04 flagged this as a real trap: a
//! Weekly job authored with the POSIX-intended weekday `1` (meaning
//! "Monday") would actually fire on Sunday. This plan resolves it by
//! boundary translation entirely on the client (49.4-04-SUMMARY.md's
//! recommendation (a)) — `WEEKDAY_OPTIONS` below presents day labels mapped
//! DIRECTLY onto the cron-crate's actual 1=Sun..7=Sat numbering, so a job
//! selected for "Monday" is built and stored as weekday `2` and genuinely
//! fires on Monday. Because `humanize_schedule`'s own `weekday_short_name`
//! still assumes POSIX numbering, this file renders weekly schedule text via
//! its own [`weekly_cron_label`] (cron-crate numbering) instead, falling
//! through to `humanize_schedule` for every other shape — see
//! [`display_schedule_text`]. `schedules_api.rs` itself is untouched; both
//! fixes are entirely local to this file.

use std::collections::BTreeMap;

use dioxus::prelude::*;

use crate::server::blueprints_api::{
    create_schedule_from_blueprint, create_schedule_from_saved_blueprint, list_blueprints,
    BlueprintKind, BlueprintSlotView, BlueprintView,
};
use crate::server::api::list_skills;
use crate::server::display_tz_api::get_display_timezones;
use crate::server::provider_config_api::get_provider_config;
use crate::server::schedules_api::{
    build_schedule_string, create_schedule, delete_schedule, get_schedules, humanize_schedule,
    run_schedule_now, set_schedule_enabled, update_schedule, IntervalUnit, RecurringPreset,
    ScheduleBuilderInput, ScheduleMode, ScheduleRow, SchedulesView, ScheduleWriteInput,
};
use crate::server::skills_import_api::save_job_as_blueprint;
use crate::server::tools_config_api::{get_tools_page_state, ConfigScope};

/// Weekday picker options: `(cron-crate dow value, label)`. Cron-crate
/// numbering (1=Sun..7=Sat) — see module doc "Weekday numbering".
const WEEKDAY_OPTIONS: [(u32, &str); 7] = [
    (1, "SUN"),
    (2, "MON"),
    (3, "TUE"),
    (4, "WED"),
    (5, "THU"),
    (6, "FRI"),
    (7, "SAT"),
];

/// Default weekday for a fresh Weekly-mode editor: Monday, cron-crate
/// numbering.
const DEFAULT_WEEKDAY: u32 = 2;

/// Map a server error into the two-line copy the UI-SPEC specifies. Invalid
/// schedules render the exact Copywriting Contract lines; everything else
/// falls back to a generic line plus the raw server message. With the
/// mode-picker's pre-flight `build_schedule_string` check in place, the
/// server should no longer return "Invalid schedule" in practice — this
/// stays as a defensive fallback for any other server-side rejection (e.g.
/// job name/prompt validation).
#[allow(dead_code)] // called from the SAVE JOB onclick closure in
                    // ScreenSchedules; dead_code fires under `--all-features
                    // --all-targets` (test target) — same known false
                    // positive as skills.rs's tab_predicate/search_matches
                    // helpers and providers.rs's map_save_error in this crate.
fn map_save_error(e: &ServerFnError) -> (String, String) {
    let msg = e.to_string();
    if msg.contains("Invalid schedule") {
        (
            "Invalid schedule.".to_string(),
            "Use a cron expression (0 9 * * *), interval (every 2h), or timestamp (2026-08-01T09:00Z).".to_string(),
        )
    } else {
        ("Save failed.".to_string(), msg)
    }
}

/// Blueprints Set-up form's own two-line whole-form message (UI-SPEC
/// Copywriting Contract, distinct fixed first line from `map_save_error`'s
/// "Save failed." — this is a server rejection of a blueprint fill/create
/// call, not the manual dialog's job save).
#[allow(dead_code)] // called from the ⏱ Schedule it onclick closure — same
                    // known false positive as `map_save_error` under
                    // `--all-features --all-targets`.
fn map_blueprint_error(e: &ServerFnError) -> (String, String) {
    ("Could not schedule this blueprint.".to_string(), e.to_string())
}

/// Save-as-blueprint dialog's own whole-form error message (Phase 49.6 Plan
/// 04, D-15). Distinct fixed first line from `map_save_error`'s "Save
/// failed." and `map_blueprint_error`'s "Could not schedule this
/// blueprint." — each names a different operation, per this file's own
/// established one-fixed-line-per-operation convention.
fn map_blueprint_save_error(e: &ServerFnError) -> (String, String) {
    ("Could not save this blueprint.".to_string(), e.to_string())
}

/// Client-safe mirror of `ironhermes_core::skills::sanitize_blueprint_name`
/// (Phase 49.6 Plan 04). `ironhermes-core` is a non-wasm-only dependency of
/// this crate (`Cargo.toml`'s `[target.'cfg(not(target_arch =
/// "wasm32"))'.dependencies]` table) — the wasm client cannot call the real
/// function directly, so this MUST match its algorithm exactly rather than
/// approximate it: lowercase, map every character that is not
/// ASCII-alphanumeric/`-`/`_` to `-`, trim leading/trailing `-`/`_`, and
/// fall back to `shared-blueprint` when the result is empty. The server
/// re-sanitizes the raw (unmodified) name independently on submit — this
/// preview exists only so the operator sees the same result before saving.
fn sanitize_preview(raw: &str) -> String {
    let mapped: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = mapped.trim_matches(|c| c == '-' || c == '_');
    if trimmed.is_empty() {
        "shared-blueprint".to_string()
    } else {
        trimmed.to_string()
    }
}

/// First line of the save-as-blueprint dialog's description textarea — the
/// part that becomes the composed `SKILL.md`'s frontmatter `description`
/// server-side (mirrors `ironhermes_core::skills::
/// blueprint_description_from_body`'s own first-line derivation; read-only
/// preview purposes here, the server derives its own copy independently).
fn description_first_line(description: &str) -> &str {
    description.lines().next().unwrap_or("")
}

/// `{n} / 200` counter text for the description's first line (Copywriting
/// Contract), pinning at the literal cut-off message once the count
/// exceeds 200 rather than continuing to count past it.
fn description_counter_text(first_line_char_count: usize) -> String {
    if first_line_char_count > 200 {
        "200 / 200 — first line will be cut off here".to_string()
    } else {
        format!("{first_line_char_count} / 200")
    }
}

/// Client-side pre-flight validation for the Blueprints tab's Set-up form,
/// mirroring `ironhermes_cron::fill_blueprint`'s own required/optional
/// semantics so a malformed submission never reaches the server. Returns a
/// per-slot-name error message for every slot that fails; an empty map
/// means the form is ready to submit.
#[allow(dead_code)] // called from the ⏱ Schedule it onclick closure — same
                    // known false positive as `map_save_error`.
fn validate_blueprint_slots(
    slots: &[BlueprintSlotView],
    values: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut errors = BTreeMap::new();
    for slot in slots {
        let val = values.get(&slot.name).cloned().unwrap_or_default();
        let trimmed = val.trim();
        if trimmed.is_empty() {
            if !slot.optional {
                errors.insert(slot.name.clone(), format!("{} required.", slot.label.to_uppercase()));
            }
            continue;
        }
        if slot.slot_type == "time" && parse_hh_mm(trimmed).is_none() {
            errors.insert(slot.name.clone(), "Enter a time.".to_string());
        }
    }
    errors
}

/// Current instant as RFC3339 — `build_schedule_string`'s one-time mode
/// needs it for the "Pick a time in the future." check, and this file calls
/// that function directly on the CLIENT (not just server-side) as a
/// pre-flight gate before ever reaching `create_schedule`/`update_schedule`.
/// `chrono::Utc::now()`/`Local::now()` call `SystemTime::now()` internally,
/// which panics on `wasm32-unknown-unknown` without the `wasmbind` feature
/// (not enabled in this crate — plan 04's key-decision was to avoid needing
/// it by always taking the instant as a caller-supplied parameter). Mirrors
/// `kanban/card.rs`'s `current_unix_time` cfg split.
fn now_rfc3339() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let ms = js_sys::Date::now();
        let secs = (ms / 1000.0).floor() as i64;
        let millis_part = (ms - (secs as f64) * 1000.0).max(0.0) as u32;
        let nanos = millis_part.saturating_mul(1_000_000);
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Utc::now().to_rfc3339()
    }
}

/// Phase 49.5 Plan 06 (D-15): parse the `CONTEXT_FROM JOB IDS` textarea —
/// one job id per line, trimmed, blanks dropped, entered order preserved.
/// An empty (or all-blank) textarea yields `None` rather than `Some(vec![])`
/// — an untouched field is absent, not an explicit clear.
fn parse_context_from(raw: &str) -> Option<Vec<String>> {
    let ids: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    (!ids.is_empty()).then_some(ids)
}

/// Phase 49.5 Plan 06 (D-15/T-49.5-06-04): whether `row` has any advanced
/// field set — drives auto-expanding the ADVANCED FIELDS disclosure on
/// edit-open so a stored value is never hidden behind a collapsed control
/// the operator might not open.
fn schedule_row_has_advanced_fields(row: &ScheduleRow) -> bool {
    row.provider.is_some()
        || row.model.is_some()
        || row.base_url.is_some()
        || row.script.is_some()
        || row.workdir.is_some()
        || row.no_agent
        || row.context_from.as_ref().is_some_and(|v| !v.is_empty())
        || row.enabled_toolsets.as_ref().is_some_and(|v| !v.is_empty())
        || row.continuity
}

/// Parse an `HH:MM` string into `(hour, minute)`. `None` for anything
/// malformed or out of range.
fn parse_hh_mm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

fn preset_value_str(p: RecurringPreset) -> &'static str {
    match p {
        RecurringPreset::Hourly => "hourly",
        RecurringPreset::Daily => "daily",
        RecurringPreset::Weekly => "weekly",
    }
}

fn preset_from_value_str(s: &str) -> RecurringPreset {
    match s {
        "hourly" => RecurringPreset::Hourly,
        "weekly" => RecurringPreset::Weekly,
        _ => RecurringPreset::Daily,
    }
}

fn interval_unit_value_str(u: IntervalUnit) -> &'static str {
    match u {
        IntervalUnit::Minutes => "min",
        IntervalUnit::Hours => "hour",
    }
}

fn interval_unit_from_value_str(s: &str) -> IntervalUnit {
    if s == "hour" {
        IntervalUnit::Hours
    } else {
        IntervalUnit::Minutes
    }
}

/// Same 5-field/digit-class cron shape check `schedules_api.rs`'s private
/// `looks_like_cron_shape` uses, duplicated here (that fn is not `pub`) —
/// used only to recognize the mode-picker's OWN three cron shapes for
/// pre-fill mode detection, never as a validator (the real parser remains
/// the sole authority once a string reaches `create_schedule`/
/// `update_schedule`).
fn cron_fields(s: &str) -> Option<Vec<&str>> {
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }
    if !fields.iter().all(|f| {
        !f.is_empty()
            && f.chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '*' | '-' | ',' | '/' | '?'))
    }) {
        return None;
    }
    Some(fields)
}

/// Weekly-schedule display label using the CRON-CRATE weekday numbering
/// (1=Sun..7=Sat) — see module doc "Weekday numbering". Returns `None` for
/// anything that isn't exactly the weekly cron shape the mode-picker itself
/// produces (5 fields, dom/month `*`, dow present and non-`*`), so the
/// caller ([`display_schedule_text`]) can fall through to
/// `humanize_schedule` for everything else.
fn weekly_cron_label(raw: &str) -> Option<String> {
    let fields = cron_fields(raw.trim())?;
    let (minute, hour, dom, month, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
    if dom != "*" || month != "*" || hour == "*" || dow == "*" {
        return None;
    }
    let minute_n: u32 = minute.parse().ok()?;
    let hour_n: u32 = hour.parse().ok()?;
    let dow_n: u32 = dow.parse().ok()?;
    let (_, name) = WEEKDAY_OPTIONS.iter().find(|(v, _)| *v == dow_n)?;
    let title_case = format!(
        "{}{}",
        name.chars().next().unwrap_or_default(),
        name.chars().skip(1).collect::<String>().to_lowercase()
    );
    Some(format!("weekly {title_case} {hour_n:02}:{minute_n:02}"))
}

/// Row-display schedule text: corrects the weekly-cron weekday label (see
/// module doc) and otherwise delegates to `humanize_schedule` unchanged.
fn display_schedule_text(raw: &str) -> String {
    weekly_cron_label(raw).unwrap_or_else(|| humanize_schedule(raw))
}

/// Pre-fill data for opening the editor on an existing job. See module doc
/// for the weekday-numbering convention (cron-crate 1=Sun..7=Sat).
#[derive(Debug, Clone, PartialEq)]
struct EditorPrefill {
    mode: ScheduleMode,
    date: String,
    time: String,
    preset: RecurringPreset,
    weekday: u32,
    interval_count: String,
    interval_unit: IntervalUnit,
    cron: String,
}

impl EditorPrefill {
    fn blank() -> Self {
        Self {
            mode: ScheduleMode::OneTime,
            date: String::new(),
            time: String::new(),
            preset: RecurringPreset::Daily,
            weekday: DEFAULT_WEEKDAY,
            interval_count: "1".to_string(),
            interval_unit: IntervalUnit::Minutes,
            cron: String::new(),
        }
    }
}

/// Detect which of the four editor modes `raw` (a job's `schedule_raw`)
/// corresponds to, and extract that mode's fields — the reverse of
/// `build_schedule_string`'s three writable shapes. Anything that isn't
/// exactly one of the three cron shapes / the interval shape / an RFC3339
/// instant the mode-picker itself can produce falls back to Advanced mode
/// with the raw string shown verbatim — never guesses a mode it can't
/// prove (mirrors `humanize_schedule`'s "wrong is worse than raw" rule).
fn detect_editor_prefill(raw: &str, tz_name: Option<&str>) -> EditorPrefill {
    let trimmed = raw.trim();

    if let Some(rest) = trimmed.strip_prefix("every ") {
        if let Some(minutes_str) = rest.strip_suffix('m') {
            if let Ok(minutes) = minutes_str.parse::<i64>() {
                if minutes > 0 {
                    let (count, unit) = if minutes % 60 == 0 {
                        (minutes / 60, IntervalUnit::Hours)
                    } else {
                        (minutes, IntervalUnit::Minutes)
                    };
                    return EditorPrefill {
                        mode: ScheduleMode::Interval,
                        interval_count: count.to_string(),
                        interval_unit: unit,
                        ..EditorPrefill::blank()
                    };
                }
            }
        }
    }

    if let Some(fields) = cron_fields(trimmed) {
        let (minute, hour, dom, month, dow) =
            (fields[0], fields[1], fields[2], fields[3], fields[4]);
        if dom == "*" && month == "*" {
            if let Ok(minute_n) = minute.parse::<u32>() {
                if hour == "*" && dow == "*" {
                    return EditorPrefill {
                        mode: ScheduleMode::Recurring,
                        preset: RecurringPreset::Hourly,
                        time: format!("00:{minute_n:02}"),
                        ..EditorPrefill::blank()
                    };
                }
                if let Ok(hour_n) = hour.parse::<u32>() {
                    if dow == "*" {
                        return EditorPrefill {
                            mode: ScheduleMode::Recurring,
                            preset: RecurringPreset::Daily,
                            time: format!("{hour_n:02}:{minute_n:02}"),
                            ..EditorPrefill::blank()
                        };
                    }
                    if let Ok(dow_n) = dow.parse::<u32>() {
                        if (1..=7).contains(&dow_n) {
                            return EditorPrefill {
                                mode: ScheduleMode::Recurring,
                                preset: RecurringPreset::Weekly,
                                time: format!("{hour_n:02}:{minute_n:02}"),
                                weekday: dow_n,
                                ..EditorPrefill::blank()
                            };
                        }
                    }
                }
            }
        }
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        let utc = dt.with_timezone(&chrono::Utc);
        let (date, time) = match tz_name.and_then(|n| n.parse::<chrono_tz::Tz>().ok()) {
            Some(tz) => {
                let local = utc.with_timezone(&tz).naive_local();
                (
                    local.format("%Y-%m-%d").to_string(),
                    local.format("%H:%M").to_string(),
                )
            }
            None => (
                utc.format("%Y-%m-%d").to_string(),
                utc.format("%H:%M").to_string(),
            ),
        };
        return EditorPrefill {
            mode: ScheduleMode::OneTime,
            date,
            time,
            ..EditorPrefill::blank()
        };
    }

    EditorPrefill {
        mode: ScheduleMode::Advanced,
        cron: trimmed.to_string(),
        ..EditorPrefill::blank()
    }
}

#[component]
pub fn ScreenSchedules(is_active: bool) -> Element {
    // Phase 49.6 Plan 02 (D-04): the store-selector's current scope — `None`
    // is the aggregate default (every profile + root). `SchedulesScopeBar`
    // (below) writes into this signal; every read uses it, and every write
    // collapses server-side to root when it is `None` (D-04).
    let schedule_scope_sig: Signal<Option<String>> = use_signal(|| None);
    let schedules_resource = use_server_future(move || {
        let scope = schedule_scope_sig();
        async move { get_schedules(scope).await }
    })?;

    // Extract data BEFORE rsx! — signal borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let initial_loading = schedules_resource().is_none();

    // Optimistic local working copy — seeded once from the resource (see
    // module doc / providers.rs precedent for why this is not driven by
    // calling the resource restart method). A SCOPE CHANGE never relies on
    // this seed-once path — `SchedulesScopeBar`'s `on_scope_changed` below
    // does its own direct refetch, the same "never the resource restart
    // method" discipline every write already follows in this file.
    let mut schedule_list_sig: Signal<Vec<ScheduleRow>> = use_signal(Vec::new);
    // Phase 49.6 Plan 02 (D-04, UI-SPEC E10): the aggregate scope's
    // degrade-gracefully companion to `schedule_list_sig` — which profile
    // stores this most recent read could not open. Updated by every
    // refetch (initial load, retry, scope change, and every write's own
    // "direct refetch" below) alongside `schedule_list_sig`, never left
    // stale after one of those updates and not the other.
    let mut unreadable_profiles_sig: Signal<Vec<String>> = use_signal(Vec::new);
    let mut seeded = use_signal(|| false);
    // Task 2 (D-12/E7 error): a separate error signal, distinct from the
    // one-shot `schedules_resource`'s own Result — the RETRY button below
    // re-fetches via a direct `get_schedules()` call (never
    // `schedules_resource`'s restart method, matching this file's existing
    // save/delete/toggle "direct refetch" discipline) and updates this
    // signal instead.
    let mut list_error: Signal<Option<String>> = use_signal(|| None);
    let mut retrying = use_signal(|| false);
    {
        let loaded = match schedules_resource() {
            Some(Ok(ref view)) => Some(view.clone()),
            _ => None,
        };
        let initial_err = match schedules_resource() {
            Some(Err(ref e)) => Some(e.to_string()),
            _ => None,
        };
        use_effect(move || {
            if let Some(ref view) = loaded {
                if !*seeded.read() {
                    schedule_list_sig.set(view.rows.clone());
                    unreadable_profiles_sig.set(view.unreadable_profiles.clone());
                    seeded.set(true);
                    list_error.set(None);
                }
            }
            if let Some(ref e) = initial_err {
                if !*seeded.read() {
                    list_error.set(Some(e.clone()));
                }
            }
        });
    }

    let schedule_list = schedule_list_sig.read().clone();
    let unreadable_profiles = unreadable_profiles_sig.read().clone();

    // Display timezone (D-13's resolution rule) — one-shot resource, no
    // refresh needed. Used both when building a one-time schedule string
    // (matching the rule `create_schedule`/`update_schedule` themselves use
    // server-side) and when pre-filling an existing one-time job's
    // date/time fields for editing.
    let tz_resource = use_resource(move || async move { get_display_timezones().await });
    let tz_name_val: Option<String> = match tz_resource() {
        Some(Ok(dt)) => dt.primary,
        _ => None,
    };

    // Phase 49.5 Plan 06 (D-15): installed-skills catalog for the SKILLS
    // checkbox list on the manual New/Edit Job panel. One-shot resource,
    // no new listing endpoint — reuses the same `list_skills()` the Skills
    // screen already calls.
    let skills_resource = use_resource(move || async move { list_skills().await });
    let installed_skills: Vec<crate::server::api::SkillInfo> = match skills_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };

    // Phase 49.5 Plan 06 (D-15): PROVIDER select options (no filtering —
    // same precedent as `models.rs`'s provider dropdown) and ENABLED_TOOLSETS
    // checkbox-list options. `ConfigScope::Root` — cron jobs are root-scoped
    // today (D-17); scoping to an operator's individual identity is
    // deferred to Phase 49.6.
    let provider_resource = use_resource(move || async move { get_provider_config().await });
    let provider_names: Vec<String> = match provider_resource() {
        Some(Ok(snap)) => snap.providers.iter().map(|p| p.name.clone()).collect(),
        _ => Vec::new(),
    };
    let toolsets_resource = use_resource(move || async move {
        get_tools_page_state(ConfigScope::Root).await
    });
    let available_toolsets: Vec<String> = match toolsets_resource() {
        Some(Ok(state)) => state.toolsets.iter().map(|t| t.name.clone()).collect(),
        _ => Vec::new(),
    };

    // ── Editor form state (mode-picker, D-11) ───────────────────────────
    let mut editor_open = use_signal(|| false);
    let mut editor_is_new = use_signal(|| false);
    let mut editor_id = use_signal(String::new);
    let mut editor_name = use_signal(String::new);
    let mut editor_prompt = use_signal(String::new);
    let mut editor_deliver = use_signal(String::new);
    let mut editor_mode: Signal<ScheduleMode> = use_signal(|| ScheduleMode::OneTime);
    let mut editor_date = use_signal(String::new);
    let mut editor_time = use_signal(String::new);
    let mut editor_preset: Signal<RecurringPreset> = use_signal(|| RecurringPreset::Daily);
    let mut editor_weekday: Signal<u32> = use_signal(|| DEFAULT_WEEKDAY);
    let mut editor_interval_count = use_signal(|| "1".to_string());
    let mut editor_interval_unit: Signal<IntervalUnit> = use_signal(|| IntervalUnit::Minutes);
    let mut editor_cron = use_signal(String::new);
    // Phase 49.5 Plan 06 (D-15): SKILLS checkbox-list selection, seeded from
    // the row's `skills` on edit-open and empty on `+ NEW JOB` — see the
    // "+ NEW JOB" and `on_edit` handlers below.
    let mut editor_skills: Signal<Vec<String>> = use_signal(Vec::new);
    // Phase 49.5 Plan 06 (D-15): ADVANCED FIELDS disclosure state + its nine
    // controls. `editor_provider`/`editor_model`/`editor_base_url`/
    // `editor_script`/`editor_workdir` are empty-string-means-unset, same
    // convention as `editor_deliver` above; `editor_context_from` holds the
    // raw textarea text (split into job ids at submit time, see the SAVE
    // handler). `editor_advanced_open` is seeded closed for a new job and
    // opened automatically when an existing job has any advanced field set
    // (T-49.5-06-04) — see the "+ NEW JOB" and `on_edit` handlers below.
    let mut editor_advanced_open = use_signal(|| false);
    let mut editor_provider = use_signal(String::new);
    let mut editor_model = use_signal(String::new);
    let mut editor_base_url = use_signal(String::new);
    let mut editor_no_agent = use_signal(|| false);
    let mut editor_script = use_signal(String::new);
    let mut editor_workdir = use_signal(String::new);
    let mut editor_continuity = use_signal(|| false);
    let mut editor_context_from = use_signal(String::new);
    let mut editor_enabled_toolsets: Signal<Vec<String>> = use_signal(Vec::new);
    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut field_error: Signal<Option<String>> = use_signal(|| None);

    // ── Delete-confirm state ────────────────────────────────────────────
    let mut delete_confirm: Signal<Option<ScheduleRow>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    // ── Save-as-blueprint dialog state (Phase 49.6 Plan 04, D-15) ───────
    // `blueprint_save_target` is the ONE signal both entry points write —
    // `Some(row)` opens `SaveBlueprintDialog` with that row as the source;
    // `None` keeps it closed. `editor_current_row` mirrors the row the
    // inline editor panel currently holds (kept alongside the individual
    // `editor_*` field signals, which have no full-row representation of
    // their own) so the Edit Job panel's own entry point has a source row
    // to open the SAME dialog with — `None` for a new, unsaved job (which
    // has nothing to export yet, hence that entry not rendering at all).
    let mut blueprint_save_target: Signal<Option<ScheduleRow>> = use_signal(|| None);
    let mut editor_current_row: Signal<Option<ScheduleRow>> = use_signal(|| None);

    // ── Blueprints tab state (Phase 49.5 Plan 01, D-10/D-12) ────────────
    // `active_tab` is a local Signal, never persisted, never a Screen
    // variant — the "jobs" default is what keeps the three existing
    // deep-links (settings.rs, gateway/schedules_card.rs, bot_roster/
    // routines.rs) landing on Jobs with zero extra wiring (D-13).
    let mut active_tab = use_signal(|| "jobs");
    // The currently-expanded blueprint card's key, or None — setting a new
    // key implicitly collapses whichever card was open (only one at a time).
    let mut expanded_key: Signal<Option<String>> = use_signal(|| None);
    // Live-editable slot values for the currently-expanded card, keyed by
    // slot name. Re-seeded from the blueprint's slot defaults every time a
    // new card is expanded.
    let mut blueprint_values: Signal<BTreeMap<String, String>> = use_signal(BTreeMap::new);
    let mut blueprint_scheduling = use_signal(|| false);
    let mut blueprint_error: Signal<Option<(String, String)>> = use_signal(|| None);
    // Per-slot-name inline validation messages (UI-SPEC "required"/
    // "malformed time" errors) — distinct from `blueprint_error`, the
    // whole-form server-rejection message.
    let mut blueprint_field_errors: Signal<BTreeMap<String, String>> = use_signal(BTreeMap::new);

    // The CURATED half of this list is compiled into the binary (D-05), but
    // since Phase 49.6 Plan 04 the response also carries SAVED blueprints,
    // read from the live `SkillRegistry` — which `save_job_as_blueprint`
    // mutates at runtime via `reload_skill_catalog`. So this is no longer a
    // fetch-once resource: it must re-run after a save, or a just-saved
    // blueprint stays invisible until a full page reload (UAT 49.6 test 6).
    // `blueprint_refresh_tick` is read in the SYNCHRONOUS prefix — that read
    // is what subscribes the resource; a read inside the async block would
    // not re-trigger it.
    let mut blueprint_refresh_tick = use_signal(|| 0u32);
    let blueprints_resource = use_resource(move || {
        let _tick = blueprint_refresh_tick();
        async move { list_blueprints().await }
    });
    let blueprint_list: Vec<BlueprintView> = match blueprints_resource() {
        Some(Ok(views)) => views,
        _ => Vec::new(),
    };

    // Phase 50.1 Plan 08 (D-22): read the bot drawer's Routines deep-link
    // prefill. When set, open THIS SAME inline editor panel above with the
    // job-name field pre-filled to the bot's namespace prefix — no second
    // create form, no restyle of this screen. Clear the prefill in the
    // same effect so a later visit does not reopen it. Reads and mutates
    // the SAME context signal, so the `.set(None)` below re-triggers this
    // effect once more with an already-`None` value, which is a no-op.
    let mut schedule_name_prefill =
        use_context::<crate::state::ScheduleNamePrefillCtx>().0;
    use_effect(move || {
        let prefill = schedule_name_prefill.read().clone();
        if let Some(prefix) = prefill {
            editor_is_new.set(true);
            editor_current_row.set(None);
            editor_id.set(String::new());
            editor_name.set(prefix);
            editor_prompt.set(String::new());
            editor_deliver.set(String::new());
            editor_mode.set(ScheduleMode::OneTime);
            editor_date.set(String::new());
            editor_time.set(String::new());
            editor_preset.set(RecurringPreset::Daily);
            editor_weekday.set(DEFAULT_WEEKDAY);
            editor_interval_count.set("1".to_string());
            editor_interval_unit.set(IntervalUnit::Minutes);
            editor_cron.set(String::new());
            editor_skills.set(Vec::new());
            editor_advanced_open.set(false);
            editor_provider.set(String::new());
            editor_model.set(String::new());
            editor_base_url.set(String::new());
            editor_no_agent.set(false);
            editor_script.set(String::new());
            editor_workdir.set(String::new());
            editor_continuity.set(false);
            editor_context_from.set(String::new());
            editor_enabled_toolsets.set(Vec::new());
            save_error.set(None);
            field_error.set(None);
            editor_open.set(true);
            schedule_name_prefill.set(None);
        }
    });

    // Read all signal values into owned locals BEFORE rsx! (Pattern B).
    let editor_open_val = *editor_open.read();
    let editor_is_new_val = *editor_is_new.read();
    // UI-SPEC E4 (D-04): the write-collapse note shows only while creating
    // a NEW job under the aggregate scope — editing an existing job is
    // unaffected (its write target is the job's own store, not the
    // selector's current scope).
    let show_write_collapse_note = editor_is_new_val && schedule_scope_sig().is_none();
    let editor_current_row_val = editor_current_row.read().clone();
    let blueprint_save_target_val = blueprint_save_target.read().clone();
    let editor_name_val = editor_name.read().clone();
    let editor_prompt_val = editor_prompt.read().clone();
    let editor_deliver_val = editor_deliver.read().clone();
    let editor_mode_val = *editor_mode.read();
    let editor_date_val = editor_date.read().clone();
    let editor_time_val = editor_time.read().clone();
    let editor_preset_val = *editor_preset.read();
    let editor_weekday_val = *editor_weekday.read();
    let editor_interval_count_val = editor_interval_count.read().clone();
    let editor_interval_unit_val = *editor_interval_unit.read();
    let editor_cron_val = editor_cron.read().clone();
    let editor_skills_val = editor_skills.read().clone();
    let editor_advanced_open_val = *editor_advanced_open.read();
    let editor_provider_val = editor_provider.read().clone();
    let editor_model_val = editor_model.read().clone();
    let editor_base_url_val = editor_base_url.read().clone();
    let editor_no_agent_val = *editor_no_agent.read();
    let editor_script_val = editor_script.read().clone();
    let editor_workdir_val = editor_workdir.read().clone();
    let editor_continuity_val = *editor_continuity.read();
    let editor_context_from_val = editor_context_from.read().clone();
    let editor_enabled_toolsets_val = editor_enabled_toolsets.read().clone();
    let saving_val = *saving.read();
    let save_error_val = save_error.read().clone();
    let field_error_val = field_error.read().clone();
    let delete_confirm_val = delete_confirm.read().clone();
    let deleting_val = *deleting.read();
    let retrying_val = *retrying.read();
    let is_loading = initial_loading || retrying_val;
    let list_error_val = list_error.read().clone();
    // UI-SPEC E3 "populated"/"error" (D-03 hard requirement): the non-root
    // banner shows whenever the current view contains at least one
    // non-root row — an empty (indeterminate) profile string counts too,
    // so a row whose ownership could not be determined fails VISIBLE
    // rather than hidden (a false positive is safe; a false negative is
    // the exact D-03 failure class this banner exists to prevent). Gated
    // on `!is_loading` so it can never flash during an in-flight fetch,
    // initial or retried (E3 loading backstop).
    let has_non_root_row = !is_loading && schedule_list.iter().any(|r| r.profile != "default");
    let active_tab_val = *active_tab.read();
    let expanded_key_val = expanded_key.read().clone();
    let blueprint_values_val = blueprint_values.read().clone();
    let blueprint_scheduling_val = *blueprint_scheduling.read();
    let blueprint_error_val = blueprint_error.read().clone();
    let blueprint_field_errors_val = blueprint_field_errors.read().clone();

    let name_ok = !editor_name_val.trim().is_empty();
    let prompt_ok = !editor_prompt_val.trim().is_empty();
    let mode_ok = match editor_mode_val {
        ScheduleMode::OneTime => {
            !editor_date_val.trim().is_empty() && parse_hh_mm(&editor_time_val).is_some()
        }
        ScheduleMode::Recurring => parse_hh_mm(&editor_time_val).is_some(),
        ScheduleMode::Interval => editor_interval_count_val
            .trim()
            .parse::<i64>()
            .map(|n| n > 0)
            .unwrap_or(false),
        ScheduleMode::Advanced => !editor_cron_val.trim().is_empty(),
    };
    let can_save = name_ok && prompt_ok && mode_ok && !saving_val;

    // Separate owned clones per closure — `Option<String>` is not `Copy`,
    // and the SAVE button's onclick and the per-row `on_edit` closure (the
    // latter instantiated once per row inside the `for` loop below) each
    // need their own capture.
    let tz_name_for_save = tz_name_val.clone();
    let tz_name_for_edit = tz_name_val.clone();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-schedules",
            "data-screen-label": "09 Cron Schedules",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 09" }
                    h1 { class: "screen-title", "Cron Schedules" }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⏵ HISTORY" }
                    button {
                        class: "btn btn--sm",
                        onclick: move |_| {
                            editor_is_new.set(true);
                            editor_current_row.set(None);
                            editor_id.set(String::new());
                            editor_name.set(String::new());
                            editor_prompt.set(String::new());
                            editor_deliver.set(String::new());
                            editor_mode.set(ScheduleMode::OneTime);
                            editor_date.set(String::new());
                            editor_time.set(String::new());
                            editor_preset.set(RecurringPreset::Daily);
                            editor_weekday.set(DEFAULT_WEEKDAY);
                            editor_interval_count.set("1".to_string());
                            editor_interval_unit.set(IntervalUnit::Minutes);
                            editor_cron.set(String::new());
                            editor_skills.set(Vec::new());
                            editor_advanced_open.set(false);
                            editor_provider.set(String::new());
                            editor_model.set(String::new());
                            editor_base_url.set(String::new());
                            editor_no_agent.set(false);
                            editor_script.set(String::new());
                            editor_workdir.set(String::new());
                            editor_continuity.set(false);
                            editor_context_from.set(String::new());
                            editor_enabled_toolsets.set(Vec::new());
                            save_error.set(None);
                            field_error.set(None);
                            editor_open.set(true);
                        },
                        "+ NEW JOB"
                    }
                    // Phase 49.6 Plan 02 (D-02/D-04, UI-SPEC Structural
                    // Note 2): same position `ToolsProfileBar` occupies in
                    // `.screen-actions` on the Tools screen.
                    SchedulesScopeBar {
                        scope: schedule_scope_sig,
                        on_scope_changed: move |_| {
                            let scope = schedule_scope_sig();
                            spawn(async move {
                                if let Ok(fresh) = get_schedules(scope).await {
                                    schedule_list_sig.set(fresh.rows);
                                    unreadable_profiles_sig.set(fresh.unreadable_profiles);
                                }
                            });
                        },
                    }
                }
            }

            div { class: "tabs",
                button {
                    class: if active_tab_val == "jobs" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("jobs"),
                    "Jobs"
                }
                button {
                    class: if active_tab_val == "blueprints" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("blueprints"),
                    "Blueprints"
                }
            }

            if let Some(ref row) = delete_confirm_val {
                div {
                    class: "panel",
                    style: "margin-top:14px;border-color:rgba(248,81,73,0.45);background:rgba(248,81,73,0.06);",
                    div { class: "panel-title", style: "color:var(--red);", "Delete job" }
                    p { style: "color:var(--text);font-size:12px;margin:0 0 12px 0;",
                        "This permanently removes the job and its run history. Continue?"
                    }
                    p { style: "color:var(--gray);font-size:11px;margin:0 0 12px 0;", "{row.name}" }
                    div { style: "display:flex;gap:10px;",
                        button {
                            class: "btn btn--sm",
                            style: "background:var(--red);border-color:var(--red);",
                            disabled: deleting_val,
                            onclick: move |_| {
                                let Some(row) = delete_confirm.read().clone() else { return };
                                let id = row.id.clone();
                                let row_profile = row.profile.clone();
                                let scope = schedule_scope_sig();
                                deleting.set(true);
                                spawn(async move {
                                    match delete_schedule(id.clone(), Some(row_profile)).await {
                                        Ok(()) => {
                                            if let Ok(fresh) = get_schedules(scope).await {
                                                schedule_list_sig.set(fresh.rows);
                                                unreadable_profiles_sig.set(fresh.unreadable_profiles);
                                            }
                                            deleting.set(false);
                                            delete_confirm.set(None);
                                        }
                                        Err(_e) => {
                                            deleting.set(false);
                                        }
                                    }
                                });
                            },
                            if deleting_val { "DELETING…" } else { "DELETE" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: deleting_val,
                            onclick: move |_| delete_confirm.set(None),
                            "CANCEL"
                        }
                    }
                }
            }

            if editor_open_val {
                div { class: "panel", style: "margin-top:14px;",
                    div {
                        style: "display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:8px;",
                        div { class: "panel-title",
                            if editor_is_new_val { "New Job" } else { "Edit Job" }
                        }
                        // Phase 49.6 Plan 04 (D-15, UI-SPEC E6): the edit-panel
                        // entry point into the same SaveBlueprintDialog the
                        // per-row action opens. Not rendered while the editor
                        // holds a new, unsaved job — an unsaved job has
                        // nothing to export yet. Owns no pending/error state
                        // of its own; both belong to the dialog.
                        if !editor_is_new_val {
                            if let Some(ref current_row) = editor_current_row_val {
                                button {
                                    class: "btn btn--ghost btn--sm",
                                    onclick: {
                                        let row_for_dialog = current_row.clone();
                                        move |_| blueprint_save_target.set(Some(row_for_dialog.clone()))
                                    },
                                    "SAVE AS BLUEPRINT"
                                }
                            }
                        }
                    }

                    if show_write_collapse_note {
                        div {
                            class: "gw-static-note",
                            style: "margin-bottom:10px;",
                            "Scope is ALL PROFILES — new jobs can't be written there. This job will be created in ROOT."
                        }
                    }

                    if let Some((ref line1, ref line2)) = save_error_val {
                        div { style: "color:var(--red);font-size:12px;margin-bottom:10px;",
                            div { "{line1}" }
                            div { style: "margin-top:2px;", "{line2}" }
                        }
                    }

                    div { class: "field-row",
                        div { class: "field-label", "Job name" }
                        input {
                            class: "field-input",
                            placeholder: "e.g. daily-report",
                            value: "{editor_name_val}",
                            oninput: move |e| editor_name.set(e.value()),
                        }
                    }

                    div { class: "sched-mode-tabs tabs",
                        button {
                            class: if editor_mode_val == ScheduleMode::OneTime { "tab is-active" } else { "tab" },
                            onclick: move |_| {
                                editor_mode.set(ScheduleMode::OneTime);
                                editor_date.set(String::new());
                                editor_time.set(String::new());
                                editor_preset.set(RecurringPreset::Daily);
                                editor_weekday.set(DEFAULT_WEEKDAY);
                                editor_interval_count.set("1".to_string());
                                editor_interval_unit.set(IntervalUnit::Minutes);
                                editor_cron.set(String::new());
                                field_error.set(None);
                            },
                            "ONE-TIME"
                        }
                        button {
                            class: if editor_mode_val == ScheduleMode::Recurring { "tab is-active" } else { "tab" },
                            onclick: move |_| {
                                editor_mode.set(ScheduleMode::Recurring);
                                editor_date.set(String::new());
                                editor_time.set(String::new());
                                editor_preset.set(RecurringPreset::Daily);
                                editor_weekday.set(DEFAULT_WEEKDAY);
                                editor_interval_count.set("1".to_string());
                                editor_interval_unit.set(IntervalUnit::Minutes);
                                editor_cron.set(String::new());
                                field_error.set(None);
                            },
                            "RECURRING"
                        }
                        button {
                            class: if editor_mode_val == ScheduleMode::Interval { "tab is-active" } else { "tab" },
                            onclick: move |_| {
                                editor_mode.set(ScheduleMode::Interval);
                                editor_date.set(String::new());
                                editor_time.set(String::new());
                                editor_preset.set(RecurringPreset::Daily);
                                editor_weekday.set(DEFAULT_WEEKDAY);
                                editor_interval_count.set("1".to_string());
                                editor_interval_unit.set(IntervalUnit::Minutes);
                                editor_cron.set(String::new());
                                field_error.set(None);
                            },
                            "INTERVAL"
                        }
                        button {
                            class: if editor_mode_val == ScheduleMode::Advanced { "tab is-active" } else { "tab" },
                            onclick: move |_| {
                                editor_mode.set(ScheduleMode::Advanced);
                                editor_date.set(String::new());
                                editor_time.set(String::new());
                                editor_preset.set(RecurringPreset::Daily);
                                editor_weekday.set(DEFAULT_WEEKDAY);
                                editor_interval_count.set("1".to_string());
                                editor_interval_unit.set(IntervalUnit::Minutes);
                                editor_cron.set(String::new());
                                field_error.set(None);
                            },
                            "ADVANCED"
                        }
                    }

                    if editor_mode_val == ScheduleMode::OneTime {
                        div { class: "field-row",
                            div { class: "field-label", "Date" }
                            input {
                                class: "field-input",
                                r#type: "date",
                                value: "{editor_date_val}",
                                oninput: move |e| {
                                    field_error.set(None);
                                    editor_date.set(e.value());
                                },
                            }
                        }
                        div { class: "field-row",
                            div { class: "field-label", "Time" }
                            input {
                                class: "field-input",
                                r#type: "time",
                                value: "{editor_time_val}",
                                oninput: move |e| {
                                    field_error.set(None);
                                    editor_time.set(e.value());
                                },
                            }
                        }
                        if let Some(ref msg) = field_error_val {
                            div { style: "color:var(--red);font-size:11px;margin-top:-6px;", "{msg}" }
                        }
                    } else if editor_mode_val == ScheduleMode::Recurring {
                        div { class: "field-row",
                            div { class: "field-label", "Preset" }
                            select {
                                class: "field-input",
                                value: preset_value_str(editor_preset_val),
                                onchange: move |e| {
                                    editor_preset.set(preset_from_value_str(&e.value()));
                                },
                                option { value: "hourly", "Hourly" }
                                option { value: "daily", "Daily" }
                                option { value: "weekly", "Weekly" }
                            }
                        }
                        div { class: "field-row",
                            div { class: "field-label",
                                if editor_preset_val == RecurringPreset::Hourly { "Minute" } else { "Time" }
                            }
                            input {
                                class: "field-input",
                                r#type: "time",
                                value: "{editor_time_val}",
                                oninput: move |e| editor_time.set(e.value()),
                            }
                        }
                        if editor_preset_val == RecurringPreset::Weekly {
                            div { class: "field-row",
                                div { class: "field-label", "Weekday" }
                                select {
                                    class: "field-input",
                                    value: "{editor_weekday_val}",
                                    onchange: move |e| {
                                        if let Ok(v) = e.value().parse::<u32>() {
                                            editor_weekday.set(v);
                                        }
                                    },
                                    for (val, label) in WEEKDAY_OPTIONS {
                                        option { value: "{val}", "{label}" }
                                    }
                                }
                            }
                        }
                    } else if editor_mode_val == ScheduleMode::Interval {
                        div { class: "field-row",
                            div { class: "field-label", "Every" }
                            div { style: "display:flex;gap:8px;",
                                input {
                                    class: "field-input",
                                    r#type: "number",
                                    min: "1",
                                    style: "max-width:100px;",
                                    value: "{editor_interval_count_val}",
                                    oninput: move |e| editor_interval_count.set(e.value()),
                                }
                                select {
                                    class: "field-input",
                                    style: "max-width:140px;",
                                    value: interval_unit_value_str(editor_interval_unit_val),
                                    onchange: move |e| {
                                        editor_interval_unit.set(interval_unit_from_value_str(&e.value()));
                                    },
                                    option { value: "min", "Minutes" }
                                    option { value: "hour", "Hours" }
                                }
                            }
                        }
                    } else {
                        div { class: "field-row",
                            div { class: "field-label", "Cron expression" }
                            textarea {
                                class: "field-input sched-cron-input",
                                rows: "2",
                                placeholder: "0 9 * * *",
                                value: "{editor_cron_val}",
                                oninput: move |e| {
                                    field_error.set(None);
                                    editor_cron.set(e.value());
                                },
                            }
                        }
                        if !editor_cron_val.trim().is_empty() {
                            div { class: "help sched-next-run",
                                "Preview (humanized text — not a computed next-run time): {humanize_schedule(&editor_cron_val)}"
                            }
                        }
                        if let Some(ref msg) = field_error_val {
                            div { style: "color:var(--red);font-size:11px;margin-top:-6px;", "{msg}" }
                        }
                    }

                    div { class: "field-row",
                        div { class: "field-label", "Prompt" }
                        textarea {
                            class: "field-input",
                            style: "min-height:64px;resize:vertical;",
                            rows: "3",
                            placeholder: "Summarize yesterday's activity and send a digest.",
                            value: "{editor_prompt_val}",
                            oninput: move |e| editor_prompt.set(e.value()),
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label",
                            "Delivery"
                            span { class: "help", "local, origin, telegram:<chat_id>, webhook:<url>" }
                        }
                        input {
                            class: "field-input",
                            placeholder: "local",
                            value: "{editor_deliver_val}",
                            oninput: move |e| editor_deliver.set(e.value()),
                        }
                    }

                    // Phase 49.5 Plan 06 (D-15, UI-SPEC "Surface
                    // Specifications" 4 item 5): SKILLS checkbox list — no
                    // new listing endpoint (`list_skills()` already backs
                    // the Skills screen), no new scroll-box class
                    // (`.gw-whitelist-rows` reused verbatim).
                    div { class: "gw-field-group",
                        span { class: "gw-field-label", "SKILLS (OPTIONAL)" }
                        if installed_skills.is_empty() {
                            p { class: "gw-field-help", "No skills installed." }
                            p { class: "gw-field-help",
                                "Install a skill from the Skills screen to make it selectable here — this job will run without one."
                            }
                        } else {
                            div { class: "gw-whitelist-rows",
                                for skill in installed_skills.iter() {
                                    {
                                        let skill_name = skill.name.clone();
                                        let skill_name_for_toggle = skill_name.clone();
                                        let checked = editor_skills_val.contains(&skill_name);
                                        let installed_names: Vec<String> = installed_skills.iter().map(|s| s.name.clone()).collect();
                                        rsx! {
                                            div { class: "gw-checkbox-row", key: "{skill_name}",
                                                input {
                                                    id: "gw-sched-skill-{skill_name}",
                                                    r#type: "checkbox",
                                                    checked,
                                                    onchange: move |evt| {
                                                        let mut current = editor_skills.read().clone();
                                                        if evt.checked() {
                                                            if !current.contains(&skill_name_for_toggle) {
                                                                current.push(skill_name_for_toggle.clone());
                                                            }
                                                        } else {
                                                            current.retain(|s| s != &skill_name_for_toggle);
                                                        }
                                                        // Preserve rendered-list order regardless of click order —
                                                        // what the operator sees top-to-bottom is what is stored.
                                                        let ordered: Vec<String> = installed_names
                                                            .iter()
                                                            .filter(|n| current.contains(n))
                                                            .cloned()
                                                            .collect();
                                                        editor_skills.set(ordered);
                                                    },
                                                }
                                                label { r#for: "gw-sched-skill-{skill_name}", "{skill_name}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        p { class: "gw-field-help",
                            "Selected skills are loaded before the prompt runs — the cron sets when, the skill sets how."
                        }
                    }

                    // Phase 49.5 Plan 06 (D-15): ADVANCED FIELDS disclosure —
                    // reuses `.gw-advanced-toggle` verbatim from the gateway
                    // chat-config form (`chat_config_form.rs:675-684`). Every
                    // control here is optional; leaving all of them
                    // blank/unchecked produces the same `CronJob` the narrow
                    // form produced before this phase. Deliberately no
                    // operator-scoping control — every cron job is
                    // root-scoped today (D-17).
                    button {
                        r#type: "button",
                        class: "btn btn--ghost btn--sm gw-advanced-toggle",
                        "aria-expanded": if editor_advanced_open_val { "true" } else { "false" },
                        onclick: move |_| {
                            let cur = *editor_advanced_open.read();
                            editor_advanced_open.set(!cur);
                        },
                        if editor_advanced_open_val { "▾ Advanced Fields" } else { "▸ Advanced Fields" }
                    }
                    if editor_advanced_open_val {
                        div { class: "sched-bp-2col",
                            div { class: "gw-field-group",
                                span { class: "gw-field-label", "PROVIDER" }
                                select {
                                    class: "gw-input",
                                    value: "{editor_provider_val}",
                                    onchange: move |e| editor_provider.set(e.value()),
                                    option { value: "", "Default" }
                                    for name in provider_names.iter() {
                                        option { value: "{name}", key: "{name}", "{name}" }
                                    }
                                }
                            }
                            div { class: "gw-field-group",
                                span { class: "gw-field-label", "MODEL" }
                                input {
                                    class: "gw-input",
                                    value: "{editor_model_val}",
                                    oninput: move |e| editor_model.set(e.value()),
                                }
                            }
                        }
                        div { class: "gw-field-group",
                            span { class: "gw-field-label", "BASE URL OVERRIDE" }
                            input {
                                class: "gw-input",
                                placeholder: "https://api.example.com/v1",
                                value: "{editor_base_url_val}",
                                oninput: move |e| editor_base_url.set(e.value()),
                            }
                        }
                        div { class: "sched-bp-2col",
                            div { class: "gw-checkbox-row",
                                input {
                                    id: "gw-sched-no-agent",
                                    r#type: "checkbox",
                                    checked: editor_no_agent_val,
                                    onchange: move |e| editor_no_agent.set(e.checked()),
                                }
                                label { r#for: "gw-sched-no-agent",
                                    "no_agent: run the script only and deliver stdout verbatim"
                                }
                            }
                            div { class: "gw-field-group",
                                span { class: "gw-field-label", "SCRIPT" }
                                input {
                                    class: "gw-input",
                                    placeholder: "relative/path/in/scripts",
                                    value: "{editor_script_val}",
                                    oninput: move |e| editor_script.set(e.value()),
                                }
                            }
                        }
                        div { class: "gw-field-group",
                            span { class: "gw-field-label", "WORKDIR" }
                            input {
                                class: "gw-input",
                                placeholder: "/absolute/project/path",
                                value: "{editor_workdir_val}",
                                oninput: move |e| editor_workdir.set(e.value()),
                            }
                        }
                        div { class: "gw-checkbox-row",
                            input {
                                id: "gw-sched-continuity",
                                r#type: "checkbox",
                                checked: editor_continuity_val,
                                onchange: move |e| editor_continuity.set(e.checked()),
                            }
                            label { r#for: "gw-sched-continuity",
                                "continuity: each run sees the previous run's output (dedupe, pick up where it left off)"
                            }
                        }
                        div { class: "sched-bp-2col",
                            div { class: "gw-field-group",
                                span { class: "gw-field-label", "CONTEXT_FROM JOB IDS" }
                                textarea {
                                    class: "gw-input",
                                    rows: "3",
                                    placeholder: "one job id per line",
                                    value: "{editor_context_from_val}",
                                    oninput: move |e| editor_context_from.set(e.value()),
                                }
                            }
                            div { class: "gw-field-group",
                                span { class: "gw-field-label", "ENABLED_TOOLSETS" }
                                if available_toolsets.is_empty() {
                                    p { class: "gw-field-help", "No toolsets available." }
                                    p { class: "gw-field-help", "Toolset catalog is empty — check your tools configuration." }
                                } else {
                                    div { class: "gw-whitelist-rows",
                                        for toolset in available_toolsets.iter() {
                                            {
                                                let toolset_name = toolset.clone();
                                                let toolset_name_for_toggle = toolset_name.clone();
                                                let checked = editor_enabled_toolsets_val.contains(&toolset_name);
                                                let all_names = available_toolsets.clone();
                                                rsx! {
                                                    div { class: "gw-checkbox-row", key: "{toolset_name}",
                                                        input {
                                                            id: "gw-sched-toolset-{toolset_name}",
                                                            r#type: "checkbox",
                                                            checked,
                                                            onchange: move |evt| {
                                                                let mut current = editor_enabled_toolsets.read().clone();
                                                                if evt.checked() {
                                                                    if !current.contains(&toolset_name_for_toggle) {
                                                                        current.push(toolset_name_for_toggle.clone());
                                                                    }
                                                                } else {
                                                                    current.retain(|s| s != &toolset_name_for_toggle);
                                                                }
                                                                let ordered: Vec<String> = all_names
                                                                    .iter()
                                                                    .filter(|n| current.contains(n))
                                                                    .cloned()
                                                                    .collect();
                                                                editor_enabled_toolsets.set(ordered);
                                                            },
                                                        }
                                                        label { r#for: "gw-sched-toolset-{toolset_name}", "{toolset_name}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { style: "display:flex;gap:10px;margin-top:6px;",
                        button {
                            class: "btn btn--sm",
                            disabled: !can_save,
                            onclick: move |_| {
                                if !can_save {
                                    return;
                                }
                                // Pattern B: read all signal values into owned
                                // locals BEFORE spawn — no borrow across .await.
                                let id_local = editor_id.read().clone();
                                let is_new_local = *editor_is_new.read();
                                let name_local = editor_name.read().clone();
                                let prompt_local = editor_prompt.read().clone();
                                let deliver_local = editor_deliver.read().clone();
                                let mode_local = *editor_mode.read();
                                let date_local = editor_date.read().clone();
                                let time_local = editor_time.read().clone();
                                let preset_local = *editor_preset.read();
                                let weekday_local = *editor_weekday.read();
                                let interval_count_local = editor_interval_count.read().clone();
                                let interval_unit_local = *editor_interval_unit.read();
                                let cron_local = editor_cron.read().clone();
                                let skills_local = editor_skills.read().clone();
                                let provider_local = editor_provider.read().clone();
                                let model_local = editor_model.read().clone();
                                let base_url_local = editor_base_url.read().clone();
                                let no_agent_local = *editor_no_agent.read();
                                let script_local = editor_script.read().clone();
                                let workdir_local = editor_workdir.read().clone();
                                let continuity_local = *editor_continuity.read();
                                let context_from_local = editor_context_from.read().clone();
                                let enabled_toolsets_local = editor_enabled_toolsets.read().clone();
                                let tz_local = tz_name_for_save.clone();

                                let (hour_opt, minute_opt) = match parse_hh_mm(&time_local) {
                                    Some((h, m)) => (Some(h), Some(m)),
                                    None => (None, None),
                                };

                                let input = ScheduleBuilderInput {
                                    mode: Some(mode_local),
                                    one_time_date: (!date_local.is_empty()).then(|| date_local.clone()),
                                    one_time_time: (!time_local.is_empty()).then(|| time_local.clone()),
                                    tz_name: tz_local,
                                    now_rfc3339: Some(now_rfc3339()),
                                    recurring_preset: Some(preset_local),
                                    hour: hour_opt,
                                    minute: minute_opt,
                                    weekday: Some(weekday_local),
                                    interval_count: interval_count_local.trim().parse::<i64>().ok(),
                                    interval_unit: Some(interval_unit_local),
                                    advanced_raw: (!cron_local.is_empty()).then(|| cron_local.clone()),
                                };

                                let schedule_string = match build_schedule_string(input) {
                                    Ok(s) => s,
                                    Err(msg) => {
                                        field_error.set(Some(msg));
                                        return;
                                    }
                                };

                                field_error.set(None);
                                saving.set(true);
                                save_error.set(None);

                                // Phase 49.5 Plan 06 (D-15): the remaining
                                // eight advanced fields plus `continuity` are
                                // left at their `Default` here — task 3 adds
                                // the ADVANCED FIELDS controls and fills this
                                // same struct from their own signals.
                                let write_input = ScheduleWriteInput {
                                    id: (!is_new_local).then(|| id_local.clone()),
                                    name: name_local,
                                    schedule: schedule_string,
                                    prompt: prompt_local,
                                    deliver: deliver_local,
                                    skills: skills_local,
                                    provider: (!provider_local.is_empty()).then_some(provider_local),
                                    model: (!model_local.is_empty()).then_some(model_local),
                                    base_url: (!base_url_local.is_empty()).then_some(base_url_local),
                                    script: (!script_local.is_empty()).then_some(script_local),
                                    workdir: (!workdir_local.is_empty()).then_some(workdir_local),
                                    no_agent: no_agent_local,
                                    context_from: parse_context_from(&context_from_local),
                                    enabled_toolsets: (!enabled_toolsets_local.is_empty())
                                        .then_some(enabled_toolsets_local),
                                    continuity: continuity_local,
                                    // Phase 49.6 Plan 02 (D-04): defaults to
                                    // the current selector scope — Task 3
                                    // wires the editor panel's true write
                                    // target (root on create-while-aggregate
                                    // per D-04; the source row's own profile
                                    // on edit, not necessarily the scope
                                    // selector's current value).
                                    profile: schedule_scope_sig(),
                                };

                                let scope_for_refetch = schedule_scope_sig();
                                spawn(async move {
                                    let result = if is_new_local {
                                        create_schedule(write_input).await
                                    } else {
                                        update_schedule(write_input).await
                                    };
                                    match result {
                                        Ok(_row) => {
                                            // Re-fetch authoritative state directly
                                            // (NOT the schedules_resource restart method —
                                            // see module doc).
                                            if let Ok(fresh) = get_schedules(scope_for_refetch).await {
                                                schedule_list_sig.set(fresh.rows);
                                                unreadable_profiles_sig.set(fresh.unreadable_profiles);
                                            }
                                            saving.set(false);
                                            editor_open.set(false);
                                        }
                                        Err(e) => {
                                            saving.set(false);
                                            save_error.set(Some(map_save_error(&e)));
                                        }
                                    }
                                });
                            },
                            if saving_val { "SAVING…" } else { "SAVE JOB" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: saving_val,
                            onclick: move |_| editor_open.set(false),
                            "CANCEL"
                        }
                    }
                }
            }

            // Phase 49.6 Plan 04 (D-15): the one save-as-blueprint dialog,
            // reachable from either entry point above. Placement here is
            // arbitrary — `.gw-form-overlay` is `position: fixed; inset: 0`,
            // so it overlays the whole viewport regardless of DOM position.
            if let Some(ref source_row) = blueprint_save_target_val {
                SaveBlueprintDialog {
                    row: source_row.clone(),
                    on_close: move |_| blueprint_save_target.set(None),
                    // Bump the tick so the Blueprints tab refetches and the
                    // just-saved blueprint appears as a SAVED card without a
                    // page reload.
                    on_saved: move |_| {
                        let cur = *blueprint_refresh_tick.peek();
                        blueprint_refresh_tick.set(cur.wrapping_add(1));
                    },
                }
            }

            if active_tab_val == "jobs" {
            // Phase 49.6 Plan 02 (D-03, hard requirement — UI-SPEC E3):
            // once per view, above the row list, whenever the resolved
            // rows include at least one non-root row. Gated on `!is_loading`
            // so it never flashes during an in-flight fetch.
            if !is_loading && has_non_root_row {
                div { class: "gw-warn", style: "margin-top:14px;", "{NON_ROOT_SENTENCE}" }
            }
            // Phase 49.6 Plan 02 (D-04, UI-SPEC E10): the aggregate scope's
            // degrade-gracefully notice — one line per unreadable profile,
            // below the non-root banner when both are present. Rendered
            // independently of whether the READABLE rows happen to be
            // empty (E10 is its own element, not gated on the Jobs list's
            // own empty state) — one unreadable profile must never look
            // identical to "nothing to show".
            if !is_loading && !unreadable_profiles.is_empty() {
                div {
                    class: "gw-warn",
                    style: "margin-top:14px;max-height:120px;overflow-y:auto;",
                    for name in unreadable_profiles.iter() {
                        p {
                            key: "{name}",
                            style: "margin:0 0 4px 0;",
                            "Could not read schedules for profile \"{name}\" — showing the rest."
                        }
                    }
                }
            }
            if is_loading {
                div { class: "row-list", style: "margin-top:14px;",
                    ScheduleGhostRow {}
                    ScheduleGhostRow {}
                    ScheduleGhostRow {}
                    ScheduleGhostRow {}
                }
            } else if let Some(ref err) = list_error_val {
                div {
                    style: "color:var(--red);font-size:12px;margin-top:14px;",
                    p { style: "margin:0 0 2px 0;font-weight:700;", "Could not load schedules." }
                    p { style: "margin:0 0 10px 0;", "{err}" }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            retrying.set(true);
                            let scope = schedule_scope_sig();
                            spawn(async move {
                                match get_schedules(scope).await {
                                    Ok(view) => {
                                        schedule_list_sig.set(view.rows);
                                        unreadable_profiles_sig.set(view.unreadable_profiles);
                                        list_error.set(None);
                                        seeded.set(true);
                                    }
                                    Err(e) => {
                                        list_error.set(Some(e.to_string()));
                                    }
                                }
                                retrying.set(false);
                            });
                        },
                        "RETRY"
                    }
                }
            } else if schedule_list.is_empty() {
                div {
                    class: "card",
                    style: "align-items:center;text-align:center;padding:32px 18px;margin-top:14px;",
                    div { class: "card-title", "No scheduled jobs" }
                    div { class: "card-meta", style: "margin-top:4px;",
                        "Create a job to run a prompt on a timer and deliver the output somewhere."
                    }
                }
            } else {
                div { class: "row-list", style: "margin-top:14px;",
                    // 9 children matching the 9-column `.sched-row` grid
                    // template (dot / JOB / SCHEDULE / DELIVERY / LAST RUN /
                    // NEXT RUN / PROFILE / STATE / ACTIONS) — Phase 49.6
                    // Plan 02 (D-01/D-04): widened from 8 to 9, inserting
                    // PROFILE between NEXT RUN and STATE. `ScheduleRowView`
                    // and `ScheduleGhostRow` must declare the SAME 9
                    // children in the SAME position or the grid desyncs —
                    // this comment, and theirs, are the only thing tying
                    // the three sites together.
                    div { class: "sched-row head",
                        span {}
                        span { "JOB" }
                        span { "SCHEDULE" }
                        span { "DELIVERY" }
                        span { "LAST RUN" }
                        span { "NEXT RUN" }
                        span { "PROFILE" }
                        span { style: "text-align:right;", "STATE" }
                        span {}
                    }
                    for row in schedule_list.iter() {
                        {
                        let tz_name_for_this_row = tz_name_for_edit.clone();
                        rsx! {
                        ScheduleRowView {
                            key: "{row.id}",
                            schedule: row.clone(),
                            on_edit: move |r: ScheduleRow| {
                                editor_is_new.set(false);
                                editor_current_row.set(Some(r.clone()));
                                editor_id.set(r.id.clone());
                                editor_name.set(r.name.clone());
                                editor_prompt.set(r.prompt.clone());
                                editor_deliver.set(r.deliver.clone());
                                let prefill = detect_editor_prefill(&r.schedule_raw, tz_name_for_this_row.as_deref());
                                editor_mode.set(prefill.mode);
                                editor_date.set(prefill.date);
                                editor_time.set(prefill.time);
                                editor_preset.set(prefill.preset);
                                editor_weekday.set(prefill.weekday);
                                editor_interval_count.set(prefill.interval_count);
                                editor_interval_unit.set(prefill.interval_unit);
                                editor_cron.set(prefill.cron);
                                editor_skills.set(r.skills.clone());
                                // Pre-fill every advanced control from the
                                // stored row (T-49.5-06-04) so an untouched
                                // save round-trips unchanged, and open the
                                // disclosure automatically when any of them
                                // is set — a stored value must never hide
                                // behind a collapsed control.
                                editor_advanced_open.set(schedule_row_has_advanced_fields(&r));
                                editor_provider.set(r.provider.clone().unwrap_or_default());
                                editor_model.set(r.model.clone().unwrap_or_default());
                                editor_base_url.set(r.base_url.clone().unwrap_or_default());
                                editor_no_agent.set(r.no_agent);
                                editor_script.set(r.script.clone().unwrap_or_default());
                                editor_workdir.set(r.workdir.clone().unwrap_or_default());
                                editor_continuity.set(r.continuity);
                                editor_context_from.set(
                                    r.context_from.clone().unwrap_or_default().join("\n"),
                                );
                                editor_enabled_toolsets.set(r.enabled_toolsets.clone().unwrap_or_default());
                                save_error.set(None);
                                field_error.set(None);
                                editor_open.set(true);
                            },
                            on_delete: move |r: ScheduleRow| {
                                delete_confirm.set(Some(r));
                            },
                            on_blueprint: move |r: ScheduleRow| {
                                blueprint_save_target.set(Some(r));
                            },
                            on_toggled: move |fresh: SchedulesView| {
                                schedule_list_sig.set(fresh.rows);
                                unreadable_profiles_sig.set(fresh.unreadable_profiles);
                            },
                            on_run_now: move |fresh: SchedulesView| {
                                schedule_list_sig.set(fresh.rows);
                                unreadable_profiles_sig.set(fresh.unreadable_profiles);
                            },
                            scope: schedule_scope_sig(),
                        }
                        }
                        }
                    }
                }
            }
            } else if blueprint_list.is_empty() {
                // Defensive only — D-05 means blueprints ship with the
                // binary, so this should never actually render. Mirrors
                // the empty-jobs state's shape (`.card`, not the `.grid`)
                // rather than an empty `.grid`.
                div {
                    class: "card",
                    style: "align-items:center;text-align:center;padding:32px 18px;margin-top:14px;",
                    div { class: "card-title", "No blueprints available." }
                    div { class: "card-meta", style: "margin-top:4px;",
                        "The blueprint catalog ships with the binary — if you're seeing this, the build is missing its catalog data. Use the Jobs tab to create a job manually."
                    }
                }
            } else {
                div { class: "grid",
                    for view in blueprint_list.iter().cloned() {
                        {
                            let bp_key = view.key.clone();
                            let bp_key_for_toggle = bp_key.clone();
                            let is_expanded = expanded_key_val.as_deref() == Some(bp_key.as_str());
                            let bp_slots = view.slots.clone();
                            let bp_slots_for_toggle = bp_slots.clone();
                            // Phase 49.6 Plan 04 (D-10/D-13): branch inside
                            // the existing per-card rendering on Task 1's
                            // kind discriminator rather than adding a
                            // parallel loop — one list, one grid, no second
                            // surface.
                            let is_saved = view.kind == BlueprintKind::Saved;
                            rsx! {
                                div { class: "card", key: "{bp_key}",
                                    div { class: "sched-bp-head",
                                        div { class: "sched-bp-saved",
                                            div { class: "card-title", "{view.title}" }
                                            // Curated cards get no badge at all —
                                            // the curated catalog stays visually
                                            // untouched (D-10). Bare `.pill`, no
                                            // colour modifier: accent teal stays
                                            // reserved for CTA buttons, the
                                            // active-tab underline, `.pill.teal`
                                            // badges and focus rings.
                                            if is_saved {
                                                span { class: "pill", "SAVED" }
                                            }
                                        }
                                        button {
                                            class: "btn btn--sm",
                                            onclick: move |_| {
                                                if expanded_key.read().as_deref() == Some(bp_key_for_toggle.as_str()) {
                                                    expanded_key.set(None);
                                                } else {
                                                    let mut vals = BTreeMap::new();
                                                    for slot in &bp_slots_for_toggle {
                                                        vals.insert(
                                                            slot.name.clone(),
                                                            slot.default.clone().unwrap_or_default(),
                                                        );
                                                    }
                                                    blueprint_values.set(vals);
                                                    blueprint_error.set(None);
                                                    blueprint_field_errors.set(BTreeMap::new());
                                                    expanded_key.set(Some(bp_key_for_toggle.clone()));
                                                }
                                            },
                                            if is_expanded { "Cancel" } else { "Set up" }
                                        }
                                    }
                                    div { class: "card-body", "{view.description}" }
                                    if !view.tags.is_empty() {
                                        div { class: "sched-bp-tags",
                                            for tag in view.tags.iter().cloned() {
                                                span { class: "sched-bp-tag", "{tag}" }
                                            }
                                        }
                                    }
                                    if is_expanded {
                                        div { class: "sched-bp-divider" }
                                        if is_saved {
                                            // Phase 49.6 Plan 04 (D-10, UI-SPEC
                                            // E9): a saved blueprint has no
                                            // slots by construction — no
                                            // slot-fill inputs render; a
                                            // read-only preview renders in
                                            // their place instead, so the
                                            // operator sees exactly what will
                                            // run before committing. Skips
                                            // `validate_blueprint_slots`
                                            // entirely — there are no slots to
                                            // validate.
                                            div { class: "gw-static-note sched-bp-preview",
                                                if let Some(ref sched) = view.schedule_preview {
                                                    div { "SCHEDULE: {sched}" }
                                                }
                                                if let Some(ref deliver) = view.deliver_preview {
                                                    div { "DELIVER: {deliver}" }
                                                }
                                                if let Some(ref prompt) = view.prompt_preview {
                                                    div { "{prompt}" }
                                                }
                                            }
                                            if let Some((ref line1, ref line2)) = blueprint_error_val {
                                                div { style: "color:var(--red);font-size:12px;",
                                                    div { "{line1}" }
                                                    div { style: "margin-top:2px;", "{line2}" }
                                                }
                                            }
                                            button {
                                                class: "btn btn--sm",
                                                disabled: blueprint_scheduling_val,
                                                onclick: move |_| {
                                                    let key_local = bp_key.clone();
                                                    blueprint_scheduling.set(true);
                                                    blueprint_error.set(None);
                                                    let scope = schedule_scope_sig();
                                                    let write_profile = schedule_scope_sig();
                                                    spawn(async move {
                                                        match create_schedule_from_saved_blueprint(
                                                            key_local,
                                                            write_profile,
                                                        )
                                                        .await
                                                        {
                                                            Ok(_row) => {
                                                                if let Ok(fresh) = get_schedules(scope).await {
                                                                    schedule_list_sig.set(fresh.rows);
                                                                    unreadable_profiles_sig.set(fresh.unreadable_profiles);
                                                                }
                                                                blueprint_scheduling.set(false);
                                                                expanded_key.set(None);
                                                            }
                                                            Err(e) => {
                                                                blueprint_scheduling.set(false);
                                                                blueprint_error.set(Some(map_blueprint_error(&e)));
                                                            }
                                                        }
                                                    });
                                                },
                                                if blueprint_scheduling_val { "⏱ Scheduling…" } else { "⏱ Schedule it" }
                                            }
                                        } else {
                                        for slot in bp_slots.iter().cloned() {
                                            {
                                                let slot_name = slot.name.clone();
                                                let slot_name_for_input = slot_name.clone();
                                                let current_val = blueprint_values_val
                                                    .get(&slot_name)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                let field_err = blueprint_field_errors_val.get(&slot_name).cloned();
                                                rsx! {
                                                    div { class: "gw-field-group",
                                                        label { class: "gw-field-label", "{slot.label}" }
                                                        if slot.slot_type == "time" {
                                                            input {
                                                                class: "gw-input",
                                                                r#type: "time",
                                                                value: "{current_val}",
                                                                oninput: move |e| {
                                                                    blueprint_field_errors.write().remove(&slot_name_for_input);
                                                                    blueprint_values
                                                                        .write()
                                                                        .insert(slot_name_for_input.clone(), e.value());
                                                                },
                                                            }
                                                        } else if slot.slot_type == "enum" || slot.slot_type == "weekdays" {
                                                            select {
                                                                class: "gw-input",
                                                                value: "{current_val}",
                                                                onchange: move |e| {
                                                                    blueprint_field_errors.write().remove(&slot_name_for_input);
                                                                    blueprint_values
                                                                        .write()
                                                                        .insert(slot_name_for_input.clone(), e.value());
                                                                },
                                                                for opt in slot.options.iter().cloned() {
                                                                    option { value: "{opt}", "{opt}" }
                                                                }
                                                            }
                                                        } else {
                                                            input {
                                                                class: "gw-input",
                                                                r#type: "text",
                                                                value: "{current_val}",
                                                                oninput: move |e| {
                                                                    blueprint_field_errors.write().remove(&slot_name_for_input);
                                                                    blueprint_values
                                                                        .write()
                                                                        .insert(slot_name_for_input.clone(), e.value());
                                                                },
                                                            }
                                                        }
                                                        // The `deliver` slot's help is the UI-SPEC three-clause
                                                        // Blueprints-tab copy verbatim, scoped to this form
                                                        // only — the manual dialog's own shorter Delivery help
                                                        // (`schedules_api.rs`'s catalog `help` string) is left
                                                        // untouched below and stays unmodified.
                                                        if slot.name == "deliver" {
                                                            div { class: "gw-field-help",
                                                                "origin = the chat you set this up from (or your configured home channel when created from the dashboard); local = save only, no message; or any connected platform name"
                                                            }
                                                        } else if let Some(ref help) = slot.help {
                                                            div { class: "gw-field-help", "{help}" }
                                                        }
                                                        if let Some(ref msg) = field_err {
                                                            div { style: "color:var(--red);font-size:11px;", "{msg}" }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if let Some((ref line1, ref line2)) = blueprint_error_val {
                                            div { style: "color:var(--red);font-size:12px;",
                                                div { "{line1}" }
                                                div { style: "margin-top:2px;", "{line2}" }
                                            }
                                        }
                                        button {
                                            class: "btn btn--sm",
                                            disabled: blueprint_scheduling_val,
                                            onclick: move |_| {
                                                let vals = blueprint_values.read().clone();
                                                let errors = validate_blueprint_slots(&bp_slots, &vals);
                                                if !errors.is_empty() {
                                                    blueprint_field_errors.set(errors);
                                                    return;
                                                }
                                                // A slot marked optional and left blank is omitted from
                                                // the fill entirely, matching upstream fill_blueprint
                                                // semantics — never sent as an empty string.
                                                let values_vec: Vec<(String, String)> = bp_slots
                                                    .iter()
                                                    .filter_map(|slot| {
                                                        let v = vals.get(&slot.name).cloned().unwrap_or_default();
                                                        if v.trim().is_empty() && slot.optional {
                                                            None
                                                        } else {
                                                            Some((slot.name.clone(), v))
                                                        }
                                                    })
                                                    .collect();
                                                let key_local = bp_key.clone();
                                                blueprint_scheduling.set(true);
                                                blueprint_error.set(None);
                                                blueprint_field_errors.set(BTreeMap::new());
                                                let scope = schedule_scope_sig();
                                                let write_profile = schedule_scope_sig();
                                                spawn(async move {
                                                    match create_schedule_from_blueprint(
                                                        key_local,
                                                        values_vec,
                                                        write_profile,
                                                    )
                                                    .await
                                                    {
                                                        Ok(_row) => {
                                                            if let Ok(fresh) = get_schedules(scope).await {
                                                                schedule_list_sig.set(fresh.rows);
                                                                unreadable_profiles_sig.set(fresh.unreadable_profiles);
                                                            }
                                                            blueprint_scheduling.set(false);
                                                            expanded_key.set(None);
                                                        }
                                                        Err(e) => {
                                                            blueprint_scheduling.set(false);
                                                            blueprint_error.set(Some(map_blueprint_error(&e)));
                                                        }
                                                    }
                                                });
                                            },
                                            if blueprint_scheduling_val { "⏱ Scheduling…" } else { "⏱ Schedule it" }
                                        }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Phase 49.6 Plan 02 (D-02/D-04): map the store selector's `scope` value
/// (the SAME three-state convention `get_schedules`/`ScheduleWriteInput`
/// use) to its closed-trigger and menu-row display label. Pure and unit
/// tested — mirrors `profile_bar.rs::scope_label_for`'s split between the
/// display label and the wire value it maps to (the root row displays
/// `ROOT` but sets `Some("default")`, never the literal string `"ROOT"`).
fn scope_trigger_label(scope: &Option<String>) -> String {
    match scope.as_deref() {
        None => "ALL PROFILES".to_string(),
        Some("default") => "ROOT".to_string(),
        Some(name) => name.to_string(),
    }
}

/// Phase 49.6 Plan 02 (D-02/D-04): the Jobs-tab store selector. Copies
/// `ToolsProfileBar`'s interaction pattern (trigger + dropdown menu, a
/// `use_resource`/`use_context` roster read) verbatim but writes net-new
/// `sched-scope-*` CSS in `screens.css` — `tools.css`'s `tools-profile-*`
/// classes are a scoped `document::Link` on the Tools screen only and are
/// not loaded here (UI-SPEC Structural Note 3). Reads the ONE shared,
/// cached profile roster (`SharedProfilesCtx`, provided at the `HermesApp`
/// root) rather than a second `list_profiles()` fetch (D-04 discretion,
/// and the project-wide rule 49.4 D-19 established: reuse the existing
/// profile roster).
#[component]
fn SchedulesScopeBar(scope: Signal<Option<String>>, on_scope_changed: EventHandler<()>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).
    let mut menu_open: Signal<bool> = use_signal(|| false);
    let profiles_resource =
        use_context::<crate::components::hermes_app::profile_topbar::SharedProfilesCtx>().0;

    // Extract data BEFORE rsx! — signal-borrow discipline
    // (iron_hermes_ui/clippy.toml: no GenerationalRef held across RSX).
    let is_loading = profiles_resource().is_none();
    let load_error: Option<String> = match profiles_resource() {
        Some(Err(ref e)) => Some(e.to_string()),
        _ => None,
    };
    let profiles: Vec<crate::protocol::ProfileRow> = match profiles_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };

    // UI-SPEC E1 `error` (explicit): when the roster fails to load, scope
    // falls back to ROOT — the aggregate view cannot be trusted complete
    // when the roster itself could not be enumerated, and ROOT never
    // depends on the roster at all. Reads `profiles_resource()` directly
    // inside the effect (not a captured plain bool) so this re-runs
    // exactly when the resource's own resolved value changes.
    use_effect(move || {
        if matches!(profiles_resource(), Some(Err(_))) {
            scope.set(Some("default".to_string()));
        }
    });

    // The trigger label comes DIRECTLY from `scope` — never from the
    // roster resource — so it keeps its LAST-KNOWN label while the roster
    // is in flight and never opens empty (UI-SPEC E1 loading backstop).
    let trigger_label = scope_trigger_label(&scope.read());
    let is_open = *menu_open.read();

    rsx! {
        div { class: "sched-scope-bar",
            button {
                class: "sched-scope-trigger",
                "aria-label": "Select job store scope — currently {trigger_label}",
                title: "{trigger_label}",
                onclick: move |_| {
                    let cur = *menu_open.read();
                    menu_open.set(!cur);
                },
                span { "SCOPE {trigger_label}" }
                span { "aria-hidden": "true", "▾" }
            }
            if is_open {
                div { class: "sched-scope-menu",
                    // ALL PROFILES first, always — the aggregate default
                    // (D-04) and the lens-clear row (UI-SPEC E1 populated).
                    div {
                        class: "sched-scope-item",
                        "aria-label": "Select ALL PROFILES scope",
                        onclick: move |_| {
                            scope.set(None);
                            menu_open.set(false);
                            on_scope_changed.call(());
                        },
                        "ALL PROFILES"
                    }
                    // ROOT second, always selectable — never gated on the
                    // roster's own outcome (mirrors `ToolsProfileBar`: ROOT
                    // is reachable even when the fetch failed or is still
                    // loading).
                    div {
                        class: "sched-scope-item",
                        "aria-label": "Select ROOT scope",
                        onclick: move |_| {
                            scope.set(Some("default".to_string()));
                            menu_open.set(false);
                            on_scope_changed.call(());
                        },
                        "ROOT"
                    }
                    if is_loading {
                        div {
                            style: "padding:8px 12px;color:var(--text-dim);font-size:11px;",
                            "Loading profiles…"
                        }
                    } else {
                        for row in profiles.iter().cloned() {
                            {
                                let name_for_click = row.name.clone();
                                rsx! {
                                    div {
                                        class: "sched-scope-item",
                                        key: "{row.name}",
                                        title: "{row.name}",
                                        "aria-label": "Select profile {row.name} scope",
                                        onclick: move |_| {
                                            scope.set(Some(name_for_click.clone()));
                                            menu_open.set(false);
                                            on_scope_changed.call(());
                                        },
                                        "{row.name}"
                                    }
                                }
                            }
                        }
                    }
                    // Constructed message only, never raw filesystem/parser
                    // error text (mirrors `profile_bar.rs:158`'s own
                    // convention) — the menu still opened with ALL
                    // PROFILES/ROOT selectable above.
                    if let Some(reason) = load_error {
                        div {
                            style: "padding:8px 12px;color:var(--amber);font-size:11px;",
                            "Could not list profiles — {reason}"
                        }
                    }
                }
            }
        }
    }
}

/// Ghost placeholder row for the loading state — visually distinct from
/// both the empty panel and a populated row (opacity-dimmed, no data).
#[component]
fn ScheduleGhostRow() -> Element {
    // 9 children matching the 9-column `.sched-row` grid template (dot /
    // JOB / SCHEDULE / DELIVERY / LAST RUN / NEXT RUN / PROFILE / STATE /
    // ACTIONS) — see `ScheduleRowView`'s doc comment. Phase 49.6 Plan 02
    // (UI-SPEC E2 loading backstop): the PROFILE placeholder participates
    // in this same skeleton, declaring the same 9 children as the head row.
    rsx! {
        div {
            class: "sched-row",
            style: "opacity:0.35;",
            "aria-hidden": "true",
            span {}
            div { class: "row-main",
                span { class: "row-title", "…" }
            }
            span { class: "sched-cron", "…" }
            span { class: "row-sub", "…" }
            span { class: "row-sub", "…" }
            span { class: "row-sub", "…" }
            span { class: "sched-profile-cell", "…" }
            span {}
            span {}
        }
    }
}

/// Phase 49.6 Plan 02 (D-03, hard requirement — UI-SPEC Copywriting
/// Contract "Non-root explanatory banner"): the verbatim sentence carried
/// in BOTH the per-row NON-ROOT badge `title`/`aria-label` (this
/// component) and the once-per-view non-root banner (Task 3,
/// `SchedulesScopeBar`'s sibling). Deliberately states the CLI daemon's
/// real, DEGRADED capability — `build_cron_runner_ctx`
/// (`ironhermes-cli/src/cron.rs`) passes `None` for the skill registry,
/// memory manager, hook registry and MCP manager and an empty delivery
/// registry, so a sentence promising a fully-capable daemon would be
/// false. A single `const` is the mechanism that keeps both surfaces
/// byte-identical — never hand-retype this string at the second call site.
const NON_ROOT_SENTENCE: &str = "Jobs on a non-root profile fire only under that profile's own \
ironhermes cron daemon, run separately from this gateway — this gateway ticks the root store only. \
That daemon does not yet load skills or memory, and cannot deliver replies to chat platforms, so a \
non-root job runs with reduced capability until it does.";

/// The ONE save-as-blueprint dialog (Phase 49.6 Plan 04, D-15). Reachable
/// from two entry points — the per-row `BLUEPRINT` action and the Edit Job
/// panel's `SAVE AS BLUEPRINT` entry — both of which open THIS SAME
/// component with the SAME source `ScheduleRow` and submit through the SAME
/// `save_job_as_blueprint` server fn. Two entry points into one dialog and
/// one server fn is the whole point of D-15; a second save path would drift
/// on validation, on the exclusion warning, and on the destination copy.
///
/// Built on the `.gw-form-overlay`/`.gw-form-modal` shell copied from
/// `gateway/chat_config_form.rs` (click-outside-to-close via the overlay's
/// own `onclick`, `stop_propagation` on the modal body) — no new modal CSS.
#[component]
fn SaveBlueprintDialog(
    row: ScheduleRow,
    on_close: EventHandler<()>,
    /// Fired ONLY on a successful save, before `on_close`. Distinct from
    /// `on_close` (which also fires on cancel and click-outside) so a
    /// dismissed dialog does not trigger a pointless refetch.
    on_saved: EventHandler<()>,
) -> Element {
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<(String, String)>> = use_signal(|| None);

    let name_val = name.read().clone();
    let description_val = description.read().clone();
    let saving_val = *saving.read();
    let error_val = error.read().clone();

    // Live "will save as" preview, recomputed on every keystroke — see
    // `sanitize_preview`'s own doc comment for why this can't just call the
    // real `sanitize_blueprint_name` from here.
    let sanitized_preview = sanitize_preview(&name_val);
    let first_line_char_count = description_first_line(&description_val).chars().count();
    let counter_text = description_counter_text(first_line_char_count);
    let counter_is_amber = first_line_char_count >= 180;

    // UI-SPEC E7 conditional warning: only when the source job carries a
    // setting D-12 deliberately excludes from the emitted SKILL.md.
    let has_excluded_fields = row.script.is_some() || row.workdir.is_some() || row.base_url.is_some();

    // export_blueprint requires a body (D-15) — an empty description is not
    // submittable, matching an empty name.
    let can_submit = !saving_val && !name_val.trim().is_empty() && !description_val.trim().is_empty();

    let job_id = row.id.clone();
    let job_profile = row.profile.clone();
    let job_name = row.name.clone();

    rsx! {
        div {
            class: "gw-form-overlay",
            "aria-label": "Save {job_name} as a reusable blueprint",
            onclick: move |_| on_close.call(()),
            div {
                class: "gw-form-modal",
                // Stop propagation so a click inside the modal never bubbles
                // up to the overlay's close-on-click-outside handler.
                onclick: move |evt| evt.stop_propagation(),
                div { class: "gw-form-header",
                    span { class: "gw-form-title", "Save as blueprint" }
                    button {
                        r#type: "button",
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Close save-as-blueprint form",
                        onclick: move |_| on_close.call(()),
                        "×"
                    }
                }

                div { class: "gw-field-group",
                    label { class: "gw-field-label", r#for: "gw-blueprint-name-input", "BLUEPRINT NAME" }
                    p { class: "gw-field-help",
                        "Only a–z, 0–9, - and _ — everything else becomes a dash. Leading/trailing dashes are trimmed."
                    }
                    input {
                        id: "gw-blueprint-name-input",
                        class: "gw-input",
                        "aria-label": "Blueprint name",
                        placeholder: "e.g. daily-report",
                        value: "{name_val}",
                        oninput: move |e| name.set(e.value()),
                    }
                    p { class: "gw-field-help", "Will save as: {sanitized_preview}" }
                }

                div { class: "gw-field-group",
                    label { class: "gw-field-label", r#for: "gw-blueprint-description-input", "DESCRIPTION" }
                    p { class: "gw-field-help",
                        "The first line becomes the blueprint's description (200 characters max, shown on its card). Anything after that is saved as notes."
                    }
                    textarea {
                        id: "gw-blueprint-description-input",
                        class: "gw-input",
                        "aria-label": "Blueprint description",
                        rows: "4",
                        value: "{description_val}",
                        oninput: move |e| description.set(e.value()),
                    }
                    p {
                        class: "gw-field-help",
                        style: if counter_is_amber { "color:var(--amber);" },
                        "{counter_text}"
                    }
                }

                p { class: "gw-field-help",
                    "Saves to the shared skills library, not this job's profile — every profile can install it from the Blueprints tab."
                }

                if has_excluded_fields {
                    div { class: "gw-warn",
                        "This job's script, working directory, and base URL settings will NOT be included in the saved blueprint — they're excluded so a shared blueprint can never carry an unsandboxed command."
                    }
                }

                if let Some((ref line1, ref line2)) = error_val {
                    div { class: "gw-form-errors",
                        span {
                            class: "pill red",
                            style: "flex-direction:column;align-items:flex-start;white-space:normal;gap:2px;",
                            span { "{line1}" }
                            span { "{line2}" }
                        }
                    }
                }

                div { style: "display:flex;gap:10px;",
                    button {
                        class: "btn btn--sm",
                        disabled: !can_submit,
                        onclick: move |_| {
                            if !can_submit {
                                return;
                            }
                            let id = job_id.clone();
                            let profile = Some(job_profile.clone());
                            let blueprint_name = name_val.clone();
                            let body = description_val.clone();
                            saving.set(true);
                            error.set(None);
                            spawn(async move {
                                match save_job_as_blueprint(id, profile, blueprint_name, body).await {
                                    Ok(_installed_name) => {
                                        saving.set(false);
                                        // Refetch BEFORE closing: the server
                                        // has already swapped its skill
                                        // registry, so the new blueprint is
                                        // visible to the very next read.
                                        on_saved.call(());
                                        on_close.call(());
                                    }
                                    Err(e) => {
                                        saving.set(false);
                                        error.set(Some(map_blueprint_save_error(&e)));
                                    }
                                }
                            });
                        },
                        if saving_val { "Saving…" } else { "Save blueprint" }
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        disabled: saving_val,
                        onclick: move |_| on_close.call(()),
                        "CANCEL"
                    }
                }
            }
        }
    }
}

#[component]
fn ScheduleRowView(
    schedule: ScheduleRow,
    on_edit: EventHandler<ScheduleRow>,
    on_delete: EventHandler<ScheduleRow>,
    /// Phase 49.6 Plan 04 (D-15): opens the shared `SaveBlueprintDialog`
    /// with this row as the source job. The row action owns no pending or
    /// error state of its own — both belong to the dialog it opens.
    on_blueprint: EventHandler<ScheduleRow>,
    on_toggled: EventHandler<SchedulesView>,
    on_run_now: EventHandler<SchedulesView>,
    /// Phase 49.6 Plan 02 (D-04): the Jobs list's current selector scope —
    /// used ONLY to re-fetch the same view after a row-scoped write, never
    /// as the write target itself (the write always targets the row's OWN
    /// `schedule.profile`, per D-01: a toggle/run-now on a specific job
    /// must act on the store that job actually lives in, regardless of
    /// which scope the operator is currently viewing).
    scope: Option<String>,
) -> Element {
    let mut toggling = use_signal(|| false);
    let mut running = use_signal(|| false);
    let toggling_val = *toggling.read();
    let running_val = *running.read();

    let row_for_edit = schedule.clone();
    let row_for_delete = schedule.clone();
    let row_for_blueprint = schedule.clone();
    let id_for_toggle = schedule.id.clone();
    let id_for_run = schedule.id.clone();
    let profile_for_toggle = schedule.profile.clone();
    let profile_for_run = schedule.profile.clone();
    let scope_for_toggle = scope.clone();
    let scope_for_run = scope.clone();
    let currently_enabled = schedule.enabled;
    let last_run_display = schedule
        .last_run_at
        .clone()
        .unwrap_or_else(|| "—".to_string());
    // Task 2 (D-13): em-dash when the job is disabled or has never run —
    // `schedules_api.rs`'s `build_schedule_row` already gates `next_run_at`
    // on `enabled` server-side, so `None` here is authoritative, never a
    // client-computed guess.
    let next_run_display = schedule
        .next_run_at
        .clone()
        .unwrap_or_else(|| "—".to_string());
    // Task 2 (D-12): humanized text in the cell, raw string on hover via
    // `title` — see module doc "Weekday numbering" for why this goes
    // through `display_schedule_text` rather than `humanize_schedule`
    // directly.
    let schedule_cell_text = display_schedule_text(&schedule.schedule_raw);
    let schedule_raw_title = schedule.schedule_raw.clone();

    // Phase 49.6 Plan 02 (D-01/D-03, UI-SPEC E2/E3): a row whose profile is
    // the root sentinel renders `ROOT`; any other non-empty profile is
    // non-root and gets the `NON-ROOT` badge. An empty profile string
    // (should not occur — every row is tagged server-side — but the UI
    // must never crash on it) degrades to a neutral em-dash placeholder
    // rather than an empty cell (UI-SPEC E2 partial backstop). When
    // ownership cannot be determined (the empty-string case), the row's
    // `aria-label` fails VISIBLE per E3's error backstop: a false positive
    // (showing the warning when it may not apply) is safe, a false
    // negative is the exact failure D-03 exists to prevent.
    let is_root_row = schedule.profile == "default";
    let is_empty_profile = schedule.profile.is_empty();
    let is_non_root_row = !is_root_row;
    let profile_slug = schedule.profile.clone();

    rsx! {
        // 9 children matching the 9-column `.sched-row` grid template
        // (dot / JOB / SCHEDULE / DELIVERY / LAST RUN / NEXT RUN / PROFILE /
        // STATE / ACTIONS) — Phase 49.6 Plan 02 (D-01/D-04): widened from 8
        // to 9. The head row (`ScreenSchedules`) and `ScheduleGhostRow` both
        // declare the same 9 children in the same position — see this
        // module's other two doc comments naming the same contract.
        div {
            class: "sched-row",
            class: if !schedule.is_valid { "is-invalid" },
            "aria-label": if is_non_root_row { "{NON_ROOT_SENTENCE}" },
            span { style: if schedule.enabled { "color:var(--green);" } else { "color:var(--amber);" }, "●" }
            div { class: "row-main",
                span { class: "row-title", "{schedule.name}" }
                span { class: "row-sub", "—" }
            }
            span { class: "sched-cron", title: "{schedule_raw_title}", "{schedule_cell_text}" }
            span { class: "row-sub", "{schedule.deliver}" }
            span { class: "row-sub", "{last_run_display}" }
            span { class: "row-sub", "{next_run_display}" }
            span { class: "sched-profile-cell",
                if is_empty_profile {
                    "—"
                } else if is_root_row {
                    "ROOT"
                } else {
                    span { class: "row-sub", title: "{profile_slug}", "{profile_slug}" }
                    span {
                        class: "pill amber",
                        title: "{NON_ROOT_SENTENCE}",
                        "NON-ROOT"
                    }
                }
            }
            div { class: "sched-state",
                if !schedule.is_valid {
                    span { class: "pill amber", "INVALID" }
                } else {
                    span {
                        class: if schedule.enabled { "pill green" } else { "pill amber" },
                        if schedule.enabled { "ACTIVE" } else { "PAUSED" }
                    }
                    div {
                        class: if schedule.enabled { "tgl on" } else { "tgl" },
                        role: "switch",
                        aria_checked: "{schedule.enabled}",
                        "aria-disabled": if toggling_val { "true" } else { "false" },
                        onclick: move |_| {
                            if toggling_val {
                                return;
                            }
                            let id = id_for_toggle.clone();
                            let profile = profile_for_toggle.clone();
                            let next_enabled = !currently_enabled;
                            let scope_for_refetch = scope_for_toggle.clone();
                            toggling.set(true);
                            spawn(async move {
                                if set_schedule_enabled(id, next_enabled, Some(profile)).await.is_ok() {
                                    if let Ok(fresh) = get_schedules(scope_for_refetch).await {
                                        on_toggled.call(fresh);
                                    }
                                }
                                toggling.set(false);
                            });
                        },
                    }
                }
            }
            div { class: "sched-actions",
                button {
                    class: "btn btn--ghost btn--sm",
                    disabled: running_val,
                    onclick: move |_| {
                        if running_val {
                            return;
                        }
                        let id = id_for_run.clone();
                        let profile = profile_for_run.clone();
                        let scope_for_refetch = scope_for_run.clone();
                        running.set(true);
                        spawn(async move {
                            if run_schedule_now(id, Some(profile)).await.is_ok() {
                                if let Ok(fresh) = get_schedules(scope_for_refetch).await {
                                    on_run_now.call(fresh);
                                }
                            }
                            running.set(false);
                        });
                    },
                    if running_val { "…" } else { "RUN NOW" }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| on_edit.call(row_for_edit.clone()),
                    "EDIT"
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| on_delete.call(row_for_delete.clone()),
                    "DELETE"
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    "aria-label": "Save {schedule.name} as a reusable blueprint",
                    onclick: move |_| on_blueprint.call(row_for_blueprint.clone()),
                    "BLUEPRINT"
                }
            }
        }
    }
}

#[cfg(test)]
mod schedules_scope_bar_tests {
    use super::*;

    #[test]
    fn none_scope_labels_as_all_profiles() {
        assert_eq!(scope_trigger_label(&None), "ALL PROFILES");
    }

    #[test]
    fn default_scope_labels_as_root() {
        assert_eq!(scope_trigger_label(&Some("default".to_string())), "ROOT");
    }

    #[test]
    fn named_profile_scope_labels_as_the_bare_slug() {
        assert_eq!(scope_trigger_label(&Some("zig".to_string())), "zig");
    }
}

#[cfg(test)]
mod editor_prefill_tests {
    use super::*;

    #[test]
    fn hourly_cron_detects_recurring_hourly_mode() {
        let p = detect_editor_prefill("5 * * * *", None);
        assert_eq!(p.mode, ScheduleMode::Recurring);
        assert_eq!(p.preset, RecurringPreset::Hourly);
        assert_eq!(p.time, "00:05");
    }

    #[test]
    fn daily_cron_detects_recurring_daily_mode() {
        let p = detect_editor_prefill("5 9 * * *", None);
        assert_eq!(p.mode, ScheduleMode::Recurring);
        assert_eq!(p.preset, RecurringPreset::Daily);
        assert_eq!(p.time, "09:05");
    }

    /// dow = 2 in cron-crate numbering (1=Sun..7=Sat) is Monday — see module
    /// doc "Weekday numbering".
    #[test]
    fn weekly_cron_detects_recurring_weekly_mode_with_cron_crate_weekday() {
        let p = detect_editor_prefill("0 9 * * 2", None);
        assert_eq!(p.mode, ScheduleMode::Recurring);
        assert_eq!(p.preset, RecurringPreset::Weekly);
        assert_eq!(p.weekday, 2);
        assert_eq!(p.time, "09:00");
    }

    #[test]
    fn interval_string_in_minutes_detects_interval_mode() {
        let p = detect_editor_prefill("every 45m", None);
        assert_eq!(p.mode, ScheduleMode::Interval);
        assert_eq!(p.interval_unit, IntervalUnit::Minutes);
        assert_eq!(p.interval_count, "45");
    }

    #[test]
    fn interval_string_normalizes_whole_hours_to_hours_unit() {
        let p = detect_editor_prefill("every 120m", None);
        assert_eq!(p.mode, ScheduleMode::Interval);
        assert_eq!(p.interval_unit, IntervalUnit::Hours);
        assert_eq!(p.interval_count, "2");
    }

    #[test]
    fn one_time_rfc3339_detects_one_time_mode() {
        let p = detect_editor_prefill("2026-08-27T08:38:00Z", None);
        assert_eq!(p.mode, ScheduleMode::OneTime);
        assert_eq!(p.date, "2026-08-27");
        assert_eq!(p.time, "08:38");
    }

    #[test]
    fn unrecognized_cron_falls_back_to_advanced_mode_with_raw_string() {
        let p = detect_editor_prefill("0 9 1 * *", None);
        assert_eq!(p.mode, ScheduleMode::Advanced);
        assert_eq!(p.cron, "0 9 1 * *");
    }

    #[test]
    fn garbage_falls_back_to_advanced_mode() {
        let p = detect_editor_prefill("not a real schedule", None);
        assert_eq!(p.mode, ScheduleMode::Advanced);
        assert_eq!(p.cron, "not a real schedule");
    }

    #[test]
    fn weekly_cron_label_uses_cron_crate_numbering_not_posix() {
        // dow=2 is Monday in cron-crate numbering; `humanize_schedule`'s own
        // POSIX-numbered weekday_short_name would call this "Tue".
        assert_eq!(weekly_cron_label("0 9 * * 2"), Some("weekly Mon 09:00".to_string()));
    }

    #[test]
    fn display_schedule_text_falls_through_to_humanize_for_non_weekly_shapes() {
        assert_eq!(display_schedule_text("every 30m"), "every 30 min");
    }
}
