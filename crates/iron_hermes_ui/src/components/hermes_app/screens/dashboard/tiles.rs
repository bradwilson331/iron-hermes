//! Phase 54.10 Plan 08 (D-05..D-08, D-10, D-11): the native Dashboard tiles.
//!
//! Tiles are read-only (D-05): they call only the read server fns of `dashboard_tiles_api` and
//! the kanban / schedules / gateway reads the app already has. Each tile owns its polling loop
//! (D-06), so `ScreenRouter` unmounting the Dashboard stops every loop with it; nothing here is
//! mounted at the HermesApp root. Text from sessions, tasks, cron errors and reports is rendered
//! as text nodes only (T-54.10-24).

use super::items::focus_dom;
use crate::components::hermes_app::screens::openui::{now_unix_secs, relative_time};
use crate::server::dashboard_api::{
    KanbanColumn, SessionsShow, TileSpec, UsageWindow, all_kanban_columns,
};
use crate::server::dashboard_tiles_api::{
    CronTileView, DashSessionRow, GatewayRow, KanbanTileView, ReportVersionView, UsageTileView,
    cron_summary, dashboard_report_versions, dashboard_sessions, dashboard_usage, gateway_rows,
    kanban_summary, list_dashboard_reports, report_delta,
};
use crate::server::gateway_platform_status_api::read_platform_status;
use crate::server::kanban_api::fetch_board;
use crate::server::schedules_api::get_schedules;
use crate::server::tools_config_api::ConfigScope;
use crate::state::{Screen, SessionIdContext};
use dioxus::prelude::*;
use std::future::Future;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// UI-SPEC A6: 60s for Usage and Reports, 30s for the rest.
pub(crate) fn poll_interval_ms(spec: &TileSpec) -> u32 {
    match spec {
        TileSpec::Usage { .. } | TileSpec::Reports { .. } => 60_000,
        _ => 30_000,
    }
}

/// First fetches are staggered 250ms per tile so a full board does not fire at once.
pub(crate) fn first_fetch_delay_ms(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX).saturating_mul(250)
}

/// `$` + 2 decimals from integer micro-dollars; rounds half up once, at the last step.
pub(crate) fn format_cost(micros: i64) -> String {
    let cents = (micros.max(0).saturating_add(5_000)) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

fn window_label(w: UsageWindow) -> &'static str {
    match w {
        UsageWindow::Day => "24h",
        UsageWindow::Week => "7d",
        UsageWindow::Month => "30d",
    }
}

/// `document.hidden` (wasm); a native build has no page to hide.
fn document_hidden() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

// ---------------------------------------------------------------------------
// Polling feed
// ---------------------------------------------------------------------------

/// What a tile's last fetches produced. A failed refresh keeps the last good data (STALE).
#[derive(Clone)]
pub(crate) enum FeedState<T> {
    Loading,
    Failed(String),
    Ready {
        data: T,
        at: i64,
        refresh_error: Option<String>,
    },
}

impl<T> FeedState<T> {
    pub(crate) fn frame(&self) -> FrameStatus {
        match self {
            FeedState::Loading => FrameStatus::Loading,
            FeedState::Failed(e) => FrameStatus::Failed(e.clone()),
            FeedState::Ready {
                at, refresh_error, ..
            } => FrameStatus::Ready {
                at: *at,
                stale: refresh_error.is_some(),
            },
        }
    }

    pub(crate) fn data(&self) -> Option<&T> {
        match self {
            FeedState::Ready { data, .. } => Some(data),
            _ => None,
        }
    }
}

/// Fold one fetch result into the previous state.
pub(crate) fn next_state<T>(prev: FeedState<T>, res: Result<T, String>, at: i64) -> FeedState<T> {
    match (res, prev) {
        (Ok(data), _) => FeedState::Ready {
            data,
            at,
            refresh_error: None,
        },
        (
            Err(e),
            FeedState::Ready {
                data, at: old_at, ..
            },
        ) => FeedState::Ready {
            data,
            at: old_at,
            refresh_error: Some(e),
        },
        (Err(e), _) => FeedState::Failed(e),
    }
}

pub(crate) struct Feed<T: 'static> {
    pub state: Signal<FeedState<T>>,
    pub busy: Signal<bool>,
    tick: Signal<u32>,
}

impl<T> Clone for Feed<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Feed<T> {}

impl<T> Feed<T> {
    /// Fetch now (the `↻` button and both retry buttons).
    pub(crate) fn refresh(self) {
        let mut t = self.tick;
        t += 1;
    }
}

/// One tile's polling. The `use_future` loop only WRITES `tick`; the `use_resource` READS `tick`
/// and the settings `key`, fetches, and is the only writer of `state` (RESEARCH Pitfall 8: no
/// effect reads and writes the same signal).
pub(crate) fn use_feed<K, T, Fut>(
    index: usize,
    interval_ms: u32,
    key: K,
    fetch: fn(K) -> Fut,
) -> Feed<T>
where
    K: Clone + PartialEq + 'static,
    T: Clone + 'static,
    Fut: Future<Output = Result<T, String>> + 'static,
{
    let mut state = use_signal(|| FeedState::<T>::Loading);
    let mut busy = use_signal(|| false);
    let mut tick = use_signal(|| 0u32);
    let _fetching = use_resource(use_reactive!(|key| async move {
        let _ = tick();
        let first = matches!(*state.peek(), FeedState::Loading);
        if first {
            crate::platform::timer::sleep(first_fetch_delay_ms(index)).await;
        }
        busy.set(true);
        let res = fetch(key).await;
        busy.set(false);
        let prev = state.peek().clone();
        state.set(next_state(prev, res, now_unix_secs()));
    }));
    use_future(move || async move {
        loop {
            crate::platform::timer::sleep(interval_ms).await;
            if !document_hidden() {
                tick += 1;
            }
        }
    });
    Feed { state, busy, tick }
}

// ---------------------------------------------------------------------------
// Frame: bar, loading / error / data, footer
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum FrameStatus {
    Loading,
    Failed(String),
    Ready { at: i64, stale: bool },
}

/// UI-SPEC "Tile error (first load)".
pub(crate) fn tile_load_error(title: &str, reason: &str) -> String {
    format!(
        "Could not load {title}: {}.",
        reason.trim().trim_end_matches('.')
    )
}

