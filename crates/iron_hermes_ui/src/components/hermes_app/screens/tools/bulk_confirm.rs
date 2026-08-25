//! Phase 48.2 Plan 06 Task 3 (D-17): the ENABLE ALL / DISABLE ALL confirm
//! dialog that wires up the Tools header's bulk-toggle buttons. The dialog
//! body names EXACTLY what will flip, computed client-side from the
//! already-loaded catalog (`ToolsPageState::bulk_targets`, Task 2 — a
//! `cfg(not(target_arch = "wasm32"))`-only fn computed it server-side, so
//! the client reads the already-fetched field rather than recomputing it,
//! and no fetch backs the confirm).
//!
//! # Never report a partial write as a full success
//!
//! `set_toolsets_bulk` is a single atomic rewrite — a failure leaves
//! nothing applied UNLESS the transport itself failed AFTER a real write
//! landed. On any error the dialog closes and the page shows a per-toolset
//! reconciliation (`reconcile_bulk_failure`) against freshly re-fetched
//! catalog state, rather than assuming the whole operation failed.

use crate::server::tools_config_api::{BulkToggleOutcome, ConfigScope, ToolsetGroup};
use dioxus::prelude::*;

/// Which bulk action this dialog confirms.
#[allow(dead_code)] // constructed inside ScreenTools rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BulkAction {
    EnableAll,
    DisableAll,
}

/// Handed to the parent (`ScreenTools`) via `on_result` once a submission
/// completes — rendered AFTER the dialog closes (must_haves:
/// bulk-confirm-dialog/error — the dialog closes and the PAGE shows the
/// per-toolset result, not the dialog itself).
#[allow(dead_code)] // constructed inside BulkConfirmDialog's spawn future; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
#[derive(Clone, Debug, PartialEq)]
pub enum BulkOutcomeReport {
    /// A full success — the returned per-toolset outcome list is the
    /// complete, authoritative truth (single atomic write, D-17).
    Success(Vec<BulkToggleOutcome>),
    /// The server fn call errored. Because the write is a single atomic
    /// rewrite, `targets`/`enable` describe what the operator INTENDED —
    /// the parent reconciles against freshly re-fetched catalog state
    /// (triggered by the same `refresh_tick` bump) via
    /// [`reconcile_bulk_failure`] to learn which targets actually landed
    /// vs. a transport-level failure after a real write.
    Failed {
        targets: Vec<String>,
        enable: bool,
        reason: String,
    },
}

/// DISABLE ALL confirm body (D-17 copywriting contract, verbatim): names
/// the exact count and every target toolset name, calls out the MCP
/// exemption. `targets` is `ToolsPageState::bulk_targets` — already
/// excludes MCP display groups and both sentinel toolsets by construction
/// (Task 2's `bulk_targets`), so this fn never needs to filter or
/// special-case those names itself. The list is rendered as ordinary text
/// (no `white-space: nowrap` in `tools.css`) so it wraps onto multiple
/// lines rather than truncating (UI-SPEC bulk-confirm-dialog/overflow).
#[allow(dead_code)] // used inside BulkConfirmDialog rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
pub fn disable_all_body(targets: &[String]) -> String {
    format!(
        "DISABLE ALL — this turns off {} toolsets: {}. MCP servers are not affected.",
        targets.len(),
        targets.join(", ")
    )
}

/// ENABLE ALL confirm body (D-17 copywriting contract, VERBATIM per
/// UI-SPEC — never truncated, never paraphrased, D-10 convention). The
/// three high-blast-radius names are the locked copy's literal text, not
/// interpolated from `HIGH_BLAST_RADIUS_TOOLSETS` — the constant instead
/// backs a test (`high_blast_radius_toolsets_names_match_the_locked_copy`)
/// asserting this literal text can never silently drift from the constant.
#[allow(dead_code)] // used inside BulkConfirmDialog rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
pub fn enable_all_body(targets: &[String]) -> String {
    format!(
        "ENABLE ALL — this turns on {} toolsets, including high-blast-radius ones: BROWSER, CODE, WEB (default-off). Confirm you want them all enabled.",
        targets.len()
    )
}

/// Reconcile a bulk-write TRANSPORT failure against fresh catalog state
/// (D-17 must_haves: never report a partial write as a full success). For
/// each originally-intended target, compares its live enabled state to
/// what was requested — `true` means it landed despite the transport
/// error, `false` means the write never reached disk for this toolset. A
/// target no longer present in `live_toolsets` (e.g. removed) is reported
/// as NOT applied, never silently dropped from the report.
#[allow(dead_code)] // used inside BulkResultBanner rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
pub fn reconcile_bulk_failure(
    targets: &[String],
    requested_enable: bool,
    live_toolsets: &[ToolsetGroup],
) -> Vec<(String, bool)> {
    targets
        .iter()
        .map(|name| {
            let live_enabled = live_toolsets
                .iter()
                .find(|g| &g.name == name)
                .map(|g| g.enabled)
                .unwrap_or(!requested_enable);
            (name.clone(), live_enabled == requested_enable)
        })
        .collect()
}

