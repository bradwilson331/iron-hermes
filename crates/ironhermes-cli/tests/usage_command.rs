//! Phase 36.2 Plan 10 — `/usage` slash command + StateStore::query_usage_events
//! integration tests.
//!
//! Per RESEARCH OQ-7 / `handlers_state_store.rs` precedent: placed in
//! `ironhermes-cli/tests/` (not `ironhermes-core/tests/`) because
//! `ironhermes-state` imports `ironhermes-core` — putting StateStore tests
//! in `ironhermes-core/tests/` would create a circular dev-dep. The CLI
//! crate already depends on both core and state.
//!
//! Behaviors verified (per 36.2-10-PLAN.md `<behavior>`):
//!   b1  — params-only (T-36.2-10-INJ): SQL-injection-shaped --provider value
//!         does NOT drop the usage_events table; returns 0 rows.
//!   b2  — bare /usage current-session: only ctx.session_id's rows surface.
//!   b3  — /usage --today: today-only cross-session filter partitions correctly.
//!   b4  — /usage --provider X: provider filter rolls up only matching rows.
//!   b5  — /usage --model X: model filter rolls up only matching rows.
//!   b6  — /usage --since 7d: rolling-window filter applies ts cutoff.
//!   b7  — parse_since: unit table (d/h/m) + None on empty / junk / unknown.
//!   b8  — empty result: filter with no matching rows returns sentinel text.
//!   b9  — registry args_hint: `/usage` CommandDef has the documented hint.
//!   b10 — dispatch wired: handlers.rs no longer references todo_stub("usage").
//!   b11 — table formatting: cost_usd_micros = 234_000 renders as `$0.234` or
//!         `$0.2340` (display-only f64).
//!   b12 — no SQL format-interpolation: ironhermes-state lib.rs has zero
//!         `format!(...SELECT` lines (T-36.2-10-INJ structural lock).
//!   b13 — running-agent guard does NOT bypass /usage.

use std::sync::{Arc, Mutex};

use ironhermes_core::commands::context::{CommandContext, StateStoreHandle};
use ironhermes_core::commands::handlers::{dispatch, parse_since};
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;
use ironhermes_state::{StateStore, UsageEvent, UsageFilter, format_usage_rollups};
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// Test adapter: Arc<Mutex<StateStore>> -> StateStoreHandle
// ---------------------------------------------------------------------------

struct TestStateStoreAdapter(Arc<Mutex<StateStore>>);

