//! Phase 46.9 Plan 05 (D-02) — Kanban config embedded panel.
//!
//! Brand-new read+write panel for `ironhermes_kanban::KanbanConfig` (there is
//! no prior stub — RESEARCH Pitfall 3). Modeled on `voice_settings.rs` (the
//! full read+write embedded-panel template): `use_resource` read on mount +
//! a `loaded` latch populate editable signals exactly once (Pitfall 3 loop
//! avoidance — never re-seed from a live resource poll after the user starts
//! editing), always showing resolved `KanbanConfig::default()` values for
//! absent keys (never a blank/unset-looking form), and a distinct ghost
//! `.field-row` loading state.
//!
//! Deviation from the plan's literal `use_server_future(...)?` phrasing:
//! `use_server_future`'s `?` operator bubbles a Suspense signal that replaces
//! this panel's entire subtree with a Suspense fallback while pending — it
//! cannot coexist with a *distinct in-panel ghost-row* loading state (a
//! must_haves truth). `voice_settings.rs`, the template this plan cites,
//! uses `use_resource` + a `loaded` latch for exactly this reason; this file
//! follows that same working precedent (Rule 1 — bug/behavior mismatch fix
//! within Task 2's own scope).
//!
//! `Config.kanban` (`serde_yaml::Value`) is never touched directly here — all
//! reads/writes go through `get_kanban_config`/`update_kanban_config`
//! (`server/api.rs`), which own the `serde_yaml::from_value`/`to_value`
//! round-trip against `KanbanConfig` (Pitfall 5).

use dioxus::prelude::*;

use crate::server::api::{get_kanban_config, update_kanban_config, KanbanWritePayload};

