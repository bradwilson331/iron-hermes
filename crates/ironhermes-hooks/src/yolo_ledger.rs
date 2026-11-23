//! Per-turn yolo-bypass ledger (Phase 54.3 D-03/D-05/D-12).
//!
//! Under `yolo = true`, `gated_exec` used to announce every bypassed call to the
//! operator on its own. Because nearly every non-trivial shell command trips the D-09
//! metacharacter `Warn`, that meant ~5 notices per turn (D-02). This module is the
//! de-clutter lever: bypasses are RECORDED into a turn-scoped ledger as they happen and
//! announced ONCE, at the end of the turn, by [`flush_yolo_ledger`].
//!
//! # Shape
//!
//! ```text
//! AgentRuntime::run_turn
//!   └─ YOLO_LEDGER.scope(ledger, agent.run(..))          // task-local, one per turn
//!        └─ … tool dispatch … execute_gated_command(yolo=true)
//!             ├─ audit(resolution="bypass")                // per call, UNCHANGED (D-04)
//!             └─ try_record(..)                            // synchronous push, no await
//!   └─ flush_yolo_ledger(ledger, audit_log, notifier, turn_id)   // once, before `run_result?`
//!        ├─ append ONE rollup audit row  (kind="yolo_bypass_rollup", resolution="rollup")
//!        └─ notifier.notify_yolo_bypass_turn(summary)      // bounded by a timeout
//! ```
//!
//! # Fail-visible
//!
//! [`try_record`] reports whether a ledger was in scope. A bypass reached OUTSIDE any
//! scope (a surface that never goes through `run_turn`) returns `false`, and the caller
//! falls back to the per-call `notify_yolo_bypass`. Surfaces with a UI (CLI/TUI/web) show it;
//! the gateway gate's per-call method sends nothing to chat (Phase 54.11 D-19) because a
//! chat platform turns N ledger-less bypasses into N identical messages. The per-call audit
//! row is the record there.
//!
//! # Task-local propagation constraint
//!
//! The design is valid only because tokio task-locals survive inline `.await`,
//! `tokio::time::timeout`, `select!`, `join!`, boxed `dyn Future` and `LocalSet`, and are
//! LOST across `tokio::spawn`, `spawn_blocking`, and the `async_bridge::block_on_sync`
//! thread hop. Routing a gated call through any of those in a future refactor silently
//! drops the ledger into the per-call fallback (visible, not silent — but noisy again).
//! `crates/ironhermes-cron-runner/src/lib.rs` is the other `task_local!` precedent and
//! documents the same `tokio::spawn` caveat.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ironhermes_core::{ApprovalGate, AuditLog, YoloBypass, YoloBypassSummary};

/// Audit `kind` of the one-per-turn rollup row. Collision-free against every `kind`
/// present in the live log. The deferred audit-viewer phase keys on this value.
pub const YOLO_ROLLUP_KIND: &str = "yolo_bypass_rollup";

/// Audit `resolution` of the rollup row. MUST NOT be `"bypass"`: a rollup carrying
/// `resolution="bypass"` would double-count every `select(.resolution=="bypass")`
/// consumer and break the 1:1 D-04 forensic record.
pub const YOLO_ROLLUP_RESOLUTION: &str = "rollup";

/// Stored per-bypass detail cap (T-54.3-03). The EXACT count is kept separately, so
/// `args.count` stays truthful in a runaway turn without unbounded memory.
pub const YOLO_LEDGER_MAX_DETAIL: usize = 256;

/// Upper bound on how long [`flush_yolo_ledger`] awaits the surface notifier
/// (T-54.3-02). The rollup audit row is already on disk by then.
pub const YOLO_FLUSH_TIMEOUT_SECS: u64 = 5;

/// One sidecar-unavailable event recorded in the turn ledger (Phase 57 D-12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableEntry {
    /// Tool the screened call was for (`terminal`, `execute_code`, ...).
    pub tool: String,
    /// Why the sidecar could not answer: `missing`, `crash`, `timeout`, `too_large`,
    /// `protocol_error`.
    pub reason: String,
    /// DEFCON level in force.
    pub level: u8,
    /// The unavailable verdict for the subject: `allow`, `warn` or `block`.
    pub verdict: String,
}