impl StateStoreHandle for TestStateStoreAdapter {
    fn list_sessions_text(&self, _limit: usize) -> String {
        unreachable!("not exercised by Plan 10 usage tests")
    }
    fn list_sessions_text_filtered(&self, _limit: usize, _workspace_root: Option<&str>) -> String {
        unreachable!("not exercised by Plan 10 usage tests")
    }
    fn history_text(&self, _session_id: &str) -> String {
        unreachable!("not exercised by Plan 10 usage tests")
    }
    fn export_session_text(&self, _session_id: &str) -> String {
        unreachable!("not exercised by Plan 10 usage tests")
    }
    fn update_title(&self, _session_id: &str, _title: &str) -> Result<(), String> {
        unreachable!("not exercised by Plan 10 usage tests")
    }
    fn get_session_id(&self, _name_or_id: &str) -> Option<String> {
        unreachable!("not exercised by Plan 10 usage tests")
    }
    fn usage_text(
        &self,
        session_id: Option<&str>,
        today_only: bool,
        provider: Option<&str>,
        model: Option<&str>,
        since_seconds: Option<i64>,
    ) -> String {
        let filter = UsageFilter {
            session_id: session_id.map(|s| s.to_string()),
            today_only,
            provider: provider.map(|s| s.to_string()),
            model: model.map(|s| s.to_string()),
            since_seconds,
        };
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.query_usage_events(&filter) {
            Ok(rows) => format_usage_rollups(&rows, &filter),
            Err(e) => format!("Usage query failed: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn fresh_store() -> (Arc<Mutex<StateStore>>, NamedTempFile) {
    let f = NamedTempFile::new().unwrap();
    let store = StateStore::new(f.path()).unwrap();
    (Arc::new(Mutex::new(store)), f)
}

fn make_ctx_with_session(session_id: &str, store: Arc<Mutex<StateStore>>) -> CommandContext {
    let mut ctx = CommandContext::new(Platform::Local, session_id.to_string());
    ctx.state_store = Some(Arc::new(TestStateStoreAdapter(store)));
    ctx
}

fn make_router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

fn find_cmd(name: &'static str) -> CommandDef {
    build_registry()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("Command '{}' not found in registry", name))
}

#[allow(clippy::too_many_arguments)] // test helper: all args are distinct event fields; params-struct would add noise with no clarity gain
fn insert_event(
    store: &Arc<Mutex<StateStore>>,
    session_id: &str,
    ts_ms: i64,
    provider: &str,
    model: &str,
    in_tok: i64,
    out_tok: i64,
    cost: i64,
) {
    let ev = UsageEvent {
        session_id: session_id.to_string(),
        ts: ts_ms,
        provider: provider.to_string(),
        model: model.to_string(),
        in_tok,
        out_tok,
        cache_read: 0,
        cache_create: 0,
        cost_usd_micros: cost,
        error_kind: None,
    };
    let mut g = store.lock().unwrap();
    let _ = g.conn_for_test().execute(
        "INSERT OR IGNORE INTO sessions \
         (id, started_at, source) VALUES (?1, ?2, ?3)",
        rusqlite::params![session_id, ts_ms / 1000, "test"],
    );
    g.insert_usage_event(&ev).unwrap();
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------------------------------------------------------------------------
// b1 — SQL injection safety (T-36.2-10-INJ).
// ---------------------------------------------------------------------------

#[test]
fn b1_sql_injection_provider_value_is_bound_not_interpolated() {
    let (store, _f) = fresh_store();
    insert_event(&store, "s1", now_ms(), "anthropic", "m", 1, 1, 1);

    let evil = "anthropic'; DROP TABLE usage_events--";
    let filter = UsageFilter {
        provider: Some(evil.to_string()),
        ..Default::default()
    };
    let rollups = {
        let g = store.lock().unwrap();
        g.query_usage_events(&filter).expect("query should succeed")
    };
    assert!(
        rollups.is_empty(),
        "malicious provider value must return 0 rows (no match), not all rows"
    );

    let table_count: i64 = {
        let g = store.lock().unwrap();
        let conn = g.conn_for_test();
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'usage_events'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(
        table_count, 1,
        "usage_events table must still exist after SQL-injection-shaped filter"
    );
}

// ---------------------------------------------------------------------------
// b2 — bare /usage filters to ctx.session_id.
// ---------------------------------------------------------------------------

#[test]
fn b2_bare_usage_filters_to_current_session() {
    let (store, _f) = fresh_store();
    let now = now_ms();
    insert_event(&store, "s1", now, "anthropic", "m", 100, 50, 1_000);
    insert_event(&store, "s2", now, "anthropic", "m", 200, 100, 9_999);

    let ctx = make_ctx_with_session("s1", store);
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &[], &ctx, &router);
    let text = match res {
        CommandResult::Output(s) => s,
        other => panic!("expected Output, got {other:?}"),
    };
    assert!(
        text.contains("$0.0010"),
        "expected s1 cost in output: {text}"
    );
    assert!(
        !text.contains("$0.0099") && !text.contains("$0.0100"),
        "s2 cost MUST be filtered out of current-session output: {text}"
    );
}

// ---------------------------------------------------------------------------
// b3 — /usage --today (cross-session, today-only window).
// ---------------------------------------------------------------------------

#[test]
fn b3_today_flag_aggregates_across_sessions_today_only() {
    let (store, _f) = fresh_store();
    let today_ms = now_ms();
    let yesterday_ms = today_ms - 2 * 24 * 60 * 60 * 1000;

    insert_event(&store, "s_a", today_ms, "anthropic", "m", 100, 50, 1_000);
    insert_event(&store, "s_b", today_ms, "anthropic", "m", 100, 50, 1_000);
    insert_event(
        &store,
        "s_y",
        yesterday_ms,
        "anthropic",
        "m",
        999,
        999,
        9_999,
    );

    let ctx = make_ctx_with_session("s_a", store);
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &["--today"], &ctx, &router);
    let text = match res {
        CommandResult::Output(s) => s,
        other => panic!("expected Output, got {other:?}"),
    };
    assert!(
        text.contains("$0.0020"),
        "expected $0.0020 in output: {text}"
    );
    assert!(
        !text.contains("$0.0099") && !text.contains("$0.0100"),
        "yesterday's row must be filtered out: {text}"
    );
}

// ---------------------------------------------------------------------------
// b4 — /usage --provider X.
// ---------------------------------------------------------------------------

#[test]
fn b4_provider_filter_isolates_provider_rows() {
    let (store, _f) = fresh_store();
    let now = now_ms();
    insert_event(&store, "s1", now, "anthropic", "m", 100, 50, 1_000);
    insert_event(&store, "s2", now, "openrouter", "m", 999, 999, 9_999);

    let ctx = make_ctx_with_session("s1", store);
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &["--provider", "anthropic"], &ctx, &router);
    let text = match res {
        CommandResult::Output(s) => s,
        other => panic!("expected Output, got {other:?}"),
    };
    assert!(text.contains("anthropic"), "anthropic row missing: {text}");
    assert!(
        !text.contains("openrouter"),
        "openrouter row must be filtered out: {text}"
    );
}

// ---------------------------------------------------------------------------
// b5 — /usage --model X.
// ---------------------------------------------------------------------------

#[test]
fn b5_model_filter_isolates_model_rows() {
    let (store, _f) = fresh_store();
    let now = now_ms();
    insert_event(&store, "s1", now, "anthropic", "opus-4-7", 100, 50, 1_000);
    insert_event(
        &store,
        "s2",
        now,
        "anthropic",
        "sonnet-4-6",
        999,
        999,
        9_999,
    );

    let ctx = make_ctx_with_session("s1", store);
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &["--model", "opus-4-7"], &ctx, &router);
    let text = match res {
        CommandResult::Output(s) => s,
        other => panic!("expected Output, got {other:?}"),
    };
    assert!(text.contains("opus-4-7"), "opus-4-7 row missing: {text}");
    assert!(
        !text.contains("sonnet-4-6"),
        "sonnet-4-6 row must be filtered out: {text}"
    );
}