/// The confirm dialog itself — names exactly what will flip, submits
/// `set_toolsets_bulk` on confirm, and ALWAYS closes (success or failure)
/// handing the result to the parent via `on_result` for page-level
/// rendering. Every control carries an accessible label.
#[component]
pub fn BulkConfirmDialog(
    action: BulkAction,
    targets: Vec<String>,
    scope: ConfigScope,
    mut refresh_tick: Signal<u32>,
    on_result: EventHandler<BulkOutcomeReport>,
    on_close: EventHandler<()>,
) -> Element {
    let mut submitting: Signal<bool> = use_signal(|| false);

    let body = match action {
        BulkAction::DisableAll => disable_all_body(&targets),
        BulkAction::EnableAll => enable_all_body(&targets),
    };
    let title = match action {
        BulkAction::DisableAll => "DISABLE ALL?",
        BulkAction::EnableAll => "ENABLE ALL?",
    };
    let enable = matches!(action, BulkAction::EnableAll);
    let confirm_label = if enable { "ENABLE ALL" } else { "DISABLE ALL" };
    let confirm_class = if enable { "btn btn--sm" } else { "btn btn--danger btn--sm" };
    let is_submitting = *submitting.read();
    let targets_for_submit = targets.clone();
    let scope_for_submit = scope.clone();

    rsx! {
        div { class: "bulk-confirm-overlay", role: "presentation",
            div {
                class: "bulk-confirm-dialog",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "bulk-confirm-title",
                div { class: "bulk-confirm-title", id: "bulk-confirm-title", "{title}" }
                div { class: "bulk-confirm-body", "{body}" }
                div { class: "bulk-confirm-actions",
                    button {
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Cancel bulk toolset change",
                        disabled: is_submitting,
                        onclick: move |_| on_close.call(()),
                        "CANCEL"
                    }
                    button {
                        class: confirm_class,
                        "aria-label": "Confirm {confirm_label}",
                        disabled: is_submitting,
                        onclick: move |_| {
                            if is_submitting {
                                return;
                            }
                            submitting.set(true);
                            let scope_value = scope_for_submit.clone();
                            let targets_value = targets_for_submit.clone();
                            let mut submitting_sig = submitting;
                            let mut refresh_tick_sig = refresh_tick;
                            let on_result_handler = on_result;
                            let on_close_handler = on_close;
                            spawn(async move {
                                let report = match crate::server::tools_config_api::set_toolsets_bulk(
                                    scope_value,
                                    enable,
                                )
                                    .await
                                {
                                    Ok(outcomes) => BulkOutcomeReport::Success(outcomes),
                                    Err(e) => BulkOutcomeReport::Failed {
                                        targets: targets_value,
                                        enable,
                                        reason: e.to_string(),
                                    },
                                };
                                submitting_sig.set(false);
                                let cur = *refresh_tick_sig.read();
                                refresh_tick_sig.set(cur + 1);
                                on_result_handler.call(report);
                                on_close_handler.call(());
                            });
                        },
                        if is_submitting { "APPLYING…" } else { "{confirm_label}" }
                    }
                }
            }
        }
    }
}

/// The page-level, post-dialog result banner (must_haves:
/// bulk-confirm-dialog/error — never a partial success reported as a full
/// success). A `Success` report lists every outcome directly; a `Failed`
/// report reconciles against `live_toolsets` (the freshly re-fetched
/// catalog, since `refresh_tick` already bumped before this renders).
#[component]
pub fn BulkResultBanner(
    report: BulkOutcomeReport,
    live_toolsets: Vec<ToolsetGroup>,
    on_dismiss: EventHandler<()>,
) -> Element {
    match report {
        BulkOutcomeReport::Success(outcomes) => rsx! {
            div { class: "bulk-result-banner",
                div { class: "bulk-result-heading", "BULK UPDATE APPLIED" }
                div { class: "bulk-result-list",
                    for outcome in outcomes.iter().cloned() {
                        div {
                            key: "{outcome.toolset}",
                            class: "bulk-result-row bulk-result-row--ok",
                            span { class: "bulk-result-name", "{outcome.toolset}" }
                            span { class: "bulk-result-state",
                                if outcome.enabled { "ENABLED" } else { "DISABLED" }
                            }
                        }
                    }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    "aria-label": "Dismiss bulk update result",
                    onclick: move |_| on_dismiss.call(()),
                    "DISMISS"
                }
            }
        },
        BulkOutcomeReport::Failed { targets, enable, reason } => {
            let reconciled = reconcile_bulk_failure(&targets, enable, &live_toolsets);
            rsx! {
                div { class: "bulk-result-banner bulk-result-banner--error",
                    div { class: "bulk-result-heading", "SAVE FAILED — {reason}." }
                    div { class: "bulk-result-list",
                        for (name, succeeded) in reconciled {
                            div {
                                key: "{name}",
                                class: if succeeded { "bulk-result-row bulk-result-row--ok" } else { "bulk-result-row bulk-result-row--failed" },
                                span { class: "bulk-result-name", "{name}" }
                                span { class: "bulk-result-state",
                                    if succeeded { "APPLIED" } else { "NOT APPLIED" }
                                }
                            }
                        }
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Dismiss bulk update result",
                        onclick: move |_| on_dismiss.call(()),
                        "DISMISS"
                    }
                }
            }
        }
    }
}