/// Everything the sidecar-unavailable ledger holds at flush time.
#[derive(Clone, Debug, Default)]
pub struct UnavailableSummary {
    pub session_id: String,
    pub surface: String,
    pub chat_id: String,
    /// Stored detail, capped at [`YOLO_LEDGER_MAX_DETAIL`].
    pub items: Vec<UnavailableEntry>,
    /// Events recorded beyond the stored-detail cap; keeps [`count`](Self::count) exact.
    pub overflow: u32,
}

impl UnavailableSummary {
    /// Exact number of unavailability events in the turn.
    pub fn count(&self) -> u32 {
        (self.items.len() as u32).saturating_add(self.overflow)
    }
}

#[derive(Default)]
struct Inner {
    items: Vec<YoloBypass>,
    /// Exact number of bypasses recorded (>= `items.len()`).
    total: u32,
    session_id: String,
    surface: String,
    chat_id: String,
    unavailable: Vec<UnavailableEntry>,
    /// Exact number of unavailability events recorded (>= `unavailable.len()`).
    unavailable_total: u32,
    unavailable_session_id: String,
    unavailable_surface: String,
    unavailable_chat_id: String,
}

/// Turn-scoped accumulator of yolo bypasses.
#[derive(Default)]
pub struct YoloLedger {
    inner: Mutex<Inner>,
}

