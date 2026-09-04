//! Automation blueprint catalog — parameterized automation templates with
//! typed slots that compile to a [`ironhermes_cron::CronJob`] via
//! [`ironhermes_cron::JobStore::add_job`] (D-02/D-04, `49.5-CONTEXT.md`).
//!
//! Ported from `hermes-agent/cron/blueprint_catalog.py` (see
//! `49.5-RESEARCH.md` "Pattern 1: Static in-binary catalog"). Day-of-week
//! tables are remapped from upstream POSIX numbering (0=Sunday) to this
//! workspace's `cron` crate numbering (1=Sunday..7=Saturday) — see
//! [`WEEKDAY_PRESETS`]/[`DAY_TO_DOW`]'s doc comments and
//! `schedules.rs`'s own module doc "Weekday numbering"
//! (RESEARCH.md Pitfall 1, the highest-risk item in this phase).
//!
//! Plan 49.5-01 (the phase tracer) shipped exactly ONE catalog entry
//! (`morning-brief`); plan 49.5-02 grew [`CATALOG`] to the full 16-entry
//! shipped set (D-06) and added `blueprint_dow_regression_tests`, the
//! Wave-0 day-of-week remap regression gate (`49.5-VALIDATION.md`).
//! [`fill_blueprint`] deliberately never populates `script`/`no_agent`/
//! `workdir`/`base_url`/etc — [`FilledBlueprint`] has no field for them, so a
//! curated catalog entry can never turn a scheduling form into arbitrary
//! local command execution (T-49.5-01-02/T-49.5-02-03).
//!
//! **Relocated here from `ironhermes-cron` in Plan 49.5-05** (Rule 4
//! escalation, operator-approved): `cmd_blueprint`'s `list`/`show` verbs
//! live in `ironhermes-core` and need to read this catalog with zero
//! `CommandContext` handle — but `ironhermes-cron` already depends on
//! `ironhermes-core`, so `ironhermes-core` cannot depend back on
//! `ironhermes-cron` (a real `cargo build` proved the cyclic-package-dependency
//! error). This module has zero dependency on anything cron-specific (no
//! `JobStore`, no `ScheduleParsed`) — it is pure data plus pure validation —
//! so moving the whole module here (not splitting it) is also the more
//! correct reading of D-04 "single source of truth for every surface": the
//! source of truth now sits at the leaf every surface can already reach.
//! `crates/ironhermes-cron/src/blueprint.rs` re-exports everything from here
//! so every existing external caller (`ironhermes_cron::blueprint::catalog()`,
//! `blueprints_api.rs`, the cron-runner, `writer_impl.rs`) keeps compiling
//! unchanged.

use std::collections::BTreeMap;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Model (D-02)
// ---------------------------------------------------------------------------

/// Slot widget/validation kind. `Weekdays` and `Enum` render the same
/// widget (a `select`) — the distinction is which lookup table populates
/// it, not the widget itself (UI-SPEC "Surface Specifications" §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotType {
    Time,
    Enum,
    Text,
    Weekdays,
}

/// A single fillable field on a blueprint.
#[derive(Debug, Clone, Copy)]
pub struct BlueprintSlot {
    pub name: &'static str,
    pub slot_type: SlotType,
    pub label: &'static str,
    pub default: Option<&'static str>,
    pub options: &'static [&'static str],
    pub optional: bool,
    pub help: Option<&'static str>,
    /// When `false`, `options` are suggestions rather than a closed set —
    /// any value is accepted (e.g. the `deliver` slot, whose real set of
    /// valid platforms depends on the operator's configured gateways).
    pub strict: bool,
}

/// A parameterized automation blueprint (D-02). The catalog's single
/// source-of-truth shape — every surface (web form, `/blueprint` command,
/// any future agent-facing seed prompt) reads from the same [`CATALOG`]
/// (D-04); nothing forks a second definition.
#[derive(Debug, Clone, Copy)]
pub struct AutomationBlueprint {
    pub key: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    /// Cron expression with `{slot}` placeholders, e.g.
    /// `"{minute} {hour} * * *"`. A literal cron string with no
    /// placeholders is a fixed schedule.
    pub schedule_template: &'static str,
    /// Seed prompt for the cron job; may contain `{slot}` placeholders.
    pub prompt_template: &'static str,
    pub slots: &'static [BlueprintSlot],
    pub deliver_default: &'static str,
    /// Skills the job loads before running.
    pub skills: &'static [&'static str],
    pub tags: &'static [&'static str],
}

/// The filled/resolved result of [`fill_blueprint`] — ready to hand to
/// [`ironhermes_cron::JobStore::add_job`]. Deliberately carries no `script`/
/// `no_agent`/`workdir`/`base_url` field (T-49.5-01-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilledBlueprint {
    pub name: String,
    pub schedule_expr: String,
    pub prompt: String,
    pub deliver: String,
    pub skills: Vec<String>,
}

/// Errors from [`fill_blueprint`]. Mirrors upstream `BlueprintFillError`
/// (`blueprint_catalog.py`), one variant per validation failure mode.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum BlueprintFillError {
    #[error("missing required value: {slot} ({label})")]
    MissingRequired {
        slot: &'static str,
        label: &'static str,
    },
    #[error("invalid time {value:?} — use HH:MM (24h)")]
    BadTime { value: String },
    #[error("{label} {value:?} not allowed — one of {options}")]
    UnknownEnumOption {
        label: &'static str,
        value: String,
        options: String,
    },
    #[error("unknown recurrence {value:?} — one of {options}")]
    UnknownRecurrence { value: String, options: String },
    #[error("unknown day {value:?}")]
    UnknownDay { value: String },
    #[error("invalid interval {value:?} — minutes as a positive integer")]
    BadInterval { value: String },
}

// ---------------------------------------------------------------------------
// Day-of-week tables — IronHermes numbering, NOT POSIX (RESEARCH Pitfall 1)
// ---------------------------------------------------------------------------

/// Named weekday recurrences -> cron `dow` field, remapped from upstream's
/// POSIX numbering (0=Sunday) to this workspace's `cron` crate numbering
/// (1=Sunday..7=Saturday — see `schedules.rs`'s `WEEKDAY_OPTIONS` and its
/// module doc "Weekday numbering"). Values are derived from
/// `WEEKDAY_OPTIONS`, never transcribed from the Python source verbatim:
/// upstream `weekdays="1-5"` (Mon-Fri under 0=Sun) becomes `"2-6"` here;
/// upstream `weekends="0,6"` becomes `"1,7"` here.
pub static WEEKDAY_PRESETS: &[(&str, &str)] =
    &[("everyday", "*"), ("weekdays", "2-6"), ("weekends", "1,7")];

/// Day name -> cron `dow` field, IronHermes numbering (1=Sunday..7=
/// Saturday). See [`WEEKDAY_PRESETS`]'s doc comment.
pub static DAY_TO_DOW: &[(&str, &str)] = &[
    ("sunday", "1"),
    ("monday", "2"),
    ("tuesday", "3"),
    ("wednesday", "4"),
    ("thursday", "5"),
    ("friday", "6"),
    ("saturday", "7"),
];

