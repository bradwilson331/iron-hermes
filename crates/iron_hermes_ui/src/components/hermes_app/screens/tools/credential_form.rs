//! Phase 48.2 Plan 07 (D-05/D-06/D-07/D-09/D-16/D-22): the inline
//! credential-entry form — the ONE mount point for setting/rotating/
//! clearing a tool credential, reused verbatim on the amber unmet-
//! prerequisite card (`catalog.rs`'s `.tool-fix-mount`) AND in the settings
//! panel's credentials section (`settings_panel.rs`). There is one
//! credential form on this page, not two.
//!
//! `MaskedPresenceRow` owns its own presence fetch and renders exactly two
//! states — `Not set` + `ADD`, or a fixed-length mask + tier + `ROTATE`/
//! `CLEAR` — and nothing else. `ToolCredentialForm` wraps it and owns the
//! reveal flow: opening `ADD`/`ROTATE` calls `get_credential_write_plan`
//! FIRST and renders its verdict (the plaintext warning and/or the
//! read-only-backend reason, REVIEWS finding 6) BEFORE the key input is
//! shown — never after a failed write.

use crate::server::tools_config_api::ConfigScope;
use crate::server::tools_credentials_api::{CredentialStorageTier, CredentialWritePlan};
use dioxus::prelude::*;

/// D-06: human-readable tier label for the masked-presence row. Never
/// rendered for `NotSet` (that state renders the `Not set` placeholder
/// instead, see [`MaskedPresenceRow`]).
#[allow(dead_code)] // used inside MaskedPresenceRow's rsx!; dead_code fires on test target (tools.rs TOOLS_CSS / catalog.rs is_mcp_group precedent)
fn tier_label(tier: &CredentialStorageTier) -> &'static str {
    match tier {
        CredentialStorageTier::Env => "ENV",
        CredentialStorageTier::Vault => "VAULT",
        CredentialStorageTier::ConfigPlaintext => "CONFIG (PLAINTEXT)",
        CredentialStorageTier::ProfileEnv => "PROFILE .ENV",
        CredentialStorageTier::NotSet => "",
    }
}

/// Pure builder behind [`CredentialNoticeBlock`] — the lines to render, in
/// order: the plaintext warning first, then (when present) the
/// read-only-backend reason. Empty when `plaintext_warning_required` is
/// `false` (the vault-tier case renders no notice at all). Kept pure and
/// disk-I/O-free so it is directly unit-testable without a Dioxus render
/// harness.
#[allow(dead_code)] // used inside CredentialNoticeBlock's rsx!; dead_code fires on test target (tools.rs TOOLS_CSS / catalog.rs is_mcp_group precedent)
fn notice_lines(plan: &CredentialWritePlan) -> Vec<String> {
    let mut lines = Vec::new();
    if plan.plaintext_warning_required {
        if let Some(warning) = &plan.warning {
            lines.push(warning.clone());
        }
        if let Some(reason) = &plan.vault_unavailable_reason {
            lines.push(reason.clone());
        }
    }
    lines
}

/// D-07/REVIEWS finding 6: the pre-input amber notice — plaintext warning,
/// and (only when the vault route was blocked by an unwritable backend
/// rather than a disabled vault) the reason naming the backend and the
/// config key to change, rendered alongside it so the operator gets both
/// facts: where the key is going, and why the vault did not take it.
#[component]
fn CredentialNoticeBlock(plan: CredentialWritePlan) -> Element {
    let lines = notice_lines(&plan);
    rsx! {
        for (i , line) in lines.iter().enumerate() {
            div { key: "{i}", class: "credential-notice", "{line}" }
        }
    }
}