// ---------------------------------------------------------------------------
// b6 — /usage --since 7d (rolling window).
// ---------------------------------------------------------------------------

#[test]
fn b6_since_7d_filters_to_rolling_window() {
    let (store, _f) = fresh_store();
    let now = now_ms();
    let three_days = now - 3 * 24 * 60 * 60 * 1000;
    let ten_days = now - 10 * 24 * 60 * 60 * 1000;

    insert_event(
        &store,
        "s_recent",
        three_days,
        "anthropic",
        "m",
        100,
        50,
        1_000,
    );
    insert_event(&store, "s_old", ten_days, "anthropic", "m", 999, 999, 9_999);

    let ctx = make_ctx_with_session("s_recent", store);
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &["--since", "7d"], &ctx, &router);
    let text = match res {
        CommandResult::Output(s) => s,
        other => panic!("expected Output, got {other:?}"),
    };
    assert!(
        text.contains("$0.0010"),
        "expected $0.0010 in output: {text}"
    );
    assert!(
        !text.contains("$0.0099") && !text.contains("$0.0100"),
        "10-day-old row must be filtered out: {text}"
    );
}

// ---------------------------------------------------------------------------
// b7 — parse_since unit table.
// ---------------------------------------------------------------------------

#[test]
fn b7_parse_since_unit_table() {
    assert_eq!(parse_since("7d"), Some(7 * 86400));
    assert_eq!(parse_since("24h"), Some(24 * 3600));
    assert_eq!(parse_since("30m"), Some(30 * 60));
    assert_eq!(parse_since(""), None);
    assert_eq!(parse_since("junk"), None);
    assert_eq!(parse_since("7x"), None, "unknown unit must return None");
    assert_eq!(parse_since("d"), None, "missing number must return None");
}

// ---------------------------------------------------------------------------
// b8 — empty result returns sentinel text.
// ---------------------------------------------------------------------------