impl YoloLedger {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Poison-tolerant (the repo's pattern); the guard is never held across an await.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record one bypass. Synchronous and non-blocking beyond a short mutex hold.
    pub fn record(&self, tool: &str, reason: &str, session_id: &str, surface: &str, chat_id: &str) {
        let mut g = self.lock();
        if g.total == 0 {
            g.session_id = session_id.to_string();
            g.surface = surface.to_string();
            g.chat_id = chat_id.to_string();
        }
        g.total = g.total.saturating_add(1);
        if g.items.len() < YOLO_LEDGER_MAX_DETAIL {
            g.items.push(YoloBypass {
                tool: tool.to_string(),
                reason: reason.to_string(),
            });
        }
    }

    /// Take everything recorded so far. `None` when nothing was recorded.
    pub fn drain(&self) -> Option<YoloBypassSummary> {
        let mut g = self.lock();
        if g.total == 0 {
            return None;
        }
        let items = std::mem::take(&mut g.items);
        let total = std::mem::take(&mut g.total);
        let overflow = total.saturating_sub(items.len() as u32);
        Some(YoloBypassSummary {
            session_id: std::mem::take(&mut g.session_id),
            surface: std::mem::take(&mut g.surface),
            chat_id: std::mem::take(&mut g.chat_id),
            items,
            overflow,
        })
    }

    /// Exact number of bypasses recorded and not yet drained.
    pub fn recorded_count(&self) -> u32 {
        self.lock().total
    }

    /// Record one sidecar-unavailable event (Phase 57 D-12). Synchronous, like
    /// [`record`](Self::record). The per-event `sidecar_unavailable` audit row is the
    /// source of truth; this only feeds the per-turn rollup.
    #[allow(clippy::too_many_arguments)]
    pub fn record_unavailable(
        &self,
        tool: &str,
        reason: &str,
        level: u8,
        verdict: &str,
        session_id: &str,
        surface: &str,
        chat_id: &str,
    ) {
        let mut g = self.lock();
        if g.unavailable_total == 0 {
            g.unavailable_session_id = session_id.to_string();
            g.unavailable_surface = surface.to_string();
            g.unavailable_chat_id = chat_id.to_string();
        }
        g.unavailable_total = g.unavailable_total.saturating_add(1);
        if g.unavailable.len() < YOLO_LEDGER_MAX_DETAIL {
            g.unavailable.push(UnavailableEntry {
                tool: tool.to_string(),
                reason: reason.to_string(),
                level,
                verdict: verdict.to_string(),
            });
        }
    }

    /// Take every unavailability event recorded so far. `None` when there were none.
    pub fn drain_unavailable(&self) -> Option<UnavailableSummary> {
        let mut g = self.lock();
        if g.unavailable_total == 0 {
            return None;
        }
        let items = std::mem::take(&mut g.unavailable);
        let total = std::mem::take(&mut g.unavailable_total);
        let overflow = total.saturating_sub(items.len() as u32);
        Some(UnavailableSummary {
            session_id: std::mem::take(&mut g.unavailable_session_id),
            surface: std::mem::take(&mut g.unavailable_surface),
            chat_id: std::mem::take(&mut g.unavailable_chat_id),
            items,
            overflow,
        })
    }

    /// Exact number of unavailability events recorded and not yet drained.
    pub fn unavailable_count(&self) -> u32 {
        self.lock().unavailable_total
    }
}

tokio::task_local! {
    /// The current turn's ledger. Installed by `AgentRuntime::run_turn` around the agent
    /// run; absent everywhere else (callers then fall back to the per-call notice).
    pub static YOLO_LEDGER: Arc<YoloLedger>;
}

/// Record a bypass into the current turn's ledger, if one is in scope.
///
/// Returns `true` when recorded, `false` when no ledger is in scope. Uses `try_with`
/// (NOT `with`, which panics when the task-local is unset). See the module doc for the
/// propagation constraint that makes this valid.
pub fn try_record(
    tool: &str,
    reason: &str,
    session_id: &str,
    surface: &str,
    chat_id: &str,
) -> bool {
    YOLO_LEDGER
        .try_with(|l| l.record(tool, reason, session_id, surface, chat_id))
        .is_ok()
}

/// Audit `kind` of the one-per-turn rollup of sidecar-unavailable events (Phase 57 D-12).
pub const SIDECAR_UNAVAILABLE_ROLLUP_KIND: &str = "sidecar_unavailable_rollup";

/// Record a sidecar-unavailable event into the current turn's ledger, if one is in scope.
/// Returns `true` when recorded.
///
/// Uses `try_with` like [`try_record`]: a call outside any turn (a task-local does not
/// cross `tokio::spawn`) returns `false`, and the caller's per-event audit row is the
/// only record (research Pitfall 10).
pub fn try_record_unavailable(
    tool: &str,
    reason: &str,
    level: u8,
    verdict: &str,
    session_id: &str,
    surface: &str,
    chat_id: &str,
) -> bool {
    YOLO_LEDGER
        .try_with(|l| {
            l.record_unavailable(tool, reason, level, verdict, session_id, surface, chat_id)
        })
        .is_ok()
}

/// Flush the ledger at the turn boundary: ONE rollup audit row, then ONE bounded
/// aggregated notice. Best-effort throughout — returns `()` and never changes the
/// turn's outcome.
///
/// Order is load-bearing: the audit row is appended BEFORE the notifier is invoked, so
/// a notifier that hangs, panics or is slow can never cost the audit record.
pub async fn flush_yolo_ledger(
    ledger: &YoloLedger,
    audit_log: &AuditLog,
    notifier: Option<&dyn ApprovalGate>,
    turn_id: Option<uuid::Uuid>,
) {
    // a. Nothing recorded → no entry, no notifier call.
    if let Some(summary) = ledger.drain() {
        flush_bypass_rollup(&summary, audit_log, notifier, turn_id).await;
    }
    // Phase 57 D-12: the sidecar-unavailable rollup comes after the yolo rollup and is
    // independent of it (an outage with no bypass still flushes).
    if let Some(summary) = ledger.drain_unavailable() {
        flush_unavailable_rollup(&summary, audit_log, turn_id).await;
    }
}

/// Phase 57 D-12: ONE `sidecar_unavailable_rollup` row per turn. The per-event
/// `sidecar_unavailable` rows are untouched and remain the source of truth; no notifier is
/// involved because each event already reached the surface through
/// `ApprovalGate::notify_security_event`.
async fn flush_unavailable_rollup(
    summary: &UnavailableSummary,
    audit_log: &AuditLog,
    turn_id: Option<uuid::Uuid>,
) {
    let count = summary.count();
    let mut reasons: BTreeMap<&str, u32> = BTreeMap::new();
    let mut tools: BTreeMap<&str, u32> = BTreeMap::new();
    let mut verdicts: BTreeMap<&str, u32> = BTreeMap::new();
    for item in &summary.items {
        *reasons.entry(item.reason.as_str()).or_insert(0) += 1;
        *tools.entry(item.tool.as_str()).or_insert(0) += 1;
        *verdicts.entry(item.verdict.as_str()).or_insert(0) += 1;
    }
    let tool_field = tools.keys().copied().collect::<Vec<_>>().join(",");
    let mut raw_args = serde_json::json!({
        "count": count,
        "reasons": reasons,
        "tools": tools,
        "verdicts": verdicts,
        "turn_id": turn_id.map(|u| u.to_string()),
    });
    if summary.overflow > 0 {
        raw_args["detail_truncated"] = serde_json::json!(summary.overflow);
    }
    let entry = audit_log.make_entry(
        SIDECAR_UNAVAILABLE_ROLLUP_KIND,
        tool_field,
        None,
        summary.session_id.as_str(),
        summary.surface.as_str(),
        summary.chat_id.as_str(),
        format!("{count} security sidecar outage event(s) this turn"),
        "recorded",
        YOLO_ROLLUP_RESOLUTION,
        &raw_args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "ironhermes::security::yolo_ledger",
            error = %e,
            count,
            "failed to append sidecar_unavailable rollup audit entry"
        );
    }
}