// ---------------------------------------------------------------------------
// Curated in-repo catalog (D-05: ships with the binary)
// ---------------------------------------------------------------------------

const MORNING_BRIEF_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "time",
        slot_type: SlotType::Time,
        label: "What time?",
        default: Some("08:00"),
        options: &[],
        optional: false,
        help: Some("24h local time, e.g. 08:00"),
        strict: true,
    },
    BlueprintSlot {
        name: "deliver",
        slot_type: SlotType::Enum,
        label: "Where to deliver?",
        default: Some("origin"),
        options: &["origin", "local", "telegram", "discord", "email"],
        optional: false,
        help: Some(
            "origin = the chat you set this up from (or your configured home \
             channel when created from the dashboard); local = save only, no \
             message; or any connected platform name",
        ),
        strict: false,
    },
];

// --- Shared slot factories. Mirror upstream `_TIME`/`_DELIVER`, plus the
// two slot shapes upstream repeats inline with no named factory: the
// `weekdays`-typed "Repeat on" slot and the single-day "Which day?" slot. ---

/// `_TIME` factory (`blueprint_catalog.py:106-109`) — every entry with a
/// `{minute}`/`{hour}` placeholder shares this exact label/help/type,
/// varying only the default time.
const fn time_slot(default: &'static str) -> BlueprintSlot {
    BlueprintSlot {
        name: "time",
        slot_type: SlotType::Time,
        label: "What time?",
        default: Some(default),
        options: &[],
        optional: false,
        help: Some("24h local time, e.g. 08:00"),
        strict: true,
    }
}

/// `_DELIVER` factory (`blueprint_catalog.py:110-117`) — shared verbatim by
/// every entry.
const DELIVER_SLOT: BlueprintSlot = BlueprintSlot {
    name: "deliver",
    slot_type: SlotType::Enum,
    label: "Where to deliver?",
    default: Some("origin"),
    options: &["origin", "local", "telegram", "discord", "email"],
    optional: false,
    help: Some(
        "origin = the chat you set this up from (or your configured home \
         channel when created from the dashboard); local = save only, no \
         message; or any connected platform name",
    ),
    strict: false,
};

/// The `weekdays`-typed "Repeat on" slot repeated inline (no named factory
/// upstream) across `custom-reminder`, `news-digest`, `bill-renewal-watch`,
/// `habit-checkin`, `learn-daily`, and `gratitude-journal` — varying only
/// its default preset.
const fn recurrence_slot(default: &'static str) -> BlueprintSlot {
    BlueprintSlot {
        name: "recurrence",
        slot_type: SlotType::Weekdays,
        label: "Repeat on",
        default: Some(default),
        options: &["everyday", "weekdays", "weekends"],
        optional: false,
        help: None,
        strict: true,
    }
}

/// The single-day `enum` "Which day?" slot repeated inline (no named
/// factory upstream) across `weekly-review`, `meal-plan`, and (Rule 1 fix —
/// see `49.5-02-SUMMARY.md`) `competitor-watch`.
const fn day_slot(default: &'static str) -> BlueprintSlot {
    BlueprintSlot {
        name: "day",
        slot_type: SlotType::Enum,
        label: "Which day?",
        default: Some(default),
        options: &["sunday", "monday", "friday", "saturday"],
        optional: false,
        help: None,
        strict: true,
    }
}

const IMPORTANT_MAIL_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "interval_min",
        slot_type: SlotType::Enum,
        label: "How often?",
        default: Some("30"),
        options: &["15", "30", "60"],
        optional: false,
        help: Some("minutes between checks"),
        strict: true,
    },
    BlueprintSlot {
        name: "criteria",
        slot_type: SlotType::Text,
        label: "Only notify me if the mail…",
        default: Some("needs a reply today, is from my manager or family, or mentions a deadline"),
        options: &[],
        optional: false,
        help: None,
        strict: true,
    },
    DELIVER_SLOT,
];

const WEEKLY_REVIEW_SLOTS: &[BlueprintSlot] = &[time_slot("18:00"), day_slot("sunday"), DELIVER_SLOT];

const WORKDAY_START_SLOTS: &[BlueprintSlot] = &[time_slot("09:00"), DELIVER_SLOT];

const CUSTOM_REMINDER_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "what",
        slot_type: SlotType::Text,
        label: "Remind me to…",
        default: Some("take a break and stretch"),
        options: &[],
        optional: false,
        help: None,
        strict: true,
    },
    time_slot("14:00"),
    recurrence_slot("everyday"),
    DELIVER_SLOT,
];

const EVENING_WINDDOWN_SLOTS: &[BlueprintSlot] = &[time_slot("21:00"), DELIVER_SLOT];

const NEWS_DIGEST_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "topic",
        slot_type: SlotType::Text,
        label: "What topic?",
        default: Some("AI and technology"),
        options: &[],
        optional: false,
        help: Some("a subject, product, person, or search phrase"),
        strict: true,
    },
    time_slot("18:00"),
    recurrence_slot("weekdays"),
    BlueprintSlot {
        name: "count",
        slot_type: SlotType::Enum,
        label: "How many bullets?",
        default: Some("5"),
        options: &["3", "5", "8"],
        optional: false,
        help: None,
        strict: true,
    },
    DELIVER_SLOT,
];

const BILL_RENEWAL_WATCH_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "what",
        slot_type: SlotType::Text,
        label: "What's due?",
        default: Some("my streaming subscription renews soon"),
        options: &[],
        optional: false,
        help: None,
        strict: true,
    },
    time_slot("10:00"),
    recurrence_slot("everyday"),
    DELIVER_SLOT,
];

const PRICE_WATCH_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "item",
        slot_type: SlotType::Text,
        label: "What exactly to watch?",
        default: Some("a product URL or exact flight/hotel/listing description"),
        options: &[],
        optional: false,
        help: Some("URL or precise description — variant, dates, seller"),
        strict: true,
    },
    BlueprintSlot {
        name: "condition",
        slot_type: SlotType::Text,
        label: "Alert me when…",
        default: Some("the all-in price drops below my target"),
        options: &[],
        optional: false,
        help: Some("threshold price (state the currency), availability, or terms change"),
        strict: true,
    },
    BlueprintSlot {
        name: "interval_h",
        slot_type: SlotType::Enum,
        label: "How often?",
        default: Some("6"),
        options: &["1", "3", "6", "12", "24"],
        optional: false,
        help: Some("hours between checks — be gentle with rate limits"),
        strict: true,
    },
    DELIVER_SLOT,
];