/// What every tile shares: the control bar (STALE, `↻`, `⚙`), the state or the data, the footer.
#[component]
pub(crate) fn TileFrame(
    title: String,
    status: FrameStatus,
    now: i64,
    busy: bool,
    has_settings: bool,
    settings_open: bool,
    gear_id: String,
    on_refresh: EventHandler<()>,
    on_gear: EventHandler<()>,
    link: Option<String>,
    on_link: EventHandler<()>,
    children: Element,
) -> Element {
    let stale = matches!(status, FrameStatus::Ready { stale: true, .. });
    let expanded = if settings_open { "true" } else { "false" };
    rsx! {
        div { class: "ih-tile__frame",
            div { class: "ih-tile__bar",
                if stale {
                    span { class: "pill amber", "STALE" }
                }
                div { class: "ih-tile__spacer" }
                button {
                    class: "btn btn--icon btn--ghost",
                    r#type: "button",
                    "aria-label": "Refresh {title}",
                    title: "Refresh {title}",
                    disabled: busy,
                    "aria-busy": if busy { "true" },
                    onclick: move |_| on_refresh.call(()),
                    "↻"
                }
                if has_settings {
                    button {
                        id: "{gear_id}",
                        class: "btn btn--icon btn--ghost",
                        r#type: "button",
                        "aria-label": "Settings for {title}",
                        title: "Settings for {title}",
                        "aria-haspopup": "dialog",
                        "aria-expanded": "{expanded}",
                        onclick: move |_| on_gear.call(()),
                        "⚙"
                    }
                }
            }
            div { class: "ih-tile__main",
                match &status {
                    FrameStatus::Loading => rsx! {
                        div { class: "ih-tile__center", "Loading…" }
                    },
                    FrameStatus::Failed(reason) => rsx! {
                        div { class: "ih-tile__fail", role: "alert",
                            span {
                                class: "ih-tile__clamp",
                                style: "color: var(--red);",
                                title: "{tile_load_error(&title, reason)}",
                                "{tile_load_error(&title, reason)}"
                            }
                            button {
                                class: "btn btn--sm btn--ghost",
                                r#type: "button",
                                onclick: move |_| on_refresh.call(()),
                                "Retry load"
                            }
                        }
                    },
                    FrameStatus::Ready { .. } => rsx! {
                        {children}
                    },
                }
            }
            div { class: "ih-tile__foot",
                match &status {
                    FrameStatus::Ready { at, stale: false } => rsx! {
                        span { class: "ih-tile__meta", "Updated {relative_time(now, *at)}" }
                    },
                    FrameStatus::Ready { at, stale: true } => rsx! {
                        span { class: "ih-tile__meta",
                            "Could not refresh. Showing data from {relative_time(now, *at)}."
                        }
                        button {
                            class: "btn btn--sm btn--ghost",
                            r#type: "button",
                            onclick: move |_| on_refresh.call(()),
                            "Retry refresh"
                        }
                    },
                    _ => rsx! {},
                }
                div { class: "ih-tile__spacer" }
                if let Some(label) = link {
                    button {
                        class: "btn btn--sm btn--ghost",
                        r#type: "button",
                        onclick: move |_| on_link.call(()),
                        "{label}"
                    }
                }
            }
        }
    }
}

/// Everything a tile needs from the item that holds it.
#[derive(Clone, PartialEq)]
pub(crate) struct TileCtx {
    pub item_id: String,
    pub index: usize,
    pub now: i64,
    pub selected: bool,
    pub read_only: bool,
}

fn has_settings(spec: &TileSpec) -> bool {
    !matches!(spec, TileSpec::Cron {} | TileSpec::Gateway {})
}

/// The frame plus its `⚙` popover. Keys typed inside a tile belong to the tile: only Esc with
/// the popover closed is left to bubble so the item still deselects.
#[component]
fn TileShell(
    title: String,
    spec: TileSpec,
    ctx: TileCtx,
    status: FrameStatus,
    busy: bool,
    on_refresh: EventHandler<()>,
    on_settings: EventHandler<TileSpec>,
    link: Option<(String, Screen)>,
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);
    let mut screen = use_context::<Signal<Screen>>();
    // A press anywhere else (another item or the empty canvas) deselects this item: close.
    let selected = ctx.selected;
    use_effect(use_reactive!(|selected| {
        if !selected {
            open.set(false);
        }
    }));
    let gear_id = format!("ih-tile-gear-{}", ctx.item_id);
    let settings = has_settings(&spec) && !ctx.read_only;
    let is_open = *open.read() && settings;
    let (link_label, link_to) = match link {
        Some((l, s)) => (Some(l), Some(s)),
        None => (None, None),
    };
    let esc_gear = gear_id.clone();
    rsx! {
        div {
            class: "ih-tile",
            onkeydown: move |e| {
                if e.key().to_string() == "Escape" {
                    if *open.peek() {
                        e.stop_propagation();
                        open.set(false);
                        focus_dom(&esc_gear);
                    }
                } else {
                    e.stop_propagation();
                }
            },
            TileFrame {
                title: title.clone(),
                status,
                now: ctx.now,
                busy,
                has_settings: settings,
                settings_open: is_open,
                gear_id: gear_id.clone(),
                on_refresh,
                on_gear: move |_| {
                    let v = *open.peek();
                    open.set(!v);
                },
                link: link_label,
                on_link: move |_| {
                    if let Some(s) = link_to {
                        screen.set(s);
                    }
                },
                {children}
            }
            if is_open {
                SettingsPopover {
                    title: title.clone(),
                    spec: spec.clone(),
                    on_settings,
                    on_done: move |_| {
                        open.set(false);
                        focus_dom(&gear_id);
                    },
                }
            }
        }
    }
}

fn parse_window(v: &str) -> UsageWindow {
    match v {
        "24h" => UsageWindow::Day,
        "30d" => UsageWindow::Month,
        _ => UsageWindow::Week,
    }
}