// =============================================================================
// Pure-fn tests — Phase 48.2 Plan 06 Task 3 (D-17).
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tools_config_api::ToolsetKind;

    fn builtin_group(name: &str, enabled: bool) -> ToolsetGroup {
        ToolsetGroup {
            name: name.to_string(),
            kind: ToolsetKind::BuiltIn,
            enabled,
            tools: vec![],
        }
    }

    fn mcp_group(name: &str, server: &str) -> ToolsetGroup {
        ToolsetGroup {
            name: name.to_string(),
            kind: ToolsetKind::Mcp { server: server.to_string() },
            enabled: true,
            tools: vec![],
        }
    }

    /// A pure-function unit test of the confirm-body builder: the DISABLE
    /// ALL body contains the correct count and every target toolset name,
    /// and contains neither an `mcp__`-prefixed group name nor the bare
    /// `mcp` or `kanban` sentinel names even when all three are present in
    /// the catalog (exercised end-to-end through the real `bulk_targets`,
    /// which this body-builder trusts).
    #[test]
    fn disable_all_body_names_every_target_and_excludes_mcp_and_sentinels() {
        let catalog = vec![
            builtin_group("web", true),
            builtin_group("code", true),
            builtin_group("agent", true),
            mcp_group("mcp__github", "github"),
            builtin_group("mcp", true),
            builtin_group("kanban", true),
        ];
        let targets = crate::server::tools_config_api::bulk_targets(&catalog);
        let body = disable_all_body(&targets);

        assert!(
            body.contains(&format!("{} toolsets", targets.len())),
            "body must name the correct count; got: {body}"
        );
        for target in &targets {
            assert!(body.contains(target), "body must name {target}; got: {body}");
        }
        assert!(!body.contains("mcp__"), "body must never name an mcp__-prefixed group; got: {body}");
        assert!(
            !body.split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|word| word == "mcp"),
            "body must never name the bare mcp sentinel; got: {body}"
        );
        assert!(
            !body.split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|word| word == "kanban"),
            "body must never name the kanban sentinel; got: {body}"
        );
        assert!(body.contains("MCP servers are not affected."));
    }

    /// A pure-function unit test asserts the ENABLE ALL body names all
    /// three high-blast-radius toolsets.
    #[test]
    fn enable_all_body_names_all_three_high_blast_radius_toolsets() {
        let targets = vec!["agent".to_string(), "memory".to_string()];
        let body = enable_all_body(&targets);
        assert!(body.contains("BROWSER"));
        assert!(body.contains("CODE"));
        assert!(body.contains("WEB"));
        assert!(body.contains("Confirm you want them all enabled."));
        assert!(body.contains(&format!("{} toolsets", targets.len())));
    }

    #[test]
    fn reconcile_bulk_failure_reports_applied_when_live_state_matches_request() {
        let live = vec![builtin_group("web", true), builtin_group("code", false)];
        let targets = vec!["web".to_string(), "code".to_string()];
        let reconciled = reconcile_bulk_failure(&targets, true, &live);
        let web = reconciled.iter().find(|(n, _)| n == "web").unwrap();
        assert!(web.1, "web now matches the requested enabled:true — must report applied");
        let code = reconciled.iter().find(|(n, _)| n == "code").unwrap();
        assert!(!code.1, "code does not match the requested enabled:true — must report NOT applied");
    }

    #[test]
    fn reconcile_bulk_failure_missing_target_reports_not_applied() {
        let live: Vec<ToolsetGroup> = vec![];
        let targets = vec!["web".to_string()];
        let reconciled = reconcile_bulk_failure(&targets, true, &live);
        assert!(!reconciled[0].1, "a target absent from live state must report NOT applied");
    }

    /// `enable_all_body`'s locked copy names `HIGH_BLAST_RADIUS_TOOLSETS`
    /// literally (BROWSER, CODE, WEB) rather than interpolating the
    /// constant — this pins that the literal text can never silently drift
    /// from the constant it documents.
    #[test]
    fn high_blast_radius_toolsets_names_match_the_locked_copy() {
        assert_eq!(
            crate::server::tools_config_api::HIGH_BLAST_RADIUS_TOOLSETS,
            ["browser", "code", "web"]
        );
        let body = enable_all_body(&[]);
        assert!(body.contains("BROWSER, CODE, WEB"));
    }
}