const COMPETITOR_WATCH_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "companies",
        slot_type: SlotType::Text,
        label: "Which companies?",
        default: Some("two or three competitors, by canonical name"),
        options: &[],
        optional: false,
        help: Some("canonical names and domains; aliases help dedup"),
        strict: true,
    },
    BlueprintSlot {
        name: "categories",
        slot_type: SlotType::Text,
        label: "Which events matter?",
        default: Some(
            "product launches, pricing changes, funding, partnerships, executive moves, incidents",
        ),
        options: &[],
        optional: false,
        help: None,
        strict: true,
    },
    time_slot("09:00"),
    // Rule 1 (bug) fix: upstream declares this as a `type="weekdays"`
    // "recurrence" slot defaulting to `"monday"` — but `"monday"` is not
    // one of WEEKDAY_PRESETS' three keys (everyday/weekdays/weekends), so
    // filling this blueprint with only its upstream defaults raises
    // BlueprintFillError("unknown recurrence 'monday'") every time (a real
    // upstream bug, not a remap issue). Ported here as the same single-day
    // `enum` "Which day?" slot `weekly-review`/`meal-plan` already use
    // (default "monday" IS one of ITS four options), preserving the
    // evident intent ("check every Monday") while making the default fill
    // parseable — see 49.5-02-SUMMARY.md.
    day_slot("monday"),
    DELIVER_SLOT,
];

const HABIT_CHECKIN_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "habit",
        slot_type: SlotType::Text,
        label: "Which habit?",
        default: Some("20 minutes of reading"),
        options: &[],
        optional: false,
        help: None,
        strict: true,
    },
    time_slot("20:00"),
    recurrence_slot("everyday"),
    DELIVER_SLOT,
];

const HYDRATION_MOVE_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "interval_hours",
        slot_type: SlotType::Enum,
        label: "How often?",
        default: Some("1"),
        options: &["1", "2", "3"],
        optional: false,
        help: Some("hours between nudges"),
        strict: true,
    },
    BlueprintSlot {
        name: "start_hour",
        slot_type: SlotType::Enum,
        label: "Start hour",
        default: Some("9"),
        options: &["7", "8", "9", "10"],
        optional: false,
        help: Some("first hour of the active window (24h)"),
        strict: true,
    },
    BlueprintSlot {
        name: "end_hour",
        slot_type: SlotType::Enum,
        label: "End hour",
        default: Some("17"),
        options: &["16", "17", "18", "19"],
        optional: false,
        help: Some("last hour of the active window (24h)"),
        strict: true,
    },
    DELIVER_SLOT,
];

const MEAL_PLAN_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "diet",
        slot_type: SlotType::Enum,
        label: "Diet?",
        default: Some("no restrictions"),
        options: &["no restrictions", "vegetarian", "vegan", "high-protein", "low-carb"],
        optional: false,
        help: None,
        strict: true,
    },
    BlueprintSlot {
        name: "meals",
        slot_type: SlotType::Enum,
        label: "Meals per day?",
        default: Some("dinner only"),
        options: &["dinner only", "lunch and dinner", "all three"],
        optional: false,
        help: None,
        strict: true,
    },
    BlueprintSlot {
        name: "effort",
        slot_type: SlotType::Enum,
        label: "Cooking effort?",
        default: Some("quick"),
        options: &["quick", "medium", "ambitious"],
        optional: false,
        help: None,
        strict: true,
    },
    time_slot("17:00"),
    day_slot("sunday"),
    DELIVER_SLOT,
];

const LEARN_DAILY_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "topic",
        slot_type: SlotType::Text,
        label: "Learn about…",
        default: Some("Spanish vocabulary"),
        options: &[],
        optional: false,
        help: None,
        strict: true,
    },
    time_slot("08:30"),
    recurrence_slot("weekdays"),
    DELIVER_SLOT,
];

const GRATITUDE_JOURNAL_SLOTS: &[BlueprintSlot] =
    &[time_slot("21:30"), recurrence_slot("everyday"), DELIVER_SLOT];

const ON_THIS_DAY_SLOTS: &[BlueprintSlot] = &[
    BlueprintSlot {
        name: "flavor",
        slot_type: SlotType::Enum,
        label: "What kind?",
        default: Some("on this day in history"),
        options: &["on this day in history", "word of the day", "science fact", "quote of the day"],
        optional: false,
        help: None,
        strict: true,
    },
    time_slot("07:30"),
    DELIVER_SLOT,
];