/// The yolo-bypass rollup row and its bounded surface notice (Phase 54.3 D-05).
async fn flush_bypass_rollup(
    summary: &YoloBypassSummary,
    audit_log: &AuditLog,
    notifier: Option<&dyn ApprovalGate>,
    turn_id: Option<uuid::Uuid>,
) {
    let count = summary.count();

    // b. Rollup audit row first.
    let by_tool = summary.by_tool();
    let mut names: Vec<&str> = by_tool.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    let tool_field = names.join(",");
    let tools_map: BTreeMap<&str, u32> = by_tool.iter().map(|(n, c)| (n.as_str(), *c)).collect();
    let mut raw_args = serde_json::json!({
        "count": count,
        "tools": tools_map,
        "turn_id": turn_id.map(|u| u.to_string()),
    });
    if summary.overflow > 0 {
        // The per-tool map covers stored detail only; `count` stays exact.
        raw_args["detail_truncated"] = serde_json::json!(summary.overflow);
    }
    let entry = audit_log.make_entry(
        YOLO_ROLLUP_KIND,
        tool_field,
        None,
        summary.session_id.as_str(),
        summary.surface.as_str(),
        summary.chat_id.as_str(),
        format!("{count} tool call(s) ran without approval under yolo"),
        "allowed",
        YOLO_ROLLUP_RESOLUTION,
        &raw_args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "ironhermes::security::yolo_ledger",
            error = %e,
            count,
            "failed to append yolo rollup audit entry"
        );
    }

    // c. Only then the surface notice, bounded.
    match notifier {
        Some(n) => {
            let bounded = tokio::time::timeout(
                Duration::from_secs(YOLO_FLUSH_TIMEOUT_SECS),
                n.notify_yolo_bypass_turn(summary),
            )
            .await;
            if bounded.is_err() {
                tracing::warn!(
                    target: "ironhermes::security::yolo_ledger",
                    surface = %summary.surface,
                    count,
                    timeout_secs = YOLO_FLUSH_TIMEOUT_SECS,
                    "yolo bypass notifier did not complete in time; turn continues"
                );
            }
        }
        None => {
            // A surface that forgot to wire `TurnRequest.yolo_notifier` /
            // `approval_gate` — make that visible in logs rather than silent.
            tracing::warn!(
                target: "ironhermes::security::yolo_ledger",
                surface = %summary.surface,
                count,
                "yolo bypasses ran this turn but no notifier is wired for this surface; \
                 the operator was NOT told (audit rows were written)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::AuditConfig;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn read_rows(path: &std::path::Path) -> Vec<serde_json::Value> {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        // Stream-safe: audit.jsonl is not strictly line-delimited.
        serde_json::Deserializer::from_str(&contents)
            .into_iter::<serde_json::Value>()
            .map(|r| r.expect("audit stream parses"))
            .collect()
    }

    #[derive(Default)]
    struct CountingNotifier {
        turn_calls: AtomicU32,
    }

    #[async_trait::async_trait]
    impl ApprovalGate for CountingNotifier {
        async fn request_approval(
            &self,
            _s: &str,
            _t: &str,
            _r: &str,
            _a: &serde_json::Value,
        ) -> ironhermes_core::ApprovalOutcome {
            ironhermes_core::ApprovalOutcome::Denied
        }
        async fn resolve(&self, _s: &str, _a: bool) -> bool {
            false
        }
        async fn notify_yolo_bypass_turn(&self, _summary: &YoloBypassSummary) {
            self.turn_calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Behavior 2: with no `YOLO_LEDGER` scope installed, `try_record` reports "not
    /// recorded" so the caller falls through to the per-call notify (fail-visible).
    #[test]
    fn yolo_ledger_unscoped_try_record_reports_not_recorded() {
        assert!(YOLO_LEDGER.try_with(|_| ()).is_err(), "no scope installed");
        assert!(!try_record("terminal", "r", "s", "test", "c"));
    }

    #[tokio::test]
    async fn yolo_ledger_scoped_try_record_reports_recorded() {
        let ledger = Arc::new(YoloLedger::default());
        let recorded = YOLO_LEDGER
            .scope(ledger.clone(), async {
                try_record("terminal", "r", "s", "test", "c")
            })
            .await;
        assert!(recorded);
        assert_eq!(ledger.recorded_count(), 1);
    }

    /// Behavior 3: rollup row shape — one entry, kind/resolution/decision, `args.count`,
    /// and NO `resolution == "bypass"` row.
    #[tokio::test]
    async fn yolo_ledger_flush_writes_one_rollup_row_with_exact_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("audit.jsonl");
        let log = AuditLog::with_path(path.clone(), AuditConfig::default());
        let ledger = YoloLedger::default();
        ledger.record("terminal", "r1", "sess", "tui", "chat");
        ledger.record("execute_code", "r2", "sess", "tui", "chat");
        let notifier = CountingNotifier::default();

        flush_yolo_ledger(&ledger, &log, Some(&notifier), Some(uuid::Uuid::nil())).await;

        let rows = read_rows(&path);
        assert_eq!(rows.len(), 1, "exactly one entry: {rows:?}");
        let row = &rows[0];
        assert_eq!(row["kind"], YOLO_ROLLUP_KIND);
        assert_eq!(row["resolution"], "rollup");
        assert_eq!(row["decision"], "allowed");
        assert_eq!(row["tool"], "execute_code,terminal");
        let args: serde_json::Value = serde_json::from_str(row["args"].as_str().unwrap()).unwrap();
        assert_eq!(args["count"], 2);
        assert_eq!(args["tools"]["terminal"], 1);
        assert_eq!(args["tools"]["execute_code"], 1);
        assert!(
            rows.iter().all(|r| r["resolution"] != "bypass"),
            "a rollup must never carry resolution=\"bypass\": {rows:?}"
        );
        assert_eq!(notifier.turn_calls.load(Ordering::SeqCst), 1);
    }

    /// Behavior 4: an empty ledger writes nothing and calls no notifier.
    #[tokio::test]
    async fn yolo_ledger_flush_of_empty_ledger_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::with_path(path.clone(), AuditConfig::default());
        let notifier = CountingNotifier::default();

        flush_yolo_ledger(&YoloLedger::default(), &log, Some(&notifier), None).await;

        assert!(read_rows(&path).is_empty());
        assert_eq!(notifier.turn_calls.load(Ordering::SeqCst), 0);
    }

    /// T-54.3-03: capped detail, exact count.
    #[test]
    fn yolo_ledger_caps_stored_detail_but_keeps_the_count_exact() {
        let ledger = YoloLedger::default();
        let n = YOLO_LEDGER_MAX_DETAIL as u32 + 44;
        for _ in 0..n {
            ledger.record("terminal", "r", "s", "test", "c");
        }
        assert_eq!(ledger.recorded_count(), n);
        let summary = ledger.drain().expect("recorded");
        assert_eq!(summary.items.len(), YOLO_LEDGER_MAX_DETAIL);
        assert_eq!(summary.count(), n);
        assert!(ledger.drain().is_none(), "drain empties the ledger");
    }

    /// Without a notifier the rollup row is still written (warn-logged, not silent).
    #[tokio::test]
    async fn yolo_ledger_flush_without_notifier_still_writes_the_rollup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::with_path(path.clone(), AuditConfig::default());
        let ledger = YoloLedger::default();
        ledger.record("terminal", "r", "s", "test", "c");

        flush_yolo_ledger(&ledger, &log, None, None).await;

        assert_eq!(read_rows(&path).len(), 1);
    }
}