/// Embedded Kanban config panel — mounted inside `screens/settings.rs` as a
/// titled `.panel` (voice-settings-embed precedent), NOT a new top-level
/// screen/route/wheel entry.
#[component]
pub fn KanbanConfigPanel() -> Element {
    // ── Editable signals (Pattern B: owned locals dropped before spawn) ──────
    let mut dispatch_in_gateway_sig = use_signal(|| true);
    let mut dispatch_interval_sig = use_signal(|| 60u64);
    // Text field so the user can leave it truly empty (renders as "0" /
    // unlimited on save) without fighting a numeric input's min/parse quirks.
    let mut max_in_progress_sig = use_signal(|| "8".to_string());
    let mut stale_timeout_sig = use_signal(|| 14_400u64); // 4h default (reference.md)
    let mut notification_sources_sig = use_signal(String::new);

    // ── Load-latch: populate editable signals once per mount (Pitfall 3) ────
    let mut loaded = use_signal(|| false);
    let mut write_enabled = use_signal(|| false);
    let config_resource = use_resource(move || async move { get_kanban_config().await });

    if !*loaded.read() {
        if let Some(Ok(snapshot)) = config_resource.read().as_ref() {
            dispatch_in_gateway_sig.set(snapshot.dispatch_in_gateway);
            dispatch_interval_sig.set(snapshot.dispatch_interval_seconds);
            max_in_progress_sig.set(snapshot.max_in_progress.unwrap_or(0).to_string());
            stale_timeout_sig.set(snapshot.dispatch_stale_timeout_seconds);
            notification_sources_sig.set(
                snapshot
                    .notification_sources
                    .clone()
                    .unwrap_or_default()
                    .join(", "),
            );
            write_enabled.set(snapshot.web_config_write_enabled);
            loaded.set(true);
        }
    }

    let is_loading = config_resource.read().is_none();
    let load_failed = matches!(config_resource.read().as_ref(), Some(Err(_)));

    // ── Save lifecycle ────────────────────────────────────────────────────
    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut save_ok = use_signal(|| false);

    let dispatch_in_gateway_val = *dispatch_in_gateway_sig.read();
    let dispatch_interval_val = *dispatch_interval_sig.read();
    let max_in_progress_val = max_in_progress_sig.read().clone();
    let stale_timeout_val = *stale_timeout_sig.read();
    let notification_sources_val = notification_sources_sig.read().clone();
    let write_enabled_val = *write_enabled.read();
    let is_saving = *saving.read();
    let save_error_val = save_error.read().clone();
    let save_ok_val = *save_ok.read();
    let can_save = write_enabled_val && !is_saving;

    rsx! {
        div {
            class: "kanban-config-panel",

            if is_loading {
                // Ghost loading state — distinct from the always-populated resolved
                // state below (must_haves truth: never blank/unset-looking).
                for _ in 0..4 {
                    div { class: "field-row is-loading", style: "opacity: 0.35;",
                        div { class: "field-label", "\u{00a0}" }
                        div { class: "field-input", style: "height: 22px;" }
                    }
                }
            } else if load_failed {
                p { style: "color:var(--red);font-size:12px;margin:0;",
                    "Could not load kanban config."
                }
                p { style: "color:var(--gray);font-size:11px;margin:4px 0 0 0;",
                    "Check the server connection and retry."
                }
            } else {
                // ── Dispatch in gateway (bool toggle) ─────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Dispatch in gateway"
                        span { class: "help", "run the dispatcher tick inside the gateway runtime" }
                    }
                    button {
                        class: "btn btn--sm",
                        class: if dispatch_in_gateway_val { "is-active" },
                        disabled: !write_enabled_val,
                        onclick: move |_| {
                            save_error.set(None);
                            dispatch_in_gateway_sig.set(!dispatch_in_gateway_val);
                        },
                        if dispatch_in_gateway_val { "ON" } else { "OFF" }
                    }
                }

                // ── Dispatch interval seconds ─────────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Dispatch interval"
                        span { class: "help", "dispatcher tick period, in seconds" }
                    }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "1",
                        max: "86400",
                        disabled: !write_enabled_val,
                        value: "{dispatch_interval_val}",
                        oninput: move |evt| {
                            save_error.set(None);
                            if let Ok(v) = evt.value().parse::<u64>() {
                                dispatch_interval_sig.set(v);
                            }
                        },
                    }
                }

                // ── Max in progress ────────────────────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Max in progress"
                        span { class: "help", "concurrency cap for running tasks — 0 = unlimited" }
                    }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "100000",
                        disabled: !write_enabled_val,
                        value: "{max_in_progress_val}",
                        oninput: move |evt| {
                            save_error.set(None);
                            max_in_progress_sig.set(evt.value());
                        },
                    }
                }

                // ── Dispatch stale timeout seconds ────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Stale timeout"
                        span { class: "help", "seconds before a heartbeat-less running task resets to ready" }
                    }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "1",
                        max: "604800",
                        disabled: !write_enabled_val,
                        value: "{stale_timeout_val}",
                        oninput: move |evt| {
                            save_error.set(None);
                            if let Ok(v) = evt.value().parse::<u64>() {
                                stale_timeout_sig.set(v);
                            }
                        },
                    }
                }

                // ── Notification sources ──────────────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Notification sources"
                        span { class: "help", "comma-separated profile names (reserved — not consumed in v1)" }
                    }
                    input {
                        class: "field-input",
                        r#type: "text",
                        disabled: !write_enabled_val,
                        placeholder: "e.g. default, ops",
                        value: "{notification_sources_val}",
                        oninput: move |evt| {
                            save_error.set(None);
                            notification_sources_sig.set(evt.value());
                        },
                    }
                }

                // ── Save ────────────────────────────────────────────────────
                div { class: "field-row",
                    button {
                        class: "btn btn--sm",
                        disabled: !can_save,
                        title: if !write_enabled_val { "Config writes are disabled" } else { "" },
                        onclick: move |_| {
                            // Pattern B: owned locals read before spawn — no signal
                            // borrow across .await.
                            let dispatch_in_gateway_local = *dispatch_in_gateway_sig.read();
                            let dispatch_interval_local = *dispatch_interval_sig.read();
                            let max_in_progress_raw = max_in_progress_sig.read().clone();
                            let stale_timeout_local = *stale_timeout_sig.read();
                            let notification_sources_raw = notification_sources_sig.read().clone();

                            let max_in_progress_local = max_in_progress_raw
                                .trim()
                                .parse::<usize>()
                                .unwrap_or(0);
                            let notification_sources_local: Vec<String> = notification_sources_raw
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();

                            saving.set(true);
                            save_error.set(None);
                            save_ok.set(false);
                            spawn(async move {
                                let payload = KanbanWritePayload {
                                    dispatch_in_gateway: Some(dispatch_in_gateway_local),
                                    dispatch_interval_seconds: Some(dispatch_interval_local),
                                    max_in_progress: Some(max_in_progress_local),
                                    dispatch_stale_timeout_seconds: Some(stale_timeout_local),
                                    notification_sources: if notification_sources_local.is_empty() {
                                        None
                                    } else {
                                        Some(notification_sources_local)
                                    },
                                };
                                match update_kanban_config(payload).await {
                                    Ok(()) => {
                                        saving.set(false);
                                        save_ok.set(true);
                                        gloo_timers::future::TimeoutFuture::new(1500).await;
                                        save_ok.set(false);
                                    }
                                    Err(_e) => {
                                        saving.set(false);
                                        save_error.set(Some("Save failed. Check server logs.".to_string()));
                                    }
                                }
                            });
                        },
                        if is_saving {
                            "SAVING…"
                        } else if save_ok_val {
                            "SAVED"
                        } else {
                            "SAVE CONFIG"
                        }
                    }
                }

                if let Some(err) = save_error_val {
                    p { style: "color:var(--red);font-size:11px;margin:0;", "{err}" }
                }

                if !write_enabled_val {
                    p { class: "help", style: "color:var(--gray);",
                        "Config writes are disabled."
                    }
                    p { class: "help", style: "color:var(--gray);",
                        "Set security.web_config_write_enabled: true in config.yaml, then retry."
                    }
                }
            }
        }
    }
}