/// D-06: presence + tier row. Owns its own `get_tool_credential_status`
/// fetch, keyed on `refresh_tick` so the parent form can force a refetch
/// after a successful write/clear without restarting a resource. Renders
/// EXACTLY two states and nothing else — never a blank masked field.
#[component]
fn MaskedPresenceRow(
    scope: ConfigScope,
    key_name: String,
    writable: bool,
    refresh_tick: ReadSignal<u32>,
    on_add: EventHandler<()>,
    on_rotate: EventHandler<()>,
    on_clear: EventHandler<()>,
) -> Element {
    let scope_for_fetch = scope.clone();
    let key_for_fetch = key_name.clone();
    let status_resource = use_resource(move || {
        let scope_value = scope_for_fetch.clone();
        let key_value = key_for_fetch.clone();
        let _tick = refresh_tick();
        async move {
            crate::server::tools_credentials_api::get_tool_credential_status(
                scope_value,
                key_value,
            )
            .await
        }
    });

    let status_value = status_resource();
    let is_loading = status_value.is_none();
    let load_error: Option<String> = match &status_value {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let present = matches!(&status_value, Some(Ok(s)) if s.present);
    let masked = match &status_value {
        Some(Ok(s)) => s.masked.clone(),
        _ => String::new(),
    };
    let tier_text = match &status_value {
        Some(Ok(s)) => tier_label(&s.tier).to_string(),
        _ => String::new(),
    };
    let key_for_label = key_name.clone();
    let key_for_label_rotate = key_name.clone();
    let key_for_label_clear = key_name.clone();

    rsx! {
        div { class: "credential-row",
            span { class: "credential-row-key", "{key_name}" }
            if is_loading {
                span { class: "credential-row-loading", "aria-hidden": "true", "···" }
            } else if let Some(err) = load_error {
                span { class: "pill red", "COULD NOT LOAD — {err}." }
            } else if present {
                span { class: "credential-row-value", "{masked}" }
                span { class: "credential-row-tier", "{tier_text}" }
                if writable {
                    button {
                        r#type: "button",
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Rotate credential for {key_for_label_rotate}",
                        onclick: move |_| on_rotate.call(()),
                        "ROTATE"
                    }
                    button {
                        r#type: "button",
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Clear credential for {key_for_label_clear}",
                        onclick: move |_| on_clear.call(()),
                        "CLEAR"
                    }
                }
            } else {
                // credential-form/empty: absence-of-presence-indicator,
                // never a blank masked field (D-06).
                span { class: "credential-row-value credential-row-value--unset", "Not set" }
                if writable {
                    button {
                        r#type: "button",
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Add credential for {key_for_label}",
                        onclick: move |_| on_add.call(()),
                        "ADD"
                    }
                }
            }
        }
    }
}

/// The one credential form mounted on this page — the amber card's
/// `.tool-fix-mount` slot and the settings panel's credentials section both
/// mount this SAME component (D-16). `on_saved` fires after a successful
/// write OR clear so the caller can bump its own `refresh_tick` (the amber
/// card leaves its state on the next catalog read with no server restart).
#[component]
pub fn ToolCredentialForm(
    scope: ConfigScope,
    key_name: String,
    writable: bool,
    on_saved: EventHandler<()>,
) -> Element {
    let mut refresh_tick: Signal<u32> = use_signal(|| 0);
    let mut revealed: Signal<bool> = use_signal(|| false);
    let mut write_plan: Signal<Option<CredentialWritePlan>> = use_signal(|| None);
    let mut plan_error: Signal<Option<String>> = use_signal(|| None);
    let mut input_value: Signal<String> = use_signal(String::new);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);

    // Opening ADD/ROTATE calls get_credential_write_plan FIRST and renders
    // its verdict before the input is revealed (REVIEWS finding 6) — this
    // call is reached before any key-input rsx! node below.
    let open_scope = scope.clone();
    let open_key = key_name.clone();
    let open_form = move || {
        if !writable {
            return;
        }
        let scope_value = open_scope.clone();
        let key_value = open_key.clone();
        plan_error.set(None);
        spawn(async move {
            match crate::server::tools_credentials_api::get_credential_write_plan(
                scope_value,
                key_value,
            )
            .await
            {
                Ok(plan) => {
                    write_plan.set(Some(plan));
                    revealed.set(true);
                }
                Err(e) => {
                    plan_error.set(Some(e.to_string()));
                }
            }
        });
    };

    let save_scope = scope.clone();
    let save_key = key_name.clone();
    let mut do_save = move || {
        if *saving.peek() {
            return;
        }
        let value = input_value.peek().clone();
        if value.trim().is_empty() {
            return;
        }
        let scope_value = save_scope.clone();
        let key_value = save_key.clone();
        saving.set(true);
        save_error.set(None);
        spawn(async move {
            match crate::server::tools_credentials_api::set_tool_credential(
                scope_value,
                key_value,
                value,
            )
            .await
            {
                Ok(()) => {
                    saving.set(false);
                    input_value.set(String::new());
                    revealed.set(false);
                    write_plan.set(None);
                    let next_tick = *refresh_tick.peek() + 1;
                    refresh_tick.set(next_tick);
                    on_saved.call(());
                }
                Err(e) => {
                    // credential-form/error + partial: the presence
                    // indicator does not flip to "set" until the write
                    // actually succeeds, and the form stays open (an
                    // explicit RETRY is just clicking SAVE again) rather
                    // than silently falling back to a different tier.
                    saving.set(false);
                    save_error.set(Some(e.to_string()));
                }
            }
        });
    };

    let clear_scope = scope.clone();
    let clear_key = key_name.clone();
    let do_clear = move || {
        let scope_value = clear_scope.clone();
        let key_value = clear_key.clone();
        save_error.set(None);
        spawn(async move {
            match crate::server::tools_credentials_api::clear_tool_credential(
                scope_value,
                key_value,
            )
            .await
            {
                Ok(()) => {
                    revealed.set(false);
                    write_plan.set(None);
                    let next_tick = *refresh_tick.peek() + 1;
                    refresh_tick.set(next_tick);
                    on_saved.call(());
                }
                Err(e) => {
                    save_error.set(Some(e.to_string()));
                }
            }
        });
    };

    let revealed_val = *revealed.read();
    let plan_val = write_plan.read().clone();
    let plan_error_val = plan_error.read().clone();
    let input_val = input_value.read().clone();
    let saving_val = *saving.read();
    let save_error_val = save_error.read().clone();
    let key_for_aria = key_name.clone();
    let key_for_aria_save = key_name.clone();
    let tick_read: ReadSignal<u32> = refresh_tick.into();

    let mut open_form_for_add = open_form.clone();
    let mut open_form_for_rotate = open_form;
    let mut do_clear_for_row = do_clear.clone();

    rsx! {
        div { class: "credential-form",
            MaskedPresenceRow {
                scope: scope.clone(),
                key_name: key_name.clone(),
                writable,
                refresh_tick: tick_read,
                on_add: move |_| open_form_for_add(),
                on_rotate: move |_| open_form_for_rotate(),
                on_clear: move |_| do_clear_for_row(),
            }
            if let Some(err) = plan_error_val {
                div { class: "pill red tools-save-failed-chip", "SAVE FAILED — key not stored — {err}." }
            }
            if revealed_val {
                if let Some(plan) = plan_val.clone() {
                    CredentialNoticeBlock { plan }
                }
                div { class: "credential-input-row",
                    input {
                        r#type: "password",
                        class: "credential-input",
                        "aria-label": "New value for {key_for_aria}",
                        value: "{input_val}",
                        oninput: move |evt| input_value.set(evt.value()),
                    }
                    button {
                        r#type: "button",
                        class: "btn btn--primary btn--sm",
                        disabled: saving_val || input_val.trim().is_empty(),
                        "aria-label": "Save credential for {key_for_aria_save}",
                        onclick: move |_| do_save(),
                        if saving_val { "SAVING…" } else { "SAVE" }
                    }
                }
                if let Some(err) = save_error_val {
                    div { class: "pill red tools-save-failed-chip", "SAVE FAILED — key not stored — {err}." }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_lines_renders_both_when_plaintext_and_vault_unavailable() {
        let plan = CredentialWritePlan {
            tier: CredentialStorageTier::ConfigPlaintext,
            plaintext_warning_required: true,
            warning: Some("warn text".to_string()),
            vault_unavailable_reason: Some("reason text".to_string()),
        };
        let lines = notice_lines(&plan);
        assert_eq!(lines, vec!["warn text".to_string(), "reason text".to_string()]);
    }

    #[test]
    fn notice_lines_renders_only_warning_when_no_reason() {
        let plan = CredentialWritePlan {
            tier: CredentialStorageTier::ConfigPlaintext,
            plaintext_warning_required: true,
            warning: Some("warn text".to_string()),
            vault_unavailable_reason: None,
        };
        assert_eq!(notice_lines(&plan), vec!["warn text".to_string()]);
    }

    #[test]
    fn notice_lines_empty_for_vault_tier() {
        let plan = CredentialWritePlan {
            tier: CredentialStorageTier::Vault,
            plaintext_warning_required: false,
            warning: None,
            vault_unavailable_reason: None,
        };
        assert!(notice_lines(&plan).is_empty());
    }

    #[test]
    fn tier_label_covers_every_variant_without_panicking() {
        assert_eq!(tier_label(&CredentialStorageTier::Env), "ENV");
        assert_eq!(tier_label(&CredentialStorageTier::Vault), "VAULT");
        assert_eq!(
            tier_label(&CredentialStorageTier::ConfigPlaintext),
            "CONFIG (PLAINTEXT)"
        );
        assert_eq!(tier_label(&CredentialStorageTier::ProfileEnv), "PROFILE .ENV");
        assert_eq!(tier_label(&CredentialStorageTier::NotSet), "");
    }
}