/// UI-SPEC A6 settings: typed fields saved with the board item; a change applies at once.
#[component]
fn SettingsPopover(
    title: String,
    spec: TileSpec,
    on_settings: EventHandler<TileSpec>,
    on_done: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "panel ih-tile__settings",
            role: "dialog",
            "aria-label": "Settings for {title}",
            match spec {
                TileSpec::Usage { window } => rsx! {
                    div { class: "field-row",
                        label { class: "field-label", r#for: "ih-tile-set-window", "Window" }
                        select {
                            id: "ih-tile-set-window",
                            onchange: move |e| {
                                on_settings.call(TileSpec::Usage { window: parse_window(&e.value()) })
                            },
                            for w in [UsageWindow::Day, UsageWindow::Week, UsageWindow::Month] {
                                option {
                                    value: "{window_label(w)}",
                                    selected: w == window,
                                    "{window_label(w)}"
                                }
                            }
                        }
                    }
                },
                TileSpec::Sessions { show } => rsx! {
                    div { class: "field-row",
                        label { class: "field-label", r#for: "ih-tile-set-show", "Show" }
                        select {
                            id: "ih-tile-set-show",
                            onchange: move |e| {
                                let show = if e.value() == "active" {
                                    SessionsShow::Active
                                } else {
                                    SessionsShow::Recent
                                };
                                on_settings.call(TileSpec::Sessions { show });
                            },
                            option {
                                value: "recent",
                                selected: show == SessionsShow::Recent,
                                "Recent"
                            }
                            option {
                                value: "active",
                                selected: show == SessionsShow::Active,
                                "Active"
                            }
                        }
                    }
                },
                TileSpec::Kanban { columns } => rsx! {
                    div { class: "field-row",
                        span { class: "field-label", "Columns" }
                        for (c , on , next) in column_boxes(&columns) {
                            label { class: "ih-tile__check",
                                input {
                                    r#type: "checkbox",
                                    checked: on,
                                    onchange: move |_| {
                                        on_settings.call(TileSpec::Kanban { columns: next.clone() })
                                    },
                                }
                                "{column_label(c)}"
                            }
                        }
                    }
                },
                TileSpec::Reports { report } => rsx! {
                    ReportNameSelect { current: report, on_settings }
                },
                _ => rsx! {},
            }
            button {
                class: "btn btn--sm",
                r#type: "button",
                onclick: move |_| on_done.call(()),
                "Done"
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared body pieces
// ---------------------------------------------------------------------------

#[component]
fn TileEmpty(heading: String, body: String) -> Element {
    rsx! {
        div { class: "ih-tile__empty",
            div { class: "ih-tile__heading", "{heading}" }
            div { class: "ih-tile__meta", "{body}" }
        }
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Bar width in percent, scaled to the largest row.
fn bar_pct(value: i64, max: i64) -> i64 {
    if max <= 0 {
        0
    } else {
        (value.max(0).saturating_mul(100) / max).clamp(0, 100)
    }
}

#[component]
fn UsageBody(view: UsageTileView) -> Element {
    let by_cost = view.cost_recorded;
    let value_of = |cost: i64, tokens: i64| if by_cost { cost } else { tokens };
    let max = view
        .rows
        .iter()
        .map(|r| value_of(r.cost_micros, r.tokens))
        .max()
        .unwrap_or(0);
    let hero = if by_cost {
        format_cost(view.total_cost_micros)
    } else {
        view.total_tokens.to_string()
    };
    let sub = if by_cost {
        format!(
            "{} tokens · {}",
            view.total_tokens,
            window_label(view.window)
        )
    } else {
        "Cost not recorded".to_string()
    };
    rsx! {
        if view.rows.is_empty() {
            TileEmpty {
                heading: "No usage in this window",
                body: "Token use appears here after the agent runs a turn.",
            }
        } else {
            div { class: "ih-tile__hero", "{hero}" }
            div { class: "ih-tile__sub", "{sub}" }
            for r in view.rows.iter() {
                div { class: "ih-tile__usage-row",
                    div { class: "ih-tile__usage-head",
                        span { class: "ih-tile__trunc", title: "{r.model}", "{r.model}" }
                        span { class: "ih-tile__meta", "{r.provider}" }
                        span { class: "ih-tile__value",
                            if by_cost {
                                "{format_cost(r.cost_micros)}"
                            } else {
                                "{r.tokens}"
                            }
                        }
                    }
                    div { class: "ih-tile__track",
                        div {
                            class: "ih-tile__fill",
                            style: "width: {bar_pct(value_of(r.cost_micros, r.tokens), max)}%;",
                        }
                    }
                }
            }
        }
        if view.failed_calls > 0 {
            div { class: "ih-tile__meta", "{plural(view.failed_calls, \"failed call\", \"failed calls\")}" }
        }
    }
}

async fn load_usage(window: UsageWindow) -> Result<UsageTileView, String> {
    dashboard_usage(window).await.map_err(|e| e.to_string())
}

#[component]
fn UsageTile(ctx: TileCtx, window: UsageWindow, on_settings: EventHandler<TileSpec>) -> Element {
    let spec = TileSpec::Usage { window };
    let feed = use_feed(ctx.index, poll_interval_ms(&spec), window, load_usage);
    let st = feed.state.read().clone();
    let busy = *feed.busy.read();
    rsx! {
        TileShell {
            title: spec.title(),
            spec,
            ctx,
            status: st.frame(),
            busy,
            on_refresh: move |_| feed.refresh(),
            on_settings,
            link: None,
            if let Some(view) = st.data() {
                UsageBody { view: view.clone() }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// UI-SPEC A6: up to 6 rows; the rest collapse into `+{n} more`.
const LIST_ROWS_MAX: usize = 6;

fn more_line(total: usize, shown: usize) -> Option<String> {
    (total > shown).then(|| format!("+{} more", total - shown))
}

#[component]
fn SessionsBody(
    rows: Vec<DashSessionRow>,
    show: SessionsShow,
    now: i64,
    on_open: EventHandler<String>,
) -> Element {
    let rows: Vec<&DashSessionRow> = rows
        .iter()
        .filter(|r| show == SessionsShow::Recent || r.active)
        .collect();
    let more = more_line(rows.len(), LIST_ROWS_MAX);
    rsx! {
        if rows.is_empty() {
            TileEmpty { heading: "No sessions yet", body: "Start a chat and it shows up here." }
        }
        for r in rows.iter().take(LIST_ROWS_MAX) {
            button {
                class: "row ih-tile__row",
                r#type: "button",
                onclick: {
                    let id = r.id.clone();
                    move |_| on_open.call(id.clone())
                },
                div { class: "row-main",
                    span {
                        class: "ih-tile__trunc",
                        title: "{session_title(r)}",
                        "{session_title(r)}"
                    }
                    span { class: "ih-tile__meta", "{relative_time(now, r.started_at as i64)}" }
                }
                if r.active {
                    span { class: "pill green", "ACTIVE" }
                }
            }
        }
        if let Some(m) = more {
            div { class: "ih-tile__meta", "{m}" }
        }
    }
}

fn session_title(r: &DashSessionRow) -> String {
    r.title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("Untitled session")
        .to_string()
}

async fn load_sessions(_show: SessionsShow) -> Result<Vec<DashSessionRow>, String> {
    dashboard_sessions(20).await.map_err(|e| e.to_string())
}

#[component]
fn SessionsTile(ctx: TileCtx, show: SessionsShow, on_settings: EventHandler<TileSpec>) -> Element {
    let spec = TileSpec::Sessions { show };
    let feed = use_feed(ctx.index, poll_interval_ms(&spec), show, load_sessions);
    let mut screen = use_context::<Signal<Screen>>();
    let mut session_id = use_context::<SessionIdContext>().0;
    let st = feed.state.read().clone();
    let busy = *feed.busy.read();
    let now = ctx.now;
    rsx! {
        TileShell {
            title: spec.title(),
            spec,
            ctx,
            status: st.frame(),
            busy,
            on_refresh: move |_| feed.refresh(),
            on_settings,
            link: Some(("Open Sessions →".to_string(), Screen::Sessions)),
            if let Some(rows) = st.data() {
                SessionsBody {
                    rows: rows.clone(),
                    show,
                    now,
                    on_open: move |sid: String| {
                        session_id.set(sid);
                        screen.set(Screen::Chat);
                    },
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Kanban
// ---------------------------------------------------------------------------

fn column_label(c: KanbanColumn) -> &'static str {
    match c {
        KanbanColumn::Triage => "Triage",
        KanbanColumn::Todo => "Todo",
        KanbanColumn::Ready => "Ready",
        KanbanColumn::Running => "Running",
        KanbanColumn::Blocked => "Blocked",
        KanbanColumn::Done => "Done",
    }
}

/// The columns after toggling `c`; at least one stays on (an empty list would mean "all").
pub(crate) fn toggle_column(cols: &[KanbanColumn], c: KanbanColumn) -> Vec<KanbanColumn> {
    let on = cols.contains(&c);
    if on && cols.len() == 1 {
        return cols.to_vec();
    }
    all_kanban_columns()
        .into_iter()
        .filter(|k| if *k == c { !on } else { cols.contains(k) })
        .collect()
}

/// `(column, checked, columns after toggling it)` for every column; an empty setting means all.
fn column_boxes(cols: &[KanbanColumn]) -> Vec<(KanbanColumn, bool, Vec<KanbanColumn>)> {
    let eff = if cols.is_empty() {
        all_kanban_columns()
    } else {
        cols.to_vec()
    };
    all_kanban_columns()
        .into_iter()
        .map(|c| (c, eff.contains(&c), toggle_column(&eff, c)))
        .collect()
}

/// Attention rows shown under the chips.
const BLOCKED_ROWS_MAX: usize = 3;

#[component]
fn KanbanBody(view: KanbanTileView) -> Element {
    let counted: usize = view.columns.iter().map(|(_, n)| n).sum();
    let more = more_line(view.blocked.len(), BLOCKED_ROWS_MAX);
    rsx! {
        if counted == 0 && view.blocked.is_empty() {
            TileEmpty {
                heading: "No tasks on this board",
                body: "Create a task on the Kanban screen.",
            }
        } else {
            div { class: "ih-tile__hero", "{view.open_total}" }
            div { class: "ih-tile__sub", "open tasks" }
            div { class: "ih-tile__chips",
                for (c , n) in view.columns.iter() {
                    span { class: "ih-tile__chip",
                        span { class: "ih-tile__label", "{column_label(*c)}" }
                        span { class: "ih-tile__heading", "{n}" }
                    }
                }
            }
            for t in view.blocked.iter().take(BLOCKED_ROWS_MAX) {
                div { class: "ih-tile__attn",
                    span { class: "pill amber", "BLOCKED" }
                    span { class: "ih-tile__trunc", title: "{t}", "{t}" }
                }
            }
            if let Some(m) = more {
                div { class: "ih-tile__meta", "{m}" }
            }
        }
    }
}

async fn load_kanban(columns: Vec<KanbanColumn>) -> Result<KanbanTileView, String> {
    let rows = fetch_board(None, false).await.map_err(|e| e.to_string())?;
    Ok(kanban_summary(&rows, &columns))
}

#[component]
fn KanbanTile(
    ctx: TileCtx,
    columns: Vec<KanbanColumn>,
    on_settings: EventHandler<TileSpec>,
) -> Element {
    let spec = TileSpec::Kanban {
        columns: columns.clone(),
    };
    let feed = use_feed(ctx.index, poll_interval_ms(&spec), columns, load_kanban);
    let st = feed.state.read().clone();
    let busy = *feed.busy.read();
    rsx! {
        TileShell {
            title: spec.title(),
            spec,
            ctx,
            status: st.frame(),
            busy,
            on_refresh: move |_| feed.refresh(),
            on_settings,
            link: Some(("Open Kanban →".to_string(), Screen::Kanban)),
            if let Some(view) = st.data() {
                KanbanBody { view: view.clone() }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cron
// ---------------------------------------------------------------------------

/// `in 5 min` / `in 3 h` / `in 2 d` for a future instant `secs` away.
pub(crate) fn in_duration(secs: i64) -> String {
    match secs {
        i64::MIN..=59 => "in <1 min".to_string(),
        60..=3599 => format!("in {} min", secs / 60),
        3600..=86_399 => format!("in {} h", secs / 3600),
        _ => format!("in {} d", secs / 86_400),
    }
}

fn unix_of(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.timestamp())
}

#[component]
fn CronBody(view: CronTileView, now: i64) -> Element {
    let (status, class) = match view.last.as_ref().map(|l| l.failed) {
        None => ("NEVER RUN", "pill ended"),
        Some(Some(true)) => ("FAILED", "pill red"),
        Some(_) => ("OK", "pill green"),
    };
    rsx! {
        if view.next.is_empty() && view.last.is_none() {
            TileEmpty {
                heading: "No scheduled jobs",
                body: "Create one on the Schedules screen.",
            }
        } else {
            div { class: "ih-tile__label", "Next runs" }
            if view.next.is_empty() {
                div { class: "ih-tile__meta", "None scheduled." }
            }
            for (name , raw) in view.next.iter() {
                div { class: "ih-tile__split",
                    span { class: "ih-tile__trunc", title: "{name}", "{name}" }
                    span { class: "ih-tile__meta",
                        "{unix_of(raw).map(|t| in_duration(t - now)).unwrap_or_default()}"
                    }
                }
            }
            div { class: "ih-tile__label", "Last run" }
            div { class: "ih-tile__split",
                if let Some(l) = view.last.as_ref() {
                    span { class: "ih-tile__trunc", title: "{l.name}", "{l.name}" }
                }
                span { class: "{class}", "{status}" }
                if let Some(l) = view.last.as_ref() {
                    span { class: "ih-tile__meta",
                        "{unix_of(&l.at_raw).map(|t| relative_time(now, t)).or_else(|| l.at_display.clone()).unwrap_or_default()}"
                    }
                }
            }
            if let Some(err) = view.last.as_ref().and_then(|l| l.error.as_ref()) {
                span {
                    class: "ih-tile__clamp",
                    style: "color: var(--red);",
                    title: "{err}",
                    "{err}"
                }
            }
        }
    }
}

async fn load_cron(_: ()) -> Result<CronTileView, String> {
    let view = get_schedules(Some("default".to_string()))
        .await
        .map_err(|e| e.to_string())?;
    Ok(cron_summary(&view.rows))
}

#[component]
fn CronTile(ctx: TileCtx, on_settings: EventHandler<TileSpec>) -> Element {
    let spec = TileSpec::Cron {};
    let feed = use_feed(ctx.index, poll_interval_ms(&spec), (), load_cron);
    let st = feed.state.read().clone();
    let busy = *feed.busy.read();
    let now = ctx.now;
    rsx! {
        TileShell {
            title: spec.title(),
            spec,
            ctx,
            status: st.frame(),
            busy,
            on_refresh: move |_| feed.refresh(),
            on_settings,
            link: Some(("Open Schedules →".to_string(), Screen::Schedules)),
            if let Some(view) = st.data() {
                CronBody { view: view.clone(), now }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Gateway health (operator resolution 3: CONNECTED / DISCONNECTED only)
// ---------------------------------------------------------------------------

#[component]
fn GatewayBody(rows: Vec<GatewayRow>) -> Element {
    let more = more_line(rows.len(), LIST_ROWS_MAX);
    rsx! {
        if rows.is_empty() {
            TileEmpty {
                heading: "No platforms configured",
                body: "Connect one on the Gateway screen.",
            }
        }
        for r in rows.iter().take(LIST_ROWS_MAX) {
            div { class: "ih-tile__split",
                span { class: "ih-tile__trunc", title: "{r.platform}", "{r.platform}" }
                if r.connected {
                    span { class: "pill green", "CONNECTED" }
                } else {
                    span { class: "pill red", "DISCONNECTED" }
                }
            }
        }
        if let Some(m) = more {
            div { class: "ih-tile__meta", "{m}" }
        }
    }
}

async fn load_gateway(_: ()) -> Result<Vec<GatewayRow>, String> {
    let map = read_platform_status(ConfigScope::Root)
        .await
        .map_err(|e| e.to_string())?;
    Ok(gateway_rows(&map))
}

#[component]
fn GatewayTile(ctx: TileCtx, on_settings: EventHandler<TileSpec>) -> Element {
    let spec = TileSpec::Gateway {};
    let feed = use_feed(ctx.index, poll_interval_ms(&spec), (), load_gateway);
    let st = feed.state.read().clone();
    let busy = *feed.busy.read();
    rsx! {
        TileShell {
            title: spec.title(),
            spec,
            ctx,
            status: st.frame(),
            busy,
            on_refresh: move |_| feed.refresh(),
            on_settings,
            link: Some(("Open Gateway →".to_string(), Screen::Gateway)),
            if let Some(rows) = st.data() {
                GatewayBody { rows: rows.clone() }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reports (D-10, D-11)
// ---------------------------------------------------------------------------

const SEVERITIES: [&str; 5] = ["critical", "high", "medium", "low", "info"];

/// Severity pills for one version, highest first, zero counts omitted. With a delta each reads
/// `3 high (+1)` or `(no change)`; without one (v1, or a version being viewed) just `3 high`.
/// A rising high/critical count is red, a falling one green, anything else neutral.
pub(crate) fn severity_pills(
    counts: &[u32; 5],
    delta: Option<[i64; 5]>,
) -> Vec<(String, &'static str)> {
    SEVERITIES
        .iter()
        .enumerate()
        .filter(|(i, _)| counts[*i] > 0)
        .map(|(i, sev)| {
            let n = counts[i];
            let Some(d) = delta.map(|d| d[i]) else {
                return (format!("{n} {sev}"), "");
            };
            let text = if d == 0 {
                format!("{n} {sev} (no change)")
            } else {
                format!("{n} {sev} ({d:+})")
            };
            let class = match (i <= 1, d.signum()) {
                (true, 1) => "red",
                (true, -1) => "green",
                _ => "",
            };
            (text, class)
        })
        .collect()
}

/// `3 high, 1 medium`, or `no findings` when every count is zero.
fn counts_summary(counts: &[u32; 5]) -> String {
    let parts: Vec<String> = SEVERITIES
        .iter()
        .enumerate()
        .filter(|(i, _)| counts[*i] > 0)
        .map(|(i, sev)| format!("{} {sev}", counts[i]))
        .collect();
    if parts.is_empty() {
        "no findings".to_string()
    } else {
        parts.join(", ")
    }
}

/// The version the tile shows: the one being viewed while it is still in the list, else the
/// latest. Polling replaces the list, never the viewed version (D-11).
pub(crate) fn select_shown_version<'a>(
    viewing: Option<&str>,
    list: &'a [ReportVersionView],
) -> Option<&'a ReportVersionView> {
    viewing
        .and_then(|id| list.iter().find(|v| v.id == id))
        .or_else(|| list.first())
}

fn abs_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

fn finding_class(sev: &str) -> &'static str {
    match sev {
        "critical" | "high" => "pill red",
        "medium" => "pill amber",
        _ => "pill",
    }
}

#[component]
fn ReportsBody(
    name: Option<String>,
    versions: Vec<ReportVersionView>,
    viewing: Option<String>,
    now: i64,
    versions_open: bool,
    on_toggle: EventHandler<()>,
    on_pick: EventHandler<String>,
    on_back: EventHandler<()>,
) -> Element {
    let Some(name) = name else {
        return rsx! {
            TileEmpty {
                heading: "Choose a report",
                body: "Open this tile's settings and pick a report name.",
            }
        };
    };
    let Some(shown) = select_shown_version(viewing.as_deref(), &versions) else {
        return rsx! {
            TileEmpty {
                heading: "No reports published yet",
                body: "A red-team run or skill publishes a report with the report_publish tool. It appears here.",
            }
        };
    };
    let total = versions.len();
    let idx = versions.iter().position(|v| v.id == shown.id).unwrap_or(0);
    let n = total - idx;
    // The delta is latest against previous; a version being viewed shows counts only.
    let delta = if idx == 0 {
        report_delta(&versions)
    } else {
        None
    };
    let pills = severity_pills(&shown.counts, delta);
    let open_label = if versions_open {
        "Versions ▴"
    } else {
        "Versions ▾"
    };
    rsx! {
        if idx > 0 {
            div { class: "ih-tile__banner", role: "status",
                span { "Viewing v{n} of {total}, {abs_time(shown.published_at_ms)}." }
                button {
                    class: "btn btn--sm btn--ghost",
                    r#type: "button",
                    onclick: move |_| on_back.call(()),
                    "Back to latest"
                }
            }
        }
        div { class: "ih-tile__pills",
            for (text , class) in pills {
                span { class: "pill {class}", "{text}" }
            }
        }
        if !shown.title.is_empty() {
            div { class: "ih-tile__heading ih-tile__trunc", title: "{shown.title}", "{shown.title}" }
        }
        div { class: "ih-tile__meta",
            "v{n} · {relative_time(now, shown.published_at_ms / 1000)}"
        }
        if !shown.summary.is_empty() {
            div { class: "ih-tile__clamp", title: "{shown.summary}", "{shown.summary}" }
        }
        if !shown.findings.is_empty() {
            div { class: "ih-tile__findings",
                for (sev , title) in shown.findings.iter() {
                    div { class: "ih-tile__attn",
                        span { class: "{finding_class(sev)}", "{sev.to_uppercase()}" }
                        span { class: "ih-tile__trunc", title: "{title}", "{title}" }
                    }
                }
            }
        }
        div {
            button {
                class: "btn btn--sm btn--ghost",
                r#type: "button",
                "aria-expanded": if versions_open { "true" } else { "false" },
                onclick: move |_| on_toggle.call(()),
                "{open_label}"
            }
        }
        if versions_open {
            VersionsList { name, viewing_id: shown.id.clone(), on_pick }
        }
    }
}

/// The in-tile version list, newest first. It fetches on open (D-11) so it can fail on its own.
#[component]
fn VersionsList(name: String, viewing_id: String, on_pick: EventHandler<String>) -> Element {
    let mut rev = use_signal(|| 0u32);
    let fetched = use_resource(use_reactive!(|name| async move {
        let _ = rev();
        dashboard_report_versions(name)
            .await
            .map_err(|e| e.to_string())
    }));
    let state = fetched.read().clone();
    rsx! {
        div { class: "ih-tile__versions",
            match state {
                None => rsx! {
                    div { class: "ih-tile__meta", "Loading…" }
                },
                Some(Err(e)) => rsx! {
                    div { class: "ih-tile__fail", role: "alert",
                        span { class: "ih-tile__clamp", style: "color: var(--red);",
                            "Could not load versions: {e.trim().trim_end_matches('.')}."
                        }
                        button {
                            class: "btn btn--sm btn--ghost",
                            r#type: "button",
                            onclick: move |_| rev += 1,
                            "Retry load"
                        }
                    }
                },
                Some(Ok(list)) => rsx! {
                    for (i , v) in list.iter().enumerate() {
                        button {
                            class: "ih-tile__ver",
                            r#type: "button",
                            "aria-current": if v.id == viewing_id { "true" },
                            onclick: {
                                let id = v.id.clone();
                                move |_| on_pick.call(id.clone())
                            },
                            "v{list.len() - i} · {abs_time(v.published_at_ms)} · {counts_summary(&v.counts)}"
                        }
                    }
                },
            }
        }
    }
}

async fn load_reports(name: Option<String>) -> Result<Vec<ReportVersionView>, String> {
    match name {
        None => Ok(vec![]),
        Some(n) => dashboard_report_versions(n)
            .await
            .map_err(|e| e.to_string()),
    }
}

/// Settings select of existing report names (required: no empty choice once one exists).
#[component]
fn ReportNameSelect(current: Option<String>, on_settings: EventHandler<TileSpec>) -> Element {
    let names =
        use_resource(|| async { list_dashboard_reports().await.map_err(|e| e.to_string()) });
    let state = names.read().clone();
    rsx! {
        div { class: "field-row",
            label { class: "field-label", r#for: "ih-tile-set-report", "Report name" }
            match state {
                None => rsx! {
                    span { class: "ih-tile__meta", "Loading…" }
                },
                Some(Err(e)) => rsx! {
                    span { class: "ih-tile__meta", style: "color: var(--red);", "Could not load reports: {e}" }
                },
                Some(Ok(list)) if list.is_empty() => rsx! {
                    span { class: "ih-tile__meta", "No reports published yet" }
                },
                Some(Ok(list)) => rsx! {
                    select {
                        id: "ih-tile-set-report",
                        "aria-label": "Report name",
                        onchange: move |e| {
                            let v = e.value();
                            if !v.is_empty() {
                                on_settings.call(TileSpec::Reports { report: Some(v) });
                            }
                        },
                        if current.is_none() {
                            option { value: "", selected: true, "Choose a report" }
                        }
                        for n in list.iter() {
                            option { value: "{n}", selected: current.as_deref() == Some(n.as_str()), "{n}" }
                        }
                    }
                },
            }
        }
    }
}

#[component]
fn ReportsTile(
    ctx: TileCtx,
    report: Option<String>,
    on_settings: EventHandler<TileSpec>,
) -> Element {
    let spec = TileSpec::Reports {
        report: report.clone(),
    };
    let feed = use_feed(
        ctx.index,
        poll_interval_ms(&spec),
        report.clone(),
        load_reports,
    );
    let mut viewing = use_signal(|| None::<String>);
    let mut versions_open = use_signal(|| false);
    let st = feed.state.read().clone();
    let busy = *feed.busy.read();
    let now = ctx.now;
    let viewing_id = viewing.read().clone();
    let open = *versions_open.read();
    rsx! {
        TileShell {
            title: spec.title(),
            spec,
            ctx,
            status: st.frame(),
            busy,
            on_refresh: move |_| feed.refresh(),
            on_settings,
            link: None,
            if let Some(versions) = st.data() {
                ReportsBody {
                    name: report,
                    versions: versions.clone(),
                    viewing: viewing_id,
                    now,
                    versions_open: open,
                    on_toggle: move |_| {
                        let v = *versions_open.peek();
                        versions_open.set(!v);
                    },
                    on_pick: move |id: String| {
                        viewing.set(Some(id));
                        versions_open.set(false);
                    },
                    on_back: move |_| viewing.set(None),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TileBody: the entry point the item chrome renders
// ---------------------------------------------------------------------------

/// The body of a native tile item: dispatches on the tile kind.
#[component]
pub(crate) fn TileBody(
    item_id: String,
    spec: TileSpec,
    index: usize,
    now: i64,
    selected: bool,
    read_only: bool,
    on_settings: EventHandler<TileSpec>,
) -> Element {
    let ctx = TileCtx {
        item_id,
        index,
        now,
        selected,
        read_only,
    };
    match spec {
        TileSpec::Usage { window } => rsx! {
            UsageTile { ctx, window, on_settings }
        },
        TileSpec::Sessions { show } => rsx! {
            SessionsTile { ctx, show, on_settings }
        },
        TileSpec::Kanban { columns } => rsx! {
            KanbanTile { ctx, columns, on_settings }
        },
        TileSpec::Cron {} => rsx! {
            CronTile { ctx, on_settings }
        },
        TileSpec::Gateway {} => rsx! {
            GatewayTile { ctx, on_settings }
        },
        TileSpec::Reports { report } => rsx! {
            ReportsTile { ctx, report, on_settings }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::dashboard_tiles_api::UsageRowView;

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn poll_interval_and_stagger() {
        let usage = TileSpec::Usage {
            window: UsageWindow::Week,
        };
        assert_eq!(poll_interval_ms(&usage), 60_000);
        assert_eq!(
            poll_interval_ms(&TileSpec::Reports { report: None }),
            60_000
        );
        assert_eq!(
            poll_interval_ms(&TileSpec::Sessions {
                show: Default::default()
            }),
            30_000
        );
        assert_eq!(
            poll_interval_ms(&TileSpec::Kanban { columns: vec![] }),
            30_000
        );
        assert_eq!(poll_interval_ms(&TileSpec::Cron {}), 30_000);
        assert_eq!(poll_interval_ms(&TileSpec::Gateway {}), 30_000);
        assert_eq!(first_fetch_delay_ms(0), 0);
        assert_eq!(first_fetch_delay_ms(3), 750);
    }

    #[test]
    fn format_cost_uses_integer_micros() {
        assert_eq!(format_cost(1_234_567), "$1.23");
        assert_eq!(format_cost(0), "$0.00");
        assert_eq!(format_cost(5_000), "$0.01");
        assert_eq!(format_cost(4_999), "$0.00");
        assert_eq!(format_cost(-7), "$0.00");
    }

    fn usage_view(cost_recorded: bool) -> UsageTileView {
        let row = |model: &str, cost: i64, tokens: i64| UsageRowView {
            provider: "openrouter".into(),
            model: model.into(),
            cost_micros: if cost_recorded { cost } else { 0 },
            tokens,
        };
        UsageTileView {
            window: UsageWindow::Week,
            total_cost_micros: if cost_recorded { 1_234_567 } else { 0 },
            total_tokens: 9876,
            rows: vec![
                row("m-a", 800_000, 5000),
                row("m-b", 200_000, 2000),
                row("m-c", 100_000, 1500),
                row("m-d", 80_000, 900),
                row("m-e", 54_567, 476),
            ],
            failed_calls: 2,
            cost_recorded,
        }
    }

    #[test]
    fn usage_tile_renders_hero_rows_and_failed_calls() {
        let html = ssr(|| rsx! { UsageBody { view: usage_view(true) } });
        assert!(html.contains("$1.23"), "{html}");
        assert!(html.contains("9876 tokens · 7d"), "{html}");
        assert_eq!(html.matches("ih-tile__usage-row").count(), 5, "{html}");
        assert!(html.contains("openrouter"), "{html}");
        assert!(html.contains("2 failed calls"), "{html}");
        // The largest row fills the track; a smaller one is scaled to it.
        assert!(html.contains("width: 100%"), "{html}");
        assert!(html.contains("width: 25%"), "{html}");
        assert!(!html.contains("Cost not recorded"), "{html}");

        let tokens = ssr(|| rsx! { UsageBody { view: usage_view(false) } });
        assert!(tokens.contains("ih-tile__hero\">9876<"), "{tokens}");
        assert!(tokens.contains("Cost not recorded"), "{tokens}");
        assert!(!tokens.contains('$'), "{tokens}");
    }

    #[test]
    fn usage_tile_empty_window_and_single_failure() {
        let mut v = usage_view(true);
        v.rows.clear();
        v.failed_calls = 1;
        let html = ssr_view(v);
        assert!(html.contains("No usage in this window"), "{html}");
        assert!(
            html.contains("Token use appears here after the agent runs a turn."),
            "{html}"
        );
        assert!(html.contains("1 failed call<"), "{html}");
    }

    fn ssr_view(v: UsageTileView) -> String {
        thread_local! { static V: std::cell::RefCell<Option<UsageTileView>> = const { std::cell::RefCell::new(None) }; }
        V.with(|c| *c.borrow_mut() = Some(v));
        ssr(|| {
            let v = V.with(|c| c.borrow().clone()).unwrap();
            rsx! { UsageBody { view: v } }
        })
    }

    #[test]
    fn tile_polling_lives_only_in_the_dashboard_tree() {
        const HERMES_APP_RS: &str = include_str!("../../mod.rs");
        assert!(!HERMES_APP_RS.contains("dashboard::tiles"));
    }

    fn session_rows() -> Vec<DashSessionRow> {
        (0..8)
            .map(|i| DashSessionRow {
                id: format!("s{i}"),
                title: Some(format!("Chat {i}")),
                started_at: 1_000.0 - f64::from(i),
                active: i % 3 == 0,
            })
            .collect()
    }

    fn sessions_el(show: SessionsShow) -> Element {
        rsx! {
            SessionsBody { rows: session_rows(), show, now: 1_000, on_open: |_| {} }
        }
    }

    fn frame_el(status: FrameStatus, busy: bool, link: Option<String>) -> Element {
        rsx! {
            TileFrame {
                title: "Kanban".to_string(),
                status,
                now: 1_300,
                busy,
                has_settings: true,
                settings_open: false,
                gear_id: "g".to_string(),
                on_refresh: |_| {},
                on_gear: |_| {},
                link,
                on_link: |_| {},
                div { "body" }
            }
        }
    }

    #[test]
    fn sessions_tile_rows_and_active_pill() {
        let html = ssr(|| sessions_el(SessionsShow::Recent));
        assert_eq!(html.matches("ih-tile__row").count(), 6, "{html}");
        assert!(html.contains("+2 more"), "{html}");
        // Rows 0 and 3 are active and visible; row 6 is hidden behind the clamp.
        assert_eq!(html.matches("ACTIVE").count(), 2, "{html}");
        assert!(
            html.contains("Chat 0") && !html.contains("Chat 7"),
            "{html}"
        );

        let active = ssr(|| sessions_el(SessionsShow::Active));
        assert_eq!(active.matches("ih-tile__row").count(), 3, "{active}");
        assert_eq!(active.matches("ACTIVE").count(), 3, "{active}");
        assert!(!active.contains("more"), "{active}");

        let none = ssr(|| {
            rsx! {
                SessionsBody { rows: vec![], show: SessionsShow::Recent, now: 0, on_open: |_| {} }
            }
        });
        assert!(none.contains("No sessions yet"), "{none}");
        assert!(
            none.contains("Start a chat and it shows up here."),
            "{none}"
        );

        // The footer link comes from the shared frame.
        let frame = ssr(|| {
            frame_el(
                FrameStatus::Ready {
                    at: 1_300,
                    stale: false,
                },
                false,
                Some("Open Sessions →".to_string()),
            )
        });
        assert!(frame.contains("Open Sessions →"), "{frame}");
    }

    fn kanban_view() -> KanbanTileView {
        KanbanTileView {
            open_total: 9,
            columns: vec![
                (KanbanColumn::Todo, 3),
                (KanbanColumn::Running, 2),
                (KanbanColumn::Blocked, 4),
            ],
            blocked: (0..5).map(|i| format!("stuck {i}")).collect(),
        }
    }

    #[test]
    fn kanban_tile_chips_and_blocked_rows() {
        let html = ssr(|| rsx! { KanbanBody { view: kanban_view() } });
        assert!(html.contains("ih-tile__hero\">9<"), "{html}");
        assert_eq!(html.matches("class=\"ih-tile__chip\"").count(), 3, "{html}");
        assert_eq!(html.matches("BLOCKED").count(), 3, "{html}");
        assert!(html.contains("+2 more"), "{html}");
        assert!(
            html.contains("stuck 0") && !html.contains("stuck 3"),
            "{html}"
        );
        assert!(!html.contains("FAILED"), "{html}");

        let empty = ssr(|| {
            rsx! {
                KanbanBody {
                    view: KanbanTileView {
                        open_total: 0,
                        columns: vec![(KanbanColumn::Todo, 0)],
                        blocked: vec![],
                    },
                }
            }
        });
        assert!(empty.contains("No tasks on this board"), "{empty}");
        assert!(
            empty.contains("Create a task on the Kanban screen."),
            "{empty}"
        );
    }

    #[test]
    fn kanban_columns_toggle_keeps_one_and_canonical_order() {
        use KanbanColumn::*;
        assert_eq!(toggle_column(&[Todo], Todo), vec![Todo]);
        assert_eq!(toggle_column(&[Blocked, Todo], Blocked), vec![Todo]);
        assert_eq!(toggle_column(&[Blocked], Triage), vec![Triage, Blocked]);
        let boxes = column_boxes(&[]);
        assert_eq!(boxes.len(), 6);
        assert!(boxes.iter().all(|(_, on, _)| *on));
        assert_eq!(boxes[5].2.len(), 5, "unticking Done leaves five");
    }

    fn at(unix: i64) -> String {
        chrono::DateTime::from_timestamp(unix, 0)
            .unwrap()
            .to_rfc3339()
    }

    #[test]
    fn cron_tile_next_and_last_run() {
        let html = ssr(|| {
            let now = 1_000_000_000;
            let view = CronTileView {
                next: vec![
                    ("backup".into(), at(now + 3 * 3600)),
                    ("report".into(), at(now + 86_400)),
                ],
                last: Some(crate::server::dashboard_tiles_api::CronLastRun {
                    name: "nightly".into(),
                    at_raw: at(now - 7200),
                    at_display: None,
                    failed: Some(true),
                    error: Some("exit status 2".into()),
                }),
            };
            rsx! { CronBody { view, now } }
        });
        assert!(
            html.contains("Next runs") && html.contains("Last run"),
            "{html}"
        );
        assert!(html.contains("in 3 h") && html.contains("in 1 d"), "{html}");
        assert!(
            html.contains("FAILED") && html.contains("2 h ago"),
            "{html}"
        );
        assert!(html.contains("nightly"), "{html}");
        assert!(
            html.contains("ih-tile__clamp") && html.contains("title=\"exit status 2\""),
            "{html}"
        );
        assert!(!html.contains("history"), "{html}");

        let empty = ssr(|| {
            rsx! {
                CronBody { view: CronTileView { next: vec![], last: None }, now: 0 }
            }
        });
        assert!(empty.contains("No scheduled jobs"), "{empty}");
        assert!(
            empty.contains("Create one on the Schedules screen."),
            "{empty}"
        );

        let never = ssr(|| {
            rsx! {
                CronBody {
                    view: CronTileView {
                        next: vec![("a".into(), at(4_000_000))],
                        last: None,
                    },
                    now: 3_000_000,
                }
            }
        });
        assert!(never.contains("NEVER RUN"), "{never}");
    }

    #[test]
    fn in_duration_buckets() {
        assert_eq!(in_duration(-5), "in <1 min");
        assert_eq!(in_duration(59), "in <1 min");
        assert_eq!(in_duration(60), "in 1 min");
        assert_eq!(in_duration(7200), "in 2 h");
        assert_eq!(in_duration(172_800), "in 2 d");
    }

    #[test]
    fn gateway_tile_connected_disconnected_only() {
        let html = ssr(|| {
            rsx! {
                GatewayBody {
                    rows: vec![
                        GatewayRow {
                            platform: "telegram".into(),
                            connected: true,
                        },
                        GatewayRow {
                            platform: "discord".into(),
                            connected: false,
                        },
                    ],
                }
            }
        });
        assert!(
            html.contains("telegram") && html.contains("discord"),
            "{html}"
        );
        assert_eq!(html.matches("DISCONNECTED").count(), 1, "{html}");
        assert!(html.contains(">CONNECTED<"), "{html}");
        for gone in ["DEGRADED", "OFF", "Recent failures"] {
            assert!(!html.contains(gone), "{gone}: {html}");
        }
        let empty = ssr(|| rsx! { GatewayBody { rows: vec![] } });
        assert!(empty.contains("No platforms configured"), "{empty}");
        assert!(
            empty.contains("Connect one on the Gateway screen."),
            "{empty}"
        );
    }

    #[test]
    fn tile_error_and_stale_states() {
        let failed = ssr(|| frame_el(FrameStatus::Failed("boom".into()), false, None));
        assert!(failed.contains("Could not load Kanban: boom."), "{failed}");
        assert!(failed.contains("Retry load"), "{failed}");
        assert!(
            !failed.contains("body"),
            "no data on a first-load error: {failed}"
        );
        // A trailing period in the reason is not doubled.
        assert_eq!(
            tile_load_error("Usage", "bad."),
            "Could not load Usage: bad."
        );

        let stale = ssr(|| {
            frame_el(
                FrameStatus::Ready {
                    at: 1_000,
                    stale: true,
                },
                false,
                None,
            )
        });
        assert!(stale.contains(">STALE<"), "{stale}");
        assert!(
            stale.contains("Could not refresh. Showing data from 5 min ago."),
            "{stale}"
        );
        assert!(stale.contains("Retry refresh"), "{stale}");
        assert!(stale.contains("body"), "data stays: {stale}");

        let fresh = ssr(|| {
            frame_el(
                FrameStatus::Ready {
                    at: 1_000,
                    stale: false,
                },
                true,
                None,
            )
        });
        assert!(fresh.contains("Updated 5 min ago"), "{fresh}");
        assert!(!fresh.contains("STALE"), "{fresh}");
        assert!(
            fresh.contains("disabled") && fresh.contains("aria-busy=\"true\""),
            "{fresh}"
        );
        assert!(fresh.contains("aria-label=\"Refresh Kanban\""), "{fresh}");
        assert!(
            fresh.contains("aria-label=\"Settings for Kanban\""),
            "{fresh}"
        );

        let loading = ssr(|| frame_el(FrameStatus::Loading, false, None));
        assert!(loading.contains("Loading…"), "{loading}");
        assert!(!loading.contains("body"), "{loading}");
    }

    fn ver(n: i64, counts: [u32; 5]) -> ReportVersionView {
        ReportVersionView {
            id: format!("v{n}"),
            name: "redteam".into(),
            title: format!("Run {n}"),
            summary: "Two injection paths found.".into(),
            counts,
            findings: vec![
                ("high".into(), "SQL injection".into()),
                ("low".into(), "Verbose errors".into()),
            ],
            published_at_ms: 1_700_000_000_000 + n * 86_400_000,
        }
    }

    /// Versions newest first, as the server returns them.
    fn five_versions() -> Vec<ReportVersionView> {
        (1..=5).rev().map(|n| ver(n, [0, 3, 1, 0, 0])).collect()
    }

    fn reports_el(
        name: Option<&str>,
        versions: Vec<ReportVersionView>,
        viewing: Option<&str>,
    ) -> Element {
        rsx! {
            ReportsBody {
                name: name.map(String::from),
                versions,
                viewing: viewing.map(String::from),
                now: 1_700_000_000 + 6 * 86_400,
                versions_open: false,
                on_toggle: |_| {},
                on_pick: |_| {},
                on_back: |_| {},
            }
        }
    }

    #[test]
    fn severity_pills_with_delta() {
        let counts = [0, 3, 1, 0, 0];
        let pills = severity_pills(&counts, Some([0, 1, 0, 0, 0]));
        assert_eq!(
            pills,
            vec![
                ("3 high (+1)".to_string(), "red"),
                ("1 medium (no change)".to_string(), ""),
            ]
        );
        // Falling high is green; a falling or rising medium stays neutral.
        let falling = severity_pills(&counts, Some([0, -2, 2, 0, 0]));
        assert_eq!(falling[0], ("3 high (-2)".to_string(), "green"));
        assert_eq!(falling[1], ("1 medium (+2)".to_string(), ""));
        // v1 has no delta: plain counts, no parentheses.
        let single = severity_pills(&counts, None);
        assert_eq!(
            single,
            vec![("3 high".to_string(), ""), ("1 medium".to_string(), "")]
        );
        // Highest severity first, zeros omitted.
        let all = severity_pills(&[1, 0, 0, 2, 0], None);
        assert_eq!(all[0].0, "1 critical");
        assert_eq!(all[1].0, "2 low");

        let one = ssr(|| reports_el(Some("redteam"), vec![ver(1, [0, 3, 1, 0, 0])], None));
        assert!(one.contains("v1 · "), "{one}");
        assert!(one.contains("3 high") && !one.contains('('), "{one}");
        let many = ssr(|| reports_el(Some("redteam"), five_versions(), None));
        assert!(many.contains("3 high (no change)"), "{many}");
    }

    #[test]
    fn reports_tile_unbound_and_empty() {
        let unbound = ssr(|| reports_el(None, vec![], None));
        assert!(unbound.contains("Choose a report"), "{unbound}");
        assert!(
            unbound.contains("Open this tile&#39;s settings and pick a report name.")
                || unbound.contains("Open this tile's settings and pick a report name."),
            "{unbound}"
        );
        let empty = ssr(|| reports_el(Some("redteam"), vec![], None));
        assert!(empty.contains("No reports published yet"), "{empty}");
        assert!(
            empty.contains(
                "A red-team run or skill publishes a report with the report_publish tool. It appears here."
            ),
            "{empty}"
        );
    }

    #[test]
    fn reports_version_banner() {
        let viewing = ssr(|| reports_el(Some("redteam"), five_versions(), Some("v2")));
        let when = abs_time(ver(2, [0; 5]).published_at_ms);
        assert!(
            viewing.contains(&format!("Viewing v2 of 5, {when}.")),
            "{viewing}"
        );
        assert!(viewing.contains("Back to latest"), "{viewing}");
        // Counts only while viewing an older version.
        assert!(!viewing.contains("(no change)"), "{viewing}");
        let latest = ssr(|| reports_el(Some("redteam"), five_versions(), None));
        assert!(!latest.contains("Viewing"), "{latest}");
        assert!(!latest.contains("Back to latest"), "{latest}");
        assert!(latest.contains("Versions ▾"), "{latest}");
    }

    #[test]
    fn viewed_version_survives_poll() {
        let list = five_versions();
        assert_eq!(select_shown_version(Some("v2"), &list).unwrap().id, "v2");
        assert_eq!(select_shown_version(None, &list).unwrap().id, "v5");
        // Pruned between polls: fall back to the latest.
        let pruned: Vec<_> = list.into_iter().filter(|v| v.id != "v2").collect();
        assert_eq!(select_shown_version(Some("v2"), &pruned).unwrap().id, "v5");
        assert!(select_shown_version(Some("v2"), &[]).is_none());
        assert_eq!(counts_summary(&[0, 3, 1, 0, 0]), "3 high, 1 medium");
        assert_eq!(counts_summary(&[0; 5]), "no findings");
    }

    #[test]
    fn reports_missing_fields_are_omitted() {
        let html = ssr(|| {
            let mut bare = ver(1, [0, 0, 0, 0, 0]);
            bare.summary.clear();
            bare.title.clear();
            bare.findings.clear();
            reports_el(Some("redteam"), vec![bare], None)
        });
        assert!(!html.contains("ih-tile__clamp"), "{html}");
        assert!(!html.contains("ih-tile__findings"), "{html}");
        assert!(!html.contains("ih-tile__heading"), "{html}");
        assert!(!html.contains("Could not"), "{html}");
        assert!(html.contains("v1 · "), "{html}");

        // Findings render as text with their severity pill.
        let full = ssr(|| reports_el(Some("redteam"), vec![ver(1, [0, 1, 0, 1, 0])], None));
        assert!(full.contains("ih-tile__findings"), "{full}");
        assert!(
            full.contains("SQL injection") && full.contains(">HIGH<"),
            "{full}"
        );
    }

    #[test]
    fn next_state_keeps_data_on_a_failed_refresh() {
        let ready = next_state(FeedState::<u8>::Loading, Ok(7), 100);
        assert_eq!(
            ready.frame(),
            FrameStatus::Ready {
                at: 100,
                stale: false
            }
        );
        let stale = next_state(ready, Err("boom".into()), 200);
        assert_eq!(stale.data(), Some(&7));
        assert_eq!(
            stale.frame(),
            FrameStatus::Ready {
                at: 100,
                stale: true
            }
        );
        let failed = next_state(FeedState::<u8>::Loading, Err("boom".into()), 5);
        assert_eq!(failed.frame(), FrameStatus::Failed("boom".into()));
        let healed = next_state(stale, Ok(9), 300);
        assert_eq!(
            healed.frame(),
            FrameStatus::Ready {
                at: 300,
                stale: false
            }
        );
    }
}