/// The compiled-in blueprint catalog (D-05). Declared as a `static` slice
/// so iteration order is the declaration order, never a hash-map order —
/// [`catalog`] and every consumer rely on this. The full 16-entry shipped
/// set (D-06), in upstream declaration order, matching the operator's
/// screenshots key-for-key and title-for-title. Every `schedule_template`
/// day-of-week value — the `{dow}` placeholder path via
/// [`WEEKDAY_PRESETS`]/[`DAY_TO_DOW`], and the two hard-coded ranges below —
/// uses this workspace's `cron`-crate numbering, never the upstream POSIX
/// numbering transcribed verbatim (RESEARCH.md Pitfall 1) — see
/// `blueprint_dow_regression_tests` for the regression gate.
pub static CATALOG: &[AutomationBlueprint] = &[
    AutomationBlueprint {
        key: "morning-brief",
        title: "Morning briefing",
        description: "A short daily briefing: today's calendar, weather, and anything urgent waiting on you.",
        category: "daily",
        schedule_template: "{minute} {hour} * * *",
        prompt_template: "Produce a concise morning briefing for the user: today's calendar events, the local weather, and any urgent items. When Gmail/Google Calendar are connected, follow the google-workspace skill's references/daily-brief.md procedure (exact day window, conflict detection, meeting prep, mail-to-meeting links). Keep it short and scannable. If no data sources are connected, give a brief good-morning with the date and offer to connect calendar/email.",
        slots: MORNING_BRIEF_SLOTS,
        deliver_default: "origin",
        skills: &["google-workspace"],
        tags: &["daily", "briefing"],
    },
    AutomationBlueprint {
        key: "important-mail",
        title: "Important-mail monitor",
        description: "Check your inbox periodically and ping you ONLY about mail that actually needs attention.",
        category: "email",
        schedule_template: "*/{interval_min} * * * *",
        prompt_template: "Check the user's inbox for new messages since the last run. Surface ONLY mail matching: {criteria}. Score candidates with the urgency classifier and deliver only what clears the bar; if nothing does, respond with [SILENT]. Requires a connected mail source; if none is configured, explain how to connect one and stop.",
        slots: IMPORTANT_MAIL_SLOTS,
        deliver_default: "origin",
        skills: &["email-inbox-triage"],
        tags: &["email", "monitor"],
    },
    AutomationBlueprint {
        key: "weekly-review",
        title: "Weekly review",
        description: "A weekly recap: what got done, what's still open, and what's coming up.",
        category: "weekly",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Run the weekly-review-planning skill's procedure for the user: review the completed week and coming 1-2 weeks across connected calendar, tasks, notes, and email; surface commitments, stalled projects, and waiting items; build a capacity-aware plan for next week. Recommendations and drafts only — no mutations without approval. Keep the output in the skill's seven-section shape.",
        slots: WEEKLY_REVIEW_SLOTS,
        deliver_default: "origin",
        skills: &["weekly-review-planning"],
        tags: &["weekly", "review"],
    },
    AutomationBlueprint {
        key: "workday-start",
        title: "Workday start reminder",
        description: "A weekday nudge with your agenda and top priorities.",
        category: "daily",
        // Hard-coded dow range (bypasses WEEKDAY_PRESETS/DAY_TO_DOW):
        // upstream literal "1-5" (Mon-Fri under 0=Sunday) becomes "2-6"
        // (Mon-Fri under this workspace's 1=Sunday..7=Saturday numbering).
        schedule_template: "{minute} {hour} * * 2-6",
        prompt_template: "Give the user a brief weekday start-of-day nudge: today's calendar and the 1-3 highest-priority things to focus on, inferred from recent context and any task tools. Encouraging, short, one message.",
        slots: WORKDAY_START_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["daily", "focus"],
    },
    AutomationBlueprint {
        key: "custom-reminder",
        title: "Custom reminder",
        description: "A recurring reminder in your own words, on your schedule.",
        category: "general",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Remind the user: {what}",
        slots: CUSTOM_REMINDER_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["reminder"],
    },
    AutomationBlueprint {
        key: "evening-winddown",
        title: "Evening wind-down",
        description: "An end-of-day check-in: tomorrow's calendar at a glance and anything you should prep tonight.",
        category: "daily",
        schedule_template: "{minute} {hour} * * *",
        prompt_template: "Give the user a short evening wind-down: tomorrow's calendar, any early commitments to prep for, and one gentle nudge to wrap up loose ends from today. Keep it calm and brief — one message. If no calendar is connected, just offer a friendly sign-off and the weather for tomorrow.",
        slots: EVENING_WINDDOWN_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["daily", "evening"],
    },
    AutomationBlueprint {
        key: "news-digest",
        title: "Topic news digest",
        description: "A recurring digest on a topic you care about — deduped against what was already sent, so only genuinely new items land.",
        category: "general",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Search the web for new and noteworthy items about: {topic}. Dedupe against what you sent in previous runs — only include genuinely new developments. Deliver a tight digest of at most {count} bullets, each one line with a link. If nothing new since last run, respond with [SILENT].",
        slots: NEWS_DIGEST_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["digest", "research"],
    },
    AutomationBlueprint {
        key: "bill-renewal-watch",
        title: "Bills & renewals reminder",
        description: "A heads-up before a recurring payment, subscription renewal, or due date — so nothing auto-charges by surprise.",
        category: "general",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Remind the user about an upcoming payment or renewal: {what}. Phrase it as an actionable heads-up (e.g. 'review or cancel before it renews'), not just a notification. One short message.",
        slots: BILL_RENEWAL_WATCH_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["reminder", "finance"],
    },
    AutomationBlueprint {
        key: "price-watch",
        title: "Price & availability watch",
        description: "Watch an exact product, flight, hotel, or listing and alert when your price or availability condition is met.",
        category: "general",
        schedule_template: "0 */{interval_h} * * *",
        prompt_template: "Load the product-price-monitor skill and run the tick for this watch: {item}. Alert condition: {condition}. Compare the normalized all-in price/availability against stored state, suppress duplicate alerts, and never overwrite last-known-good state with a failed fetch. If no condition is met, respond with [SILENT]. On the first run, execute the skill's setup phase first: pin the exact item, verify one live fetch, and write the watch contract state file.",
        slots: PRICE_WATCH_SLOTS,
        deliver_default: "origin",
        skills: &["product-price-monitor"],
        tags: &["prices", "shopping", "travel", "monitor"],
    },
    AutomationBlueprint {
        key: "competitor-watch",
        title: "Competitor news watch",
        description: "Track named companies for material news — launches, pricing, funding, filings — with a cited digest.",
        category: "general",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Load the competitor-news-monitor skill and run the tick for this watch: companies {companies}; event categories {categories}. Collect incrementally from the last cutoff, deduplicate by underlying event, score materiality against the watch contract, and deliver a cited digest of material events only. If there are no material events, respond with [SILENT]. On the first run, execute the skill's setup phase first: freeze the watchlist, build source coverage, and write the watch contract state file.",
        slots: COMPETITOR_WATCH_SLOTS,
        deliver_default: "origin",
        skills: &["competitor-news-monitor"],
        tags: &["competitors", "news", "monitor", "research"],
    },
    AutomationBlueprint {
        key: "habit-checkin",
        title: "Habit check-in",
        description: "A recurring nudge to keep a habit on track and reflect on whether you did it.",
        category: "general",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Nudge the user about their habit: {habit}. Ask whether they did it today, keep it warm and non-judgmental, and offer a one-line word of encouragement. One short message.",
        slots: HABIT_CHECKIN_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["habit", "wellbeing"],
    },
    AutomationBlueprint {
        key: "hydration-move",
        title: "Hydration & movement nudge",
        description: "A periodic nudge during the day to drink water, stand up, and stretch.",
        category: "general",
        // Hard-coded dow range (bypasses WEEKDAY_PRESETS/DAY_TO_DOW), same
        // remap as workday-start: upstream literal "1-5" becomes "2-6".
        schedule_template: "0 {start_hour}-{end_hour}/{interval_hours} * * 2-6",
        prompt_template: "Send the user a brief, friendly nudge to drink some water, stand up, and stretch for a moment. Vary the wording each time so it doesn't feel robotic. One short line.",
        slots: HYDRATION_MOVE_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["wellbeing", "focus"],
    },
    AutomationBlueprint {
        key: "meal-plan",
        title: "Weekly meal plan",
        description: "A weekly meal plan plus a consolidated grocery list, tuned to your diet and how much time you have to cook.",
        category: "weekly",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Build the user a meal plan for the coming week: {meals} per day, suited to a {diet} diet and roughly {effort} cooking effort. Include a consolidated grocery list grouped by aisle. Keep blueprints simple and skimmable.",
        slots: MEAL_PLAN_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["weekly", "food"],
    },
    AutomationBlueprint {
        key: "learn-daily",
        title: "Daily learning drip",
        description: "One bite-sized lesson a day on a topic you want to learn, building progressively over time.",
        category: "daily",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Teach the user one bite-sized lesson about: {topic}. Build on earlier lessons so it progresses rather than repeating. Keep it to a couple of short paragraphs with one concrete example, and end with a single question to check understanding.",
        slots: LEARN_DAILY_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["learning", "daily"],
    },
    AutomationBlueprint {
        key: "gratitude-journal",
        title: "Gratitude & reflection prompt",
        description: "A gentle evening prompt to reflect on the day and note what went well.",
        category: "general",
        schedule_template: "{minute} {hour} * * {dow}",
        prompt_template: "Send the user a short, warm reflection prompt for the end of the day — invite them to note one thing that went well, one thing they are grateful for, and one small win. If they reply, acknowledge it kindly. One message.",
        slots: GRATITUDE_JOURNAL_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["wellbeing", "reflection"],
    },
    AutomationBlueprint {
        key: "on-this-day",
        title: "On-this-day discovery",
        description: "A daily dose of curiosity: a notable historical event, fact, or word for the day.",
        category: "daily",
        schedule_template: "{minute} {hour} * * *",
        prompt_template: "Give the user one interesting '{flavor}' item for today — keep it short, surprising, and genuinely interesting. One or two sentences, no filler.",
        slots: ON_THIS_DAY_SLOTS,
        deliver_default: "origin",
        skills: &[],
        tags: &["daily", "curiosity"],
    },
];