#[test]
fn b8_empty_result_returns_sentinel_text() {
    let (store, _f) = fresh_store();
    let ctx = make_ctx_with_session("never-existed", store);
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &[], &ctx, &router);
    let text = match res {
        CommandResult::Output(s) => s,
        other => panic!("expected Output, got {other:?}"),
    };
    assert!(
        text.contains("No usage data"),
        "empty filter must return 'No usage data found...' sentinel: {text}"
    );
}

// ---------------------------------------------------------------------------
// b9 — registry args_hint set.
// ---------------------------------------------------------------------------

#[test]
fn b9_registry_usage_command_has_args_hint() {
    let usage = find_cmd("usage");
    assert_eq!(
        usage.args_hint, "[--today | --provider X | --since Nd | --model X]",
        "args_hint on /usage must match the documented set"
    );
}

// ---------------------------------------------------------------------------
// b10 — dispatch wired; no longer returns todo_stub output.
// ---------------------------------------------------------------------------

#[test]
fn b10_dispatch_for_usage_is_no_longer_todo_stub() {
    let ctx = CommandContext::new(Platform::Local, "any-session".to_string());
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &[], &ctx, &router);
    match res {
        CommandResult::Output(s) => {
            assert!(
                !s.contains("is not yet available"),
                "Plan 10 must replace the todo_stub output for /usage; got: {s}"
            );
            assert!(
                !s.contains("No token cost tracking"),
                "Plan 10 must replace the todo_stub reason for /usage; got: {s}"
            );
            assert!(
                s.contains("Session storage not configured")
                    || s.contains("Usage")
                    || s.contains("No usage data"),
                "expected cmd_usage guard or table output; got: {s}"
            );
        }
        other => panic!("expected Output, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// b11 — table formatting; i64 micros to display USD.
// ---------------------------------------------------------------------------

#[test]
fn b11_table_formatting_renders_micro_usd_as_decimal() {
    let (store, _f) = fresh_store();
    insert_event(&store, "s1", now_ms(), "anthropic", "m", 0, 0, 234_000);

    let ctx = make_ctx_with_session("s1", store);
    let router = make_router();
    let cmd = find_cmd("usage");
    let res = dispatch(&cmd, &[], &ctx, &router);
    let text = match res {
        CommandResult::Output(s) => s,
        other => panic!("expected Output, got {other:?}"),
    };
    assert!(
        text.contains("$0.2340") || text.contains("$0.234"),
        "cost 234_000 micros must render as $0.234 or $0.2340: {text}"
    );
}

// ---------------------------------------------------------------------------
// b12 — no SQL `format!(...SELECT` interpolation in ironhermes-state lib.rs.
// ---------------------------------------------------------------------------

#[test]
fn b12_state_lib_has_no_format_select_lines() {
    let src = include_str!("../../ironhermes-state/src/lib.rs");
    let bad: Vec<&str> = src
        .lines()
        .filter(|l| {
            if let Some(fmt_idx) = l.find("format!")
                && l[fmt_idx..].contains("SELECT")
            {
                return true;
            }
            false
        })
        .collect();
    assert!(
        bad.is_empty(),
        "ironhermes-state/src/lib.rs must not contain format!(\"...SELECT...\") lines; \
         found: {bad:#?}"
    );
}

// ---------------------------------------------------------------------------
// b13 — running-agent guard removed (Phase 39.1 Plan 06).
// ---------------------------------------------------------------------------

/// Phase 39.1 Plan 06 (R39.1-06): running_agent module deleted — is_bypass() gone.
///
/// The original b13 asserted `!is_bypass("usage")` (usage must not bypass the gate).
/// The gate is now removed entirely — all commands dispatch unconditionally.
/// Converted to a traceability note: /usage dispatches like any other command.
#[test]
fn b13_usage_dispatches_without_gate() {
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    // /usage must resolve to the canonical "usage" command (gate removed — no rejection).
    let router = CommandRouter::new(build_registry());
    let resolved = router.resolve("/usage", &Platform::Local);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            assert_eq!(
                def.name, "usage",
                "/usage must resolve to canonical 'usage' def (gate removed in Plan 06)"
            );
        }
        other => panic!(
            "/usage must resolve to a known command (gate removed — no rejection), got: {:?}",
            other
        ),
    }
}