/// The compiled-in blueprint catalog, in declaration order.
pub fn catalog() -> &'static [AutomationBlueprint] {
    CATALOG
}

/// Exact-key lookup only — never a prefix or fuzzy match. (`/blueprint`'s
/// own forgiving-name resolution is a later plan's concern, layered on top
/// of this, not inside it.)
fn find_in_catalog<'a>(catalog: &'a [AutomationBlueprint], key: &str) -> Option<&'a AutomationBlueprint> {
    catalog.iter().find(|bp| bp.key == key)
}

/// Look up a blueprint by its exact `key` in [`CATALOG`].
pub fn find_blueprint(key: &str) -> Option<&'static AutomationBlueprint> {
    find_in_catalog(CATALOG, key)
}

// ---------------------------------------------------------------------------
// Fill + validate + resolve schedule (D-02)
// ---------------------------------------------------------------------------

/// Parse an `HH:MM` 24-hour time string with a hand-written two-field split
/// plus range checks (hour 0..=23, minute 0..=59) — no regex crate needed.
fn parse_hh_mm(value: &str) -> Option<(u32, u32)> {
    let trimmed = value.trim();
    let (h, m) = trimmed.split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

/// Find every `{name}` placeholder in `template`, in order of first
/// appearance. Simple bracket-scan — no regex crate needed for this shape.
fn placeholder_names(template: &'static str) -> Vec<&'static str> {
    let mut names = Vec::new();
    let mut rest = template;
    let mut offset = 0usize;
    while let Some(start) = rest.find('{') {
        if let Some(end) = rest[start..].find('}') {
            let name = &rest[start + 1..start + end];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                names.push(&template[offset + start + 1..offset + start + end]);
            }
            let advance = start + end + 1;
            rest = &rest[advance..];
            offset += advance;
        } else {
            break;
        }
    }
    names
}

/// Fill `template`'s `{name}` placeholders from `repl`, leaving any name
/// absent from `repl` untouched.
fn substitute(template: &str, repl: &BTreeMap<&'static str, String>) -> String {
    let mut out = template.to_string();
    for (name, value) in repl {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Fill `blueprint.schedule_template`'s placeholders from `values` (already
/// slot-validated by [`fill_blueprint`]). Ports upstream `_resolve_schedule`
/// (`blueprint_catalog.py:689-744`) 1:1 except the day-of-week remap — see
/// [`WEEKDAY_PRESETS`]/[`DAY_TO_DOW`].
fn resolve_schedule(
    blueprint: &AutomationBlueprint,
    values: &BTreeMap<&'static str, String>,
) -> Result<String, BlueprintFillError> {
    let sched = blueprint.schedule_template;

    // A free-text `schedule` slot passes through verbatim (full flexibility).
    if let Some(free) = values.get("schedule")
        && !free.is_empty()
    {
        return Ok(free.clone());
    }

    let mut repl: BTreeMap<&'static str, String> = BTreeMap::new();

    if sched.contains("{minute}") || sched.contains("{hour}") {
        let time_val = values
            .get("time")
            .map(String::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| BlueprintFillError::BadTime {
                value: String::new(),
            })?;
        let (hour, minute) = parse_hh_mm(time_val).ok_or_else(|| BlueprintFillError::BadTime {
            value: time_val.to_string(),
        })?;
        repl.insert("hour", hour.to_string());
        repl.insert("minute", minute.to_string());
    }

    if sched.contains("{dow}") {
        if let Some(preset) = values.get("recurrence") {
            let dow = WEEKDAY_PRESETS
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(preset))
                .map(|(_, v)| (*v).to_string())
                .ok_or_else(|| BlueprintFillError::UnknownRecurrence {
                    value: preset.clone(),
                    options: WEEKDAY_PRESETS
                        .iter()
                        .map(|(k, _)| *k)
                        .collect::<Vec<_>>()
                        .join(", "),
                })?;
            repl.insert("dow", dow);
        } else if let Some(day) = values.get("day") {
            let dow = DAY_TO_DOW
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(day))
                .map(|(_, v)| (*v).to_string())
                .ok_or_else(|| BlueprintFillError::UnknownDay { value: day.clone() })?;
            repl.insert("dow", dow);
        } else {
            repl.insert("dow", "*".to_string());
        }
    }

    if sched.contains("{interval_min}") {
        let iv = values.get("interval_min").map(String::as_str).unwrap_or("");
        let n: i64 = iv.trim().parse().map_err(|_| BlueprintFillError::BadInterval {
            value: iv.to_string(),
        })?;
        if n <= 0 {
            return Err(BlueprintFillError::BadInterval {
                value: iv.to_string(),
            });
        }
        repl.insert("interval_min", n.to_string());
    }

    // Any remaining {name} placeholder is filled verbatim from validated
    // slot values — enum options have already been checked in
    // `fill_blueprint`, so these are safe to interpolate.
    for name in placeholder_names(sched) {
        if !repl.contains_key(name)
            && let Some(v) = values.get(name)
        {
            repl.insert(name, v.clone());
        }
    }

    Ok(substitute(sched, &repl))
}

/// Validate `values` against `blueprint`'s slots and return a
/// [`FilledBlueprint`] ready for [`ironhermes_cron::JobStore::add_job`].
/// Missing required (non-optional) slots error naming the slot; a slot
/// marked `optional` supplied blank or omitted is excluded from the
/// substitution map entirely (never coerced to an empty-string value),
/// matching upstream semantics. Never indexes into a slot value by byte
/// offset — whole `String`s are substituted.
pub fn fill_blueprint(
    blueprint: &AutomationBlueprint,
    values: &BTreeMap<String, String>,
) -> Result<FilledBlueprint, BlueprintFillError> {
    let mut resolved: BTreeMap<&'static str, String> = BTreeMap::new();

    for slot in blueprint.slots {
        let raw = values
            .get(slot.name)
            .cloned()
            .or_else(|| slot.default.map(str::to_string));

        let raw = match raw {
            Some(v) if !v.is_empty() => v,
            _ => {
                if slot.optional {
                    continue;
                }
                return Err(BlueprintFillError::MissingRequired {
                    slot: slot.name,
                    label: slot.label,
                });
            }
        };

        if slot.slot_type == SlotType::Enum
            && slot.strict
            && !slot.options.is_empty()
            && !slot.options.contains(&raw.as_str())
        {
            return Err(BlueprintFillError::UnknownEnumOption {
                label: slot.label,
                value: raw,
                options: slot.options.join(", "),
            });
        }

        resolved.insert(slot.name, raw);
    }

    let schedule_expr = resolve_schedule(blueprint, &resolved)?;
    let prompt = substitute(blueprint.prompt_template, &resolved);
    let deliver = resolved
        .get("deliver")
        .cloned()
        .unwrap_or_else(|| blueprint.deliver_default.to_string());

    Ok(FilledBlueprint {
        name: blueprint.title.to_string(),
        schedule_expr,
        prompt,
        deliver,
        skills: blueprint.skills.iter().map(|s| s.to_string()).collect(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test-only stand-in for `ironhermes_cron::parser::parse_schedule`'s
/// Cron-branch validation. This module cannot depend on `ironhermes-cron`
/// (that circular dependency is exactly why this module moved to
/// `ironhermes-core` — see the module doc above), so these tests validate
/// the produced `schedule_expr` the same way `parse_schedule` itself does:
/// normalise the 5-field cron string to 6-field (prepend a `"0 "` seconds
/// field) and hand it to the same `cron` crate. `parse_schedule` remains the
/// single production-path validator; this is a test-only mirror of its
/// Cron-branch logic (`ironhermes-cron/src/parser.rs`), added as a
/// `[dev-dependencies]`-only use of the `cron` crate.
#[cfg(test)]
fn assert_parseable_cron_expr(expr: &str) -> Result<(), String> {
    use std::str::FromStr;
    cron::Schedule::from_str(&format!("0 {expr}"))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod blueprint_catalog_tests {
    use super::*;

    fn morning_brief() -> &'static AutomationBlueprint {
        find_blueprint("morning-brief").expect("morning-brief must be in CATALOG")
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn catalog_is_non_empty_and_keys_are_unique() {
        let cat = catalog();
        assert!(!cat.is_empty(), "catalog() must return at least one entry");
        let mut keys: Vec<&str> = cat.iter().map(|bp| bp.key).collect();
        let unique_before = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), unique_before, "no two entries may share a key");
    }

    #[test]
    fn fill_morning_brief_default_time_yields_eight_am() {
        let bp = morning_brief();
        let filled = fill_blueprint(bp, &values(&[("time", "08:00")])).expect("fill");
        assert_eq!(filled.schedule_expr, "0 8 * * *");
        assert!(assert_parseable_cron_expr(&filled.schedule_expr).is_ok());
    }

    #[test]
    fn fill_time_boundary_midnight_and_last_minute() {
        let bp = morning_brief();
        let midnight = fill_blueprint(bp, &values(&[("time", "00:00")])).expect("fill midnight");
        assert_eq!(midnight.schedule_expr, "0 0 * * *");
        let last_minute = fill_blueprint(bp, &values(&[("time", "23:59")])).expect("fill 23:59");
        assert_eq!(last_minute.schedule_expr, "59 23 * * *");
    }

    #[test]
    fn fill_rejects_bad_time() {
        let bp = morning_brief();
        for bad in ["24:00", "8", "08:60", ""] {
            let result = fill_blueprint(bp, &values(&[("time", bad)]));
            assert!(result.is_err(), "time {bad:?} must be rejected");
        }
    }

    #[test]
    fn fill_rejects_missing_required_slot() {
        // morning-brief's own slots (`time`, `deliver`) both carry non-empty
        // defaults, so omitting them from `values` always resolves via the
        // default (matching upstream `values.get(name, default)` semantics)
        // and can never exercise this path — a stub slot with NO default is
        // needed to prove a genuinely-missing required value is rejected.
        const SLOTS: &[BlueprintSlot] = &[BlueprintSlot {
            name: "what",
            slot_type: SlotType::Text,
            label: "Remind me to…",
            default: None,
            options: &[],
            optional: false,
            help: None,
            strict: true,
        }];
        let bp = AutomationBlueprint {
            key: "stub-required",
            title: "Stub",
            description: "stub",
            category: "general",
            schedule_template: "0 8 * * *",
            prompt_template: "Remind the user: {what}",
            slots: SLOTS,
            deliver_default: "origin",
            skills: &[],
            tags: &[],
        };
        let err = fill_blueprint(&bp, &values(&[])).unwrap_err();
        match err {
            BlueprintFillError::MissingRequired { slot, .. } => assert_eq!(slot, "what"),
            other => panic!("expected MissingRequired, got {other:?}"),
        }
    }

    #[test]
    fn fill_omits_blank_optional_slot() {
        const SLOTS: &[BlueprintSlot] = &[
            BlueprintSlot {
                name: "time",
                slot_type: SlotType::Time,
                label: "What time?",
                default: Some("08:00"),
                options: &[],
                optional: false,
                help: None,
                strict: true,
            },
            BlueprintSlot {
                name: "extra",
                slot_type: SlotType::Text,
                label: "Extra note",
                default: None,
                options: &[],
                optional: true,
                help: None,
                strict: true,
            },
        ];
        let bp = AutomationBlueprint {
            key: "stub-optional",
            title: "Stub",
            description: "stub",
            category: "general",
            schedule_template: "{minute} {hour} * * *",
            prompt_template: "Base prompt. Extra: {extra}",
            slots: SLOTS,
            deliver_default: "origin",
            skills: &[],
            tags: &[],
        };
        let filled = fill_blueprint(&bp, &values(&[("time", "08:00"), ("extra", "")])).expect("fill");
        assert!(
            !filled.prompt.contains("Extra:") || filled.prompt.contains("Extra: {extra}"),
            "a blank optional slot must not be substituted as empty string: {:?}",
            filled.prompt
        );
    }

    #[test]
    fn fill_preserves_multibyte_slot_value() {
        let bp = AutomationBlueprint {
            key: "stub-multibyte",
            title: "Stub",
            description: "stub",
            category: "general",
            schedule_template: "{minute} {hour} * * *",
            prompt_template: "Topic: {topic}",
            slots: &[
                BlueprintSlot {
                    name: "time",
                    slot_type: SlotType::Time,
                    label: "What time?",
                    default: Some("08:00"),
                    options: &[],
                    optional: false,
                    help: None,
                    strict: true,
                },
                BlueprintSlot {
                    name: "topic",
                    slot_type: SlotType::Text,
                    label: "Topic",
                    default: None,
                    options: &[],
                    optional: false,
                    help: None,
                    strict: true,
                },
            ],
            deliver_default: "origin",
            skills: &[],
            tags: &[],
        };
        let topic = "日本語トピック — café";
        let filled = fill_blueprint(&bp, &values(&[("time", "08:00"), ("topic", topic)])).expect("fill");
        assert_eq!(filled.prompt, format!("Topic: {topic}"));
    }

    #[test]
    fn find_blueprint_prefers_exact_key_over_prefix() {
        let stub_catalog = [
            AutomationBlueprint {
                key: "morning",
                title: "Morning (short key)",
                description: "stub",
                category: "general",
                schedule_template: "{minute} {hour} * * *",
                prompt_template: "stub",
                slots: &[],
                deliver_default: "origin",
                skills: &[],
                tags: &[],
            },
            AutomationBlueprint {
                key: "morning-brief",
                title: "Morning briefing (long key)",
                description: "stub",
                category: "general",
                schedule_template: "{minute} {hour} * * *",
                prompt_template: "stub",
                slots: &[],
                deliver_default: "origin",
                skills: &[],
                tags: &[],
            },
        ];
        let found = find_in_catalog(&stub_catalog, "morning").expect("exact key must resolve");
        assert_eq!(found.title, "Morning (short key)");
    }

    #[test]
    fn catalog_order_is_declaration_order() {
        let first: Vec<&str> = catalog().iter().map(|bp| bp.key).collect();
        let second: Vec<&str> = catalog().iter().map(|bp| bp.key).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn fill_rejects_enum_value_outside_options() {
        let bp = morning_brief();
        let ok = fill_blueprint(bp, &values(&[("time", "08:00"), ("deliver", "origin")]));
        assert!(ok.is_ok(), "deliver is strict:false — any value must be accepted");

        // A strict enum slot (unlike morning-brief's own non-strict `deliver`)
        // must reject an out-of-options value and name the slot label.
        const SLOTS: &[BlueprintSlot] = &[BlueprintSlot {
            name: "diet",
            slot_type: SlotType::Enum,
            label: "Diet?",
            default: None,
            options: &["no restrictions", "vegetarian", "vegan"],
            optional: false,
            help: None,
            strict: true,
        }];
        let strict_bp = AutomationBlueprint {
            key: "stub-strict-enum",
            title: "Stub",
            description: "stub",
            category: "general",
            schedule_template: "0 8 * * *",
            prompt_template: "Diet: {diet}",
            slots: SLOTS,
            deliver_default: "origin",
            skills: &[],
            tags: &[],
        };
        let err = fill_blueprint(&strict_bp, &values(&[("diet", "carnivore")])).unwrap_err();
        match err {
            BlueprintFillError::UnknownEnumOption { label, .. } => assert_eq!(label, "Diet?"),
            other => panic!("expected UnknownEnumOption, got {other:?}"),
        }
    }

    #[test]
    fn fill_accepts_every_weekday_preset() {
        const SLOTS: &[BlueprintSlot] = &[BlueprintSlot {
            name: "recurrence",
            slot_type: SlotType::Weekdays,
            label: "Repeat on",
            default: Some("everyday"),
            options: &["everyday", "weekdays", "weekends"],
            optional: false,
            help: None,
            strict: true,
        }];
        let bp = AutomationBlueprint {
            key: "stub-weekdays",
            title: "Stub",
            description: "stub",
            category: "general",
            schedule_template: "0 8 * * {dow}",
            prompt_template: "stub",
            slots: SLOTS,
            deliver_default: "origin",
            skills: &[],
            tags: &[],
        };
        for (preset, _) in WEEKDAY_PRESETS {
            let filled = fill_blueprint(&bp, &values(&[("recurrence", preset)]))
                .unwrap_or_else(|e| panic!("preset {preset:?} must fill: {e}"));
            assert_parseable_cron_expr(&filled.schedule_expr)
                .unwrap_or_else(|e| panic!("preset {preset:?} produced unparseable schedule {:?}: {e}", filled.schedule_expr));
        }
    }
}

// ---------------------------------------------------------------------------
// Wave-0 day-of-week remap regression gate (49.5-VALIDATION.md "Wave 0
// Requirements", RESEARCH.md Pitfall 1 — the highest-risk item in this
// phase).
// ---------------------------------------------------------------------------

/// The upstream Python catalog numbers weekdays 0=Sunday..6=Saturday
/// (POSIX/vixie-cron); this workspace's `cron` crate numbers them
/// 1=Sunday..7=Saturday (`schedules.rs`'s module doc + its
/// `WEEKDAY_OPTIONS`). Most wrong values still PARSE cleanly under the new
/// numbering — `"1-5"` and `"2-6"` are both valid dow ranges, just
/// different days — so a transcription bug here produces a job that
/// silently fires on the wrong day, never a parse error or a crash. (The
/// one exception: a literal `"0"`, upstream's Sunday, hard-errors under
/// this crate's 1-7 range — a fail-fast case, not the dangerous one.)
///
/// Every assertion below compares against this module's own
/// [`EXPECTED_DOW`], transcribed directly from `schedules.rs`'s
/// `WEEKDAY_OPTIONS` — never against [`DAY_TO_DOW`]/[`WEEKDAY_PRESETS`]
/// themselves, so a future edit that drifts either production table away
/// from `WEEKDAY_OPTIONS` turns this suite red instead of silently passing
/// because the test re-reads the same wrong value it is supposed to check.
#[cfg(test)]
mod blueprint_dow_regression_tests {
    use super::*;

    /// Transcribed from
    /// `crates/iron_hermes_ui/src/components/hermes_app/screens/schedules.rs`'s
    /// `WEEKDAY_OPTIONS` (1=Sunday..7=Saturday) — the in-repo source of
    /// truth this workspace's `cron` crate actually accepts.
    const EXPECTED_DOW: &[(&str, u32)] = &[
        ("sunday", 1),
        ("monday", 2),
        ("tuesday", 3),
        ("wednesday", 4),
        ("thursday", 5),
        ("friday", 6),
        ("saturday", 7),
    ];

    fn expected(day: &str) -> u32 {
        EXPECTED_DOW
            .iter()
            .find(|(name, _)| *name == day)
            .map(|(_, v)| *v)
            .unwrap_or_else(|| panic!("EXPECTED_DOW has no entry for {day:?}"))
    }

    /// Extract the day-of-week field (5th whitespace-separated field) from
    /// a filled cron expression.
    fn dow_field(expr: &str) -> &str {
        expr.split_whitespace()
            .nth(4)
            .unwrap_or_else(|| panic!("expression {expr:?} has fewer than 5 fields"))
    }

    /// A dow field parsed as intent (integers), not string equality — a
    /// wildcard, a single value, a hyphen range, or a comma list.
    #[derive(Debug, PartialEq, Eq)]
    enum Dow {
        Wildcard,
        Single(u32),
        Range(u32, u32),
        List(Vec<u32>),
    }

    fn parse_dow(field: &str) -> Dow {
        if field == "*" {
            return Dow::Wildcard;
        }
        if let Some((lo, hi)) = field.split_once('-') {
            return Dow::Range(
                lo.parse().unwrap_or_else(|_| panic!("bad dow range lo in {field:?}")),
                hi.parse().unwrap_or_else(|_| panic!("bad dow range hi in {field:?}")),
            );
        }
        if field.contains(',') {
            let mut vals: Vec<u32> = field
                .split(',')
                .map(|v| v.parse().unwrap_or_else(|_| panic!("bad dow list value in {field:?}")))
                .collect();
            vals.sort_unstable();
            return Dow::List(vals);
        }
        Dow::Single(field.parse().unwrap_or_else(|_| panic!("bad dow value {field:?}")))
    }

    /// `DAY_TO_DOW`'s values must equal `EXPECTED_DOW` (transcribed from
    /// `WEEKDAY_OPTIONS`) for every day name — anchors the production table
    /// to the in-repo numbering source of truth, not to itself.
    #[test]
    fn day_to_dow_matches_workspace_weekday_options() {
        for (day, want) in EXPECTED_DOW {
            let raw = DAY_TO_DOW
                .iter()
                .find(|(k, _)| k == day)
                .unwrap_or_else(|| panic!("DAY_TO_DOW missing {day:?}"))
                .1;
            let got: u32 = raw.parse().unwrap_or_else(|_| panic!("DAY_TO_DOW[{day:?}] = {raw:?} is not an integer"));
            assert_eq!(got, *want, "DAY_TO_DOW[{day:?}] must equal WEEKDAY_OPTIONS's value");
        }
    }

    #[test]
    fn weekday_preset_everyday_is_wildcard() {
        let val = WEEKDAY_PRESETS
            .iter()
            .find(|(k, _)| *k == "everyday")
            .expect("WEEKDAY_PRESETS must have an 'everyday' entry")
            .1;
        assert_eq!(parse_dow(val), Dow::Wildcard);
    }

    #[test]
    fn weekday_preset_weekdays_spans_monday_through_friday() {
        let val = WEEKDAY_PRESETS
            .iter()
            .find(|(k, _)| *k == "weekdays")
            .expect("WEEKDAY_PRESETS must have a 'weekdays' entry")
            .1;
        assert_eq!(parse_dow(val), Dow::Range(expected("monday"), expected("friday")));
    }

    #[test]
    fn weekday_preset_weekends_is_sunday_and_saturday() {
        let val = WEEKDAY_PRESETS
            .iter()
            .find(|(k, _)| *k == "weekends")
            .expect("WEEKDAY_PRESETS must have a 'weekends' entry")
            .1;
        let mut want = [expected("sunday"), expected("saturday")];
        want.sort_unstable();
        assert_eq!(parse_dow(val), Dow::List(want.to_vec()));
    }

    /// Every catalog entry whose `schedule_template` carries a `{dow}`
    /// placeholder — filled with every `recurrence` preset it accepts, or
    /// every `day` name it accepts — must resolve to an expression
    /// `parse_schedule` accepts. Covers the entries the two hard-coded
    /// literals (tested separately below) deliberately bypass.
    #[test]
    fn every_preset_and_day_fills_to_a_parseable_expression() {
        let mut exercised = 0usize;
        for bp in CATALOG {
            if !bp.schedule_template.contains("{dow}") {
                continue;
            }
            if bp.slots.iter().any(|s| s.name == "recurrence") {
                for (preset, _) in WEEKDAY_PRESETS {
                    let mut values: BTreeMap<String, String> = BTreeMap::new();
                    values.insert("recurrence".to_string(), preset.to_string());
                    let filled = fill_blueprint(bp, &values)
                        .unwrap_or_else(|e| panic!("{}: recurrence={preset:?} must fill: {e}", bp.key));
                    assert_parseable_cron_expr(&filled.schedule_expr).unwrap_or_else(|e| {
                        panic!("{}: recurrence={preset:?} produced unparseable {:?}: {e}", bp.key, filled.schedule_expr)
                    });
                }
                exercised += 1;
            } else if let Some(day_slot) = bp.slots.iter().find(|s| s.name == "day") {
                // A `day` slot's real accepted values are ITS OWN curated
                // `options` (e.g. weekly-review/meal-plan/competitor-watch
                // only offer sunday/monday/friday/saturday), not every name
                // `DAY_TO_DOW` happens to know — exercising the full
                // `DAY_TO_DOW` set would reject a value the slot itself
                // never offers (a strict-enum rejection, not a dow bug).
                for day in day_slot.options {
                    let mut values: BTreeMap<String, String> = BTreeMap::new();
                    values.insert("day".to_string(), day.to_string());
                    let filled = fill_blueprint(bp, &values)
                        .unwrap_or_else(|e| panic!("{}: day={day:?} must fill: {e}", bp.key));
                    assert_parseable_cron_expr(&filled.schedule_expr).unwrap_or_else(|e| {
                        panic!("{}: day={day:?} produced unparseable {:?}: {e}", bp.key, filled.schedule_expr)
                    });
                }
                exercised += 1;
            } else {
                panic!(
                    "{}: schedule_template contains {{dow}} but no 'recurrence' or 'day' slot",
                    bp.key
                );
            }
        }
        assert!(exercised > 0, "no catalog entry with a {{dow}} placeholder was exercised");
    }

    /// `workday-start` and `hydration-move` hard-code their dow range
    /// directly in `schedule_template` (bypassing `WEEKDAY_PRESETS`/
    /// `DAY_TO_DOW` entirely) — their literal must still be Monday-Friday
    /// under this workspace's numbering, not the upstream POSIX numbering.
    #[test]
    fn hardcoded_dow_templates_use_workspace_numbering() {
        let empty: BTreeMap<String, String> = BTreeMap::new();
        for key in ["workday-start", "hydration-move"] {
            let bp = find_blueprint(key).unwrap_or_else(|| panic!("{key} must be in CATALOG"));
            let filled =
                fill_blueprint(bp, &empty).unwrap_or_else(|e| panic!("{key}: default fill must succeed: {e}"));
            let dow = dow_field(&filled.schedule_expr);
            assert_eq!(
                parse_dow(dow),
                Dow::Range(expected("monday"), expected("friday")),
                "{key}'s hard-coded dow field {dow:?} must be Monday-Friday under workspace numbering"
            );
        }
    }

    /// Every catalog entry, filled with only its own slot defaults, must
    /// resolve to an expression `parse_schedule` accepts — the same
    /// acceptance criterion task 1 states, re-asserted here as part of the
    /// regression gate.
    #[test]
    fn every_catalog_default_fill_parses() {
        let empty: BTreeMap<String, String> = BTreeMap::new();
        for bp in CATALOG {
            let filled =
                fill_blueprint(bp, &empty).unwrap_or_else(|e| panic!("{}: default fill must succeed: {e}", bp.key));
            assert_parseable_cron_expr(&filled.schedule_expr).unwrap_or_else(|e| {
                panic!("{}: default fill produced unparseable {:?}: {e}", bp.key, filled.schedule_expr)
            });
        }
    }
}
