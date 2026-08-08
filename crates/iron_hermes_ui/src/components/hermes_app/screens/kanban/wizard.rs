//! Phase 47.4 Plan 07 (D-01/D-06/D-07/D-08/D-09/D-13): the Create Kanban
//! Profile wizard — the phase's headline surface (D-01). Three steps:
//! IDENTITY (client-side name validation, UX only), CONFIG & KEYS (three
//! key-inheritance modes plus a manual fallback, no secret-storage branch
//! per D-06), VERIFY (the real D-09 probe's four states).
//!
//! Client-side rsx! here references ONLY `protocol.rs` DTOs and `#[server]`
//! fn signatures — never a native-only crate, since this file compiles to
//! wasm. Step 1's validation therefore duplicates (does not call) the
//! server's real name-rule fn; the server independently re-validates
//! before any write (T-47.4-07-T1).
//!
//! This file must never call Dioxus's resource-restart method — doing so
//! after a resource-driven `?` early return breaks hook ordering for every
//! signal declared afterward. It must never introduce a shared-state
//! provider scoped to this component — such a provider compiles green and
//! panics its consumers at runtime; shared state belongs only at the
//! `HermesApp` root. And it must never call the native-only profile-name
//! validator directly — that function does not exist on the wasm target.

use crate::protocol::{CreateProfileRequest, KeyMode, KeyStatus, VerifyOutcome, VerifyReport};
use dioxus::prelude::*;

// ============================================================================
// Pure helpers (unit-tested — Task 1 <behavior>)
// ============================================================================

/// Mirrors the server's five-name LLM allowlist for a client-only "does
/// this profile have an LLM-family key yet" check (D-07/D-08). Duplicated
/// deliberately: this file must link no native-only crate (wasm target),
/// and the allowlist is five short literals, not logic — see this file's
/// own zero-native-import acceptance check.
#[allow(dead_code)] // used in CreateProfileWizard rsx!; dead_code fires under --all-features (legacy-shell swaps the reachable root component, mirrors Plan 01's identical precedent)
const CLIENT_LLM_KEY_ALLOWLIST: [&str; 5] = [
    "OPENROUTER_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GROQ_API_KEY",
    "OLLAMA_API_KEY",
];

/// The manual-key fallback always targets this name — mirrors
/// `scripts/make-kanban-profile`'s own non-interactive fallback exactly
/// (`OPENROUTER_API_KEY`, the judge's primary provider), which is the
/// decisive case D-08 cites for why this wizard exists as native Rust.
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
const MANUAL_KEY_TARGET: &str = "OPENROUTER_API_KEY";

#[allow(dead_code)] // used in validate_name_client_side; dead_code fires under --all-features
const RESERVED_NAMES: [&str; 3] = ["default", "current", "none"];
#[allow(dead_code)] // used in validate_name_client_side; dead_code fires under --all-features
const PROFILE_NAME_MAX_LEN: usize = 64;

/// Step spine. Display-only in this phase — no click-to-jump (matches the
/// canvas, which drives `next`/`back` only from the footer).
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WizardStep {
    Identity,
    ConfigKeys,
    Verify,
}

/// Client-side name-rule outcome. Four variants — UI-SPEC's own four
/// locked strings, ported from the bash script's `die()` messages, not
/// from the server rule's `Display` text (RESEARCH.md Pitfall 3).
#[allow(dead_code)] // used in CreateProfileWizard/validate_name_client_side; dead_code fires under --all-features
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NameError {
    Required,
    Reserved(String),
    TooLong,
    InvalidChars,
}

impl NameError {
    #[allow(dead_code)] // used in CreateProfileWizard rsx!; dead_code fires under --all-features
    fn message(&self) -> String {
        match self {
            NameError::Required => "Required — usage: make-kanban-profile <name>".to_string(),
            NameError::Reserved(n) => format!("'{n}' is a reserved profile name"),
            NameError::TooLong => "Profile name too long (max 64)".to_string(),
            NameError::InvalidChars => {
                "Lowercase letters, digits and hyphens only; must not start with \"-\"".to_string()
            }
        }
    }
}

/// Client-side mirror of the server's name rule set — UX only, entirely
/// synchronous, no network or disk call. `create_profile` independently
/// re-validates server-side through the real rule set before any write
/// (T-47.4-07-T1); a crafted request that skips this fn cannot create a
/// reserved or traversal-shaped name.
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
pub(crate) fn validate_name_client_side(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Required);
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(NameError::Reserved(name.to_string()));
    }
    if name.chars().count() > PROFILE_NAME_MAX_LEN {
        return Err(NameError::TooLong);
    }
    let Some(first) = name.chars().next() else {
        return Err(NameError::Required);
    };
    let first_ok = first.is_ascii_lowercase() || first.is_ascii_digit();
    let rest_ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !first_ok || !rest_ok {
        return Err(NameError::InvalidChars);
    }
    Ok(())
}

/// The literal placeholder substitution the canvas uses (`s.name.trim() ||
/// 'name'`) — reused by both the info block (Step 1) and the command
/// preview (footer, every step) so the two never diverge.
#[allow(dead_code)] // used in CreateProfileWizard/render_command_preview; dead_code fires under --all-features
fn display_name_or_placeholder(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "name".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Live `$ scripts/make-kanban-profile …` command preview (D-08's
/// legibility anchor between the UI path and the CLI path). Never emits a
/// secret-storage flag (D-06) — only `--force` and one of
/// `--all-keys`/`--keys "…"`, in that order.
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
pub(crate) fn render_command_preview(name: &str, force: bool, mode: &KeyMode) -> String {
    let display_name = display_name_or_placeholder(name);
    let mut out = format!("scripts/make-kanban-profile {display_name}");
    if force {
        out.push_str(" --force");
    }
    match mode {
        KeyMode::LlmOnly => {}
        KeyMode::AllKeys => out.push_str(" --all-keys"),
        KeyMode::Explicit(names) => {
            out.push_str(" --keys \"");
            out.push_str(&names.join(" "));
            out.push('"');
        }
    }
    out
}

/// Parses the space-separated custom key-name field into an ordered,
/// blank-entry-dropped name list.
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
fn parse_explicit_names(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(|s| s.to_string()).collect()
}

/// The three key-inheritance modes as a `Copy` selector — `KeyMode::Explicit`
/// carries a `Vec<String>` which would make the mode signal itself
/// non-`Copy`; the explicit name list lives in its own text-input signal
/// instead and is only assembled into `KeyMode::Explicit` on read.
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeyModeKind {
    LlmOnly,
    AllKeys,
    Explicit,
}

/// Wizard step-2 status label. Collapses `KeyStatus::ManuallySet` into the
/// same `INHERITED` presentation as `Inherited` — from this surface's own
/// perspective (a profile being created, not yet edited) the operator-
/// facing distinction is only "will be written" vs "not in the root
/// .env"; the finer per-source distinction belongs to the profile detail
/// drawer (a later plan), which already renders it separately.
#[allow(dead_code)] // used in CreateProfileWizard rsx!; dead_code fires under --all-features
fn key_row_status_label_and_class(status: &KeyStatus) -> (&'static str, &'static str) {
    match status {
        KeyStatus::Inherited | KeyStatus::ManuallySet => ("INHERITED", "kn-key-status--inherited"),
        KeyStatus::Missing => ("NOT IN ROOT .ENV", "kn-key-status--missing"),
    }
}

/// Phase 47.4 Plan 08 (reuse discipline): the VERIFY doctor-block renderer,
/// shared verbatim between this wizard's step 3 and the profile detail
/// drawer's on-demand VERIFY action — the identical Pending/Success/
/// Failure/Timeout treatment rendered from one place so the two surfaces
/// can never drift apart. `resolved_report` follows the convention
/// `CreateProfileWizard` already established: `None` renders Pending
/// (still resolving or freshly kicked off), `Some(Err(_))` is an
/// infrastructure-level failure (never upgraded to Success), `Some(Ok(report))`
/// renders from the real `VerifyReport`.
#[allow(dead_code)] // used in CreateProfileWizard + ProfileDetailDrawer rsx!; dead_code fires under --all-features
pub(crate) fn render_verify_doctor_block(
    name: &str,
    resolved_report: &Option<Result<VerifyReport, String>>,
) -> Element {
    match resolved_report {
        None => rsx! {
            div { style: "color: var(--success); font-size: var(--fs-13);", "✓ profile dir  created" }
            div { style: "color: var(--success); font-size: var(--fs-13);", "✓ config.yaml  copied from root" }
            div { style: "color: var(--success); font-size: var(--fs-13);", "✓ .env  written" }
            div { style: "color: var(--fg-dim); font-size: var(--fs-13);", "· verifying judge reachability…" }
        },
        Some(Err(infra_err)) => rsx! {
            div {
                style: "color: var(--danger); font-size: var(--fs-13); overflow-wrap: anywhere; -webkit-line-clamp: 2; display: -webkit-box; -webkit-box-orient: vertical; overflow: hidden;",
                "✕ judge model  UNREACHABLE — {infra_err}"
            }
            div { style: "color: var(--fg-dim); font-size: var(--fs-11);",
                "check the provider key above and press VERIFY AGAIN"
            }
        },
        Some(Ok(report)) => {
            let dir_row_class = if report.dir_ok { "color: var(--success);" } else { "color: var(--danger);" };
            let config_row_class = if report.config_ok { "color: var(--success);" } else { "color: var(--danger);" };
            let env_row_class = if report.env_ok { "color: var(--success);" } else { "color: var(--danger);" };
            let model = report.model_default.clone().unwrap_or_default();
            let first_key = report.first_key.clone().unwrap_or_default();
            let key_count = report.key_count;
            rsx! {
                div { style: "font-size: var(--fs-13); {dir_row_class}", "profile dir  ~/.ironhermes/profiles/{name}" }
                div { style: "font-size: var(--fs-13); {config_row_class}", "config.yaml  copied from root · model.default = {model}" }
                div { style: "font-size: var(--fs-13); {env_row_class}", ".env  0600 · {key_count} keys ({first_key}…)" }
                match &report.outcome {
                    VerifyOutcome::Success => rsx! {
                        div { style: "font-size: var(--fs-13); color: var(--success);", "judge model  reachable — build_runtime_judge_fn OK" }
                        div { style: "font-size: var(--fs-11); color: var(--fg-dim);", "worker env  scrubbed; profile .env is the only key source" }
                        div { style: "font-size: var(--fs-13); color: var(--success);", "profile \"{name}\" is ready for kanban dispatch" }
                    },
                    VerifyOutcome::Failure { summary } => rsx! {
                        div {
                            style: "color: var(--danger); font-size: var(--fs-13); overflow-wrap: anywhere; -webkit-line-clamp: 2; display: -webkit-box; -webkit-box-orient: vertical; overflow: hidden;",
                            "✕ judge model  UNREACHABLE — {summary}"
                        }
                        div { style: "color: var(--fg-dim); font-size: var(--fs-11);",
                            "check the provider key above and press VERIFY AGAIN"
                        }
                    },
                    VerifyOutcome::Timeout { seconds } => rsx! {
                        div { style: "color: var(--warn); font-size: var(--fs-13); overflow-wrap: anywhere;",
                            "· judge model  no response after {seconds}s — provider may be slow or the key may be invalid"
                        }
                    },
                }
            }
        }
    }
}

// ============================================================================
// CreateProfileWizard
// ============================================================================

/// Phase 47.4 Plan 07: the three-step profile-creation wizard. Mounted
/// conditionally from `screens/kanban.rs`, the same way `CreateTaskModal`
/// is today.
#[component]
pub fn CreateProfileWizard(
    on_dismiss: EventHandler<()>,
    on_created: EventHandler<String>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).

    // ---- Step spine ----
    let mut step: Signal<WizardStep> = use_signal(|| WizardStep::Identity);

    // ---- Step 1: IDENTITY ----
    let mut name: Signal<String> = use_signal(String::new);

    // ---- Step 2: CONFIG & KEYS ----
    let mut key_mode_kind: Signal<KeyModeKind> = use_signal(|| KeyModeKind::LlmOnly);
    let mut explicit_keys_input: Signal<String> = use_signal(String::new);
    let mut force: Signal<bool> = use_signal(|| false);
    let mut manual_key_value: Signal<String> = use_signal(String::new);
    let mut create_error: Signal<Option<String>> = use_signal(|| None);
    let mut creating: Signal<bool> = use_signal(|| false);

    // ---- Step 3: VERIFY ----
    // `verify_trigger` forces a fresh probe on VERIFY AGAIN without ever
    // calling a resource-restart method (see this file's own top-of-file
    // discipline note).
    let mut verify_trigger: Signal<u32> = use_signal(|| 0);

    // ---- Derived values (read BEFORE rsx!, clippy.toml signal-borrow
    // discipline: no GenerationalRef held across an .await or into rsx!). ----
    let name_val = name.read().clone();
    let name_validation = validate_name_client_side(&name_val);
    let name_is_valid = name_validation.is_ok();

    let key_mode_kind_val = *key_mode_kind.read();
    let explicit_input_val = explicit_keys_input.read().clone();
    let explicit_names = parse_explicit_names(&explicit_input_val);
    let explicit_is_blank = key_mode_kind_val == KeyModeKind::Explicit && explicit_names.is_empty();
    let current_key_mode = match key_mode_kind_val {
        KeyModeKind::LlmOnly => KeyMode::LlmOnly,
        KeyModeKind::AllKeys => KeyMode::AllKeys,
        KeyModeKind::Explicit => KeyMode::Explicit(explicit_names.clone()),
    };
    let force_val = *force.read();
    let manual_key_val = manual_key_value.read().clone();

    let command_preview = render_command_preview(&name_val, force_val, &current_key_mode);
    let current_step = *step.read();

    // ---- Step 2 key-resolution preview (server-computed, D-13). ----
    // A dependent `use_resource` — Dioxus tracks the signal reads made
    // before the `async move` block and re-runs this resource whenever any
    // of them change. This is native reactive re-computation, never a
    // manually invoked resource-restart call.
    let preview_resource = use_resource(move || {
        let kind = *key_mode_kind.read();
        let explicit_raw = explicit_keys_input.read().clone();
        let manual = manual_key_value.read().clone();
        async move {
            let mode = match kind {
                KeyModeKind::LlmOnly => KeyMode::LlmOnly,
                KeyModeKind::AllKeys => KeyMode::AllKeys,
                KeyModeKind::Explicit => KeyMode::Explicit(parse_explicit_names(&explicit_raw)),
            };
            let manual_keys = if manual.trim().is_empty() {
                Vec::new()
            } else {
                vec![(MANUAL_KEY_TARGET.to_string(), manual)]
            };
            crate::server::profile_api::preview_resolved_keys(mode, manual_keys).await
        }
    });
    let preview_snapshot = preview_resource();

    let has_llm_key_resolved = matches!(
        &preview_snapshot,
        Some(Ok(rows)) if rows.iter().any(|r| {
            CLIENT_LLM_KEY_ALLOWLIST.contains(&r.name.as_str())
                && matches!(r.status, KeyStatus::Inherited | KeyStatus::ManuallySet)
        })
    );
    let show_manual_key_field = matches!(&preview_snapshot, Some(Ok(_))) && !has_llm_key_resolved;

    // ---- Step 3 verify probe. ----
    // Only actually calls the server while `step == Verify` — fires on
    // entering step 3, not on wizard mount. `verify_trigger` is the only
    // way to force a second run (VERIFY AGAIN), never a resource-restart
    // call.
    let name_for_verify_read = name.read().clone();
    let verify_resource = use_resource(move || {
        let on_verify_step = *step.read() == WizardStep::Verify;
        let trigger = *verify_trigger.read();
        let profile_name = name_for_verify_read.clone();
        async move {
            let _ = trigger;
            if !on_verify_step {
                return None;
            }
            Some(crate::server::profile_verify_api::verify_profile(profile_name).await)
        }
    });
    let verify_snapshot = verify_resource();
    // Any shape other than a fully resolved report — still loading, or a
    // stale skip from a step that wasn't Verify yet — renders as Pending.
    // No branch here ever upgrades an Err/Failure/Timeout to Success.
    let resolved_report: Option<Result<VerifyReport, String>> = match verify_snapshot {
        Some(Some(Ok(report))) => Some(Ok(report)),
        Some(Some(Err(e))) => Some(Err(format!("{e}"))),
        _ => None,
    };

    // ---- Handlers ----
    let mut go_back = move || {
        let current = *step.read();
        step.set(match current {
            WizardStep::Identity => WizardStep::Identity,
            WizardStep::ConfigKeys => WizardStep::Identity,
            WizardStep::Verify => WizardStep::ConfigKeys,
        });
    };

    let mut submit_create = move || {
        let name_owned = name.read().clone();
        let key_mode_owned = match *key_mode_kind.read() {
            KeyModeKind::LlmOnly => KeyMode::LlmOnly,
            KeyModeKind::AllKeys => KeyMode::AllKeys,
            KeyModeKind::Explicit => {
                KeyMode::Explicit(parse_explicit_names(&explicit_keys_input.read()))
            }
        };
        let force_owned = *force.read();
        let manual_val = manual_key_value.read().clone();
        let manual_keys: Vec<(String, String)> = if manual_val.trim().is_empty() {
            Vec::new()
        } else {
            vec![(MANUAL_KEY_TARGET.to_string(), manual_val)]
        };
        creating.set(true);
        create_error.set(None);
        let mut creating_sig = creating;
        let mut create_error_sig = create_error;
        let mut step_sig = step;
        let mut manual_key_value_sig = manual_key_value;
        let name_for_created = name_owned.clone();
        spawn(async move {
            let req = CreateProfileRequest {
                name: name_owned,
                key_mode: key_mode_owned,
                force: force_owned,
                manual_keys,
            };
            let result = crate::server::profile_api::create_profile(req).await;
            creating_sig.set(false);
            match result {
                Ok(_rows) => {
                    // D-13: drop the typed value immediately after the
                    // write, whether or not it was actually used — never
                    // redisplayed, mirroring providers.rs:541-542.
                    manual_key_value_sig.set(String::new());
                    step_sig.set(WizardStep::Verify);
                    on_created.call(name_for_created);
                }
                Err(e) => create_error_sig.set(Some(format!("{e}"))),
            }
        });
    };

    rsx! {
        div {
            class: "kn-modal-overlay",
            role: "presentation",
            // Clicking the backdrop deliberately does NOT dismiss (mirrors
            // ModalShell's existing convention, modals.rs:104-105).
            div {
                class: "kn-wizard",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "kn-wizard-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        on_dismiss.call(());
                    }
                },
                div { class: "kn-modal-header",
                    span {
                        style: "color: var(--accent-primary); font-size: var(--fs-11);",
                        "// SCRIPTS / MAKE-KANBAN-PROFILE"
                    }
                    h3 { class: "kn-modal-title", id: "kn-wizard-title", "Create Kanban Profile" }
                    button {
                        class: "kn-drawer-close",
                        "aria-label": "Close create profile",
                        onclick: move |_| on_dismiss.call(()),
                        "✕"
                    }
                }
                div { class: "kn-wizard-steps",
                    span {
                        class: if current_step == WizardStep::Identity { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg kn-modal-seg--done" },
                        "1 · IDENTITY"
                    }
                    span {
                        class: if current_step == WizardStep::ConfigKeys {
                            "kn-modal-seg kn-modal-seg--active"
                        } else if current_step == WizardStep::Verify {
                            "kn-modal-seg kn-modal-seg--done"
                        } else {
                            "kn-modal-seg"
                        },
                        "2 · CONFIG & KEYS"
                    }
                    span {
                        class: if current_step == WizardStep::Verify { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                        "3 · VERIFY"
                    }
                }
                div { class: "kn-modal-body",
                    if current_step == WizardStep::Identity {
                        label { class: "kn-modal-label", "PROFILE NAME" }
                        input {
                            class: "kn-modal-input",
                            style: if name_is_valid {
                                "border-color: var(--accent-primary);"
                            } else {
                                "border-color: var(--danger);"
                            },
                            placeholder: "kanban-worker",
                            value: "{name_val}",
                            oninput: move |evt| name.set(evt.value()),
                        }
                        if let Err(ref err) = name_validation {
                            div {
                                style: "font-size: var(--fs-11); color: var(--danger); overflow-wrap: anywhere;",
                                "{err.message()}"
                            }
                        } else {
                            div {
                                style: "font-size: var(--fs-11); color: var(--success); overflow-wrap: anywhere;",
                                "Valid. Profile will be created at ~/.ironhermes/profiles/{display_name_or_placeholder(&name_val)}"
                            }
                        }
                        div { class: "kn-drawer-section",
                            div { class: "kn-drawer-section-label", "PROFILE DIR" }
                            div {
                                style: "overflow-wrap: anywhere; color: var(--fg);",
                                "~/.ironhermes/profiles/{display_name_or_placeholder(&name_val)}"
                            }
                            div { class: "kn-drawer-section-label", "CONFIG.YAML" }
                            div { style: "color: var(--fg);", "copied from ~/.ironhermes/config.yaml" }
                            div { class: "kn-drawer-section-label", ".ENV" }
                            div { style: "color: var(--fg);", "written 0600 with inherited provider keys" }
                        }
                        div {
                            style: "border-left: 2px solid var(--warn); background: color-mix(in srgb, var(--warn) 12%, var(--w-bg-3)); padding: var(--sp-2) var(--sp-4); font-size: var(--fs-11); color: var(--fg-dim);",
                            "Kanban workers run non-interactively (ironhermes --profile NAME chat -q), so the first-run wizard is skipped. Without config.yaml and a provider key the worker crashes at judge-build ~1s after spawn."
                        }
                    } else if current_step == WizardStep::ConfigKeys {
                        label { class: "kn-modal-label", "KEY INHERITANCE — FROM ~/.IRONHERMES/.ENV" }
                        div {
                            style: "display: flex; align-items: center; gap: var(--sp-2); padding: var(--sp-2) 0; cursor: pointer;",
                            onclick: move |_| key_mode_kind.set(KeyModeKind::LlmOnly),
                            span {
                                "aria-hidden": "true",
                                style: if key_mode_kind_val == KeyModeKind::LlmOnly { "color: var(--accent-primary);" } else { "color: var(--fg-dim);" },
                                if key_mode_kind_val == KeyModeKind::LlmOnly { "●" } else { "○" }
                            }
                            div { style: "flex: 1 1 auto;",
                                div { style: "font-size: var(--fs-13); color: var(--fg);", "LLM provider keys only" }
                                div { style: "font-size: var(--fs-11); color: var(--fg-dim);",
                                    "OPENROUTER, ANTHROPIC, OPENAI, GROQ, OLLAMA — keeps Telegram/Fal and friends out of the worker env."
                                }
                            }
                            span { style: "font-size: var(--fs-11); color: var(--accent-primary);", "default" }
                        }
                        div {
                            style: "display: flex; align-items: center; gap: var(--sp-2); padding: var(--sp-2) 0; cursor: pointer;",
                            onclick: move |_| key_mode_kind.set(KeyModeKind::AllKeys),
                            span {
                                "aria-hidden": "true",
                                style: if key_mode_kind_val == KeyModeKind::AllKeys { "color: var(--accent-primary);" } else { "color: var(--fg-dim);" },
                                if key_mode_kind_val == KeyModeKind::AllKeys { "●" } else { "○" }
                            }
                            div { style: "flex: 1 1 auto;",
                                div { style: "font-size: var(--fs-13); color: var(--fg);", "Every *_API_KEY / *_KEY / *_TOKEN" }
                                div { style: "font-size: var(--fs-11); color: var(--fg-dim);", "Inherits the whole root .env key surface." }
                            }
                            span { style: "font-size: var(--fs-11); color: var(--accent-primary);", "--all-keys" }
                        }
                        div {
                            style: "display: flex; align-items: center; gap: var(--sp-2); padding: var(--sp-2) 0; cursor: pointer;",
                            onclick: move |_| key_mode_kind.set(KeyModeKind::Explicit),
                            span {
                                "aria-hidden": "true",
                                style: if key_mode_kind_val == KeyModeKind::Explicit { "color: var(--accent-primary);" } else { "color: var(--fg-dim);" },
                                if key_mode_kind_val == KeyModeKind::Explicit { "●" } else { "○" }
                            }
                            div { style: "flex: 1 1 auto;",
                                div { style: "font-size: var(--fs-13); color: var(--fg);", "Explicit list" }
                                div { style: "font-size: var(--fs-11); color: var(--fg-dim);", "Space-separated env var names; overrides the allowlist." }
                            }
                            span { style: "font-size: var(--fs-11); color: var(--accent-primary);", "--keys" }
                        }
                        if key_mode_kind_val == KeyModeKind::Explicit {
                            input {
                                class: "kn-modal-input",
                                placeholder: "MY_CUSTOM_API_KEY ANOTHER_KEY",
                                value: "{explicit_input_val}",
                                oninput: move |evt| explicit_keys_input.set(evt.value()),
                            }
                        }
                        div { class: "kn-key-table",
                            div {
                                class: "kn-drawer-section-label",
                                "WILL BE WRITTEN TO ~/.ironhermes/profiles/{display_name_or_placeholder(&name_val)}/.env"
                            }
                            if explicit_is_blank {
                                div { class: "kn-drawer-empty",
                                    "No key names entered — the profile .env will have no provider key."
                                }
                            } else {
                                match &preview_snapshot {
                                    None => rsx! {
                                        div { class: "kn-drawer-loading", "Resolving keys from ~/.ironhermes/.env…" }
                                    },
                                    Some(Err(_)) => rsx! {
                                        div { class: "kn-modal-error", "Could not read ~/.ironhermes/.env. Check permissions and retry." }
                                    },
                                    Some(Ok(rows)) => rsx! {
                                        for row in rows.iter().cloned() {
                                            {
                                                let (status_label, status_class) = key_row_status_label_and_class(&row.status);
                                                rsx! {
                                                    div { class: "kn-key-row", key: "{row.name}",
                                                        span {
                                                            style: "font-size: var(--fs-13); color: var(--accent-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1 1 auto;",
                                                            "{row.name}"
                                                        }
                                                        span { style: "font-size: var(--fs-13); color: var(--fg-faint);", "{row.masked}" }
                                                        span { class: status_class, "{status_label}" }
                                                    }
                                                }
                                            }
                                        }
                                    },
                                }
                            }
                        }
                        if show_manual_key_field {
                            label { class: "kn-modal-label", "MANUAL KEY — {MANUAL_KEY_TARGET}" }
                            input {
                                class: "kn-key-input",
                                r#type: "password",
                                value: "{manual_key_val}",
                                oninput: move |evt| manual_key_value.set(evt.value()),
                            }
                            div { style: "font-size: var(--fs-11); color: var(--fg-dim);",
                                "No LLM provider key resolved above. A kanban worker with no provider key crashes at judge-build. Enter one here, or add it to the root .env and switch mode."
                            }
                        }
                        label { class: "kn-modal-checkbox",
                            input {
                                r#type: "checkbox",
                                checked: force_val,
                                onchange: move |evt| force.set(evt.checked()),
                            }
                            "Overwrite an existing profile's config.yaml / .env"
                            span { class: "kn-key-status--missing", " --force" }
                        }
                        if let Some(err) = create_error.read().clone() {
                            div { class: "kn-modal-error", "{err}" }
                        }
                    } else {
                        // ---- Step 3: VERIFY ----
                        div { style: "font-weight: 700; font-size: var(--fs-13); color: var(--fg);",
                            span { "aria-hidden": "true", "▊ " }
                            "VERIFY — IRONHERMES --PROFILE {name_val} DOCTOR"
                        }
                        {render_verify_doctor_block(&name_val, &resolved_report)}
                        div { class: "kn-drawer-section",
                            div { class: "kn-drawer-section-label", "ASSIGN WORK TO THIS PROFILE" }
                            pre {
                                style: "font-family: var(--font-mono); font-size: var(--fs-11); color: var(--fg-dim); white-space: pre-wrap; overflow-wrap: anywhere;",
                                "ironhermes kanban create \"your task title\" --assignee {name_val}\n# or reassign an existing task\nironhermes kanban assign <task_id> {name_val}"
                            }
                        }
                    }
                }
                div { class: "kn-wizard-footer",
                    span { class: "kn-wizard-preview", "$ {command_preview}" }
                    div { style: "display: flex; gap: var(--sp-2);",
                        button {
                            class: "kn-modal-btn",
                            onclick: move |_| {
                                match current_step {
                                    WizardStep::Identity => on_dismiss.call(()),
                                    _ => go_back(),
                                }
                            },
                            if current_step == WizardStep::Identity { "CANCEL" } else { "← BACK" }
                        }
                        if current_step == WizardStep::Identity {
                            button {
                                class: "kn-modal-btn kn-modal-btn--submit",
                                disabled: !name_is_valid,
                                onclick: move |_| {
                                    if name_is_valid {
                                        step.set(WizardStep::ConfigKeys);
                                    }
                                },
                                "NEXT →"
                            }
                        } else if current_step == WizardStep::ConfigKeys {
                            button {
                                class: "kn-modal-btn kn-modal-btn--submit",
                                disabled: *creating.read(),
                                onclick: move |_| submit_create(),
                                if *creating.read() { "CREATING…" } else { "CREATE PROFILE" }
                            }
                        } else {
                            match &resolved_report {
                                Some(Ok(report)) if report.outcome == VerifyOutcome::Success => rsx! {
                                    button {
                                        class: "kn-modal-btn kn-modal-btn--submit",
                                        onclick: move |_| on_dismiss.call(()),
                                        "DONE"
                                    }
                                },
                                Some(_) => rsx! {
                                    button {
                                        class: "kn-modal-btn kn-modal-btn--submit",
                                        onclick: move |_| {
                                            let cur = *verify_trigger.read();
                                            verify_trigger.set(cur + 1);
                                        },
                                        "VERIFY AGAIN"
                                    }
                                },
                                None => rsx! {
                                    button { class: "kn-modal-btn", disabled: true, "VERIFYING…" }
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// Pure-fn tests (Task 1 <behavior>)
// ============================================================================
#[cfg(test)]
mod wizard_pure_fn_tests {
    use super::*;

    // --- validate_name_client_side ---------------------------------------

    #[test]
    fn empty_name_is_required() {
        assert_eq!(validate_name_client_side(""), Err(NameError::Required));
    }

    #[test]
    fn reserved_names_carry_the_name() {
        assert_eq!(
            validate_name_client_side("default"),
            Err(NameError::Reserved("default".to_string()))
        );
        assert_eq!(
            validate_name_client_side("current"),
            Err(NameError::Reserved("current".to_string()))
        );
        assert_eq!(
            validate_name_client_side("none"),
            Err(NameError::Reserved("none".to_string()))
        );
    }

    #[test]
    fn sixty_five_chars_is_too_long_sixty_four_is_valid() {
        let sixty_four = "a".repeat(64);
        let sixty_five = "a".repeat(65);
        assert!(validate_name_client_side(&sixty_four).is_ok());
        assert_eq!(
            validate_name_client_side(&sixty_five),
            Err(NameError::TooLong)
        );
    }

    #[test]
    fn invalid_char_shapes_are_rejected() {
        for bad in ["Foo", "-lead", "has_underscore", "has space", "a/b", ".."] {
            assert_eq!(
                validate_name_client_side(bad),
                Err(NameError::InvalidChars),
                "expected InvalidChars for {bad:?}"
            );
        }
    }

    #[test]
    fn valid_names_pass() {
        for good in ["kanban-worker", "a", "9x"] {
            assert!(
                validate_name_client_side(good).is_ok(),
                "expected Ok for {good:?}"
            );
        }
    }

    // --- render_command_preview -------------------------------------------

    #[test]
    fn preview_with_no_flags() {
        assert_eq!(
            render_command_preview("myprofile", false, &KeyMode::LlmOnly),
            "scripts/make-kanban-profile myprofile"
        );
    }

    #[test]
    fn preview_with_force() {
        assert_eq!(
            render_command_preview("myprofile", true, &KeyMode::LlmOnly),
            "scripts/make-kanban-profile myprofile --force"
        );
    }

    #[test]
    fn preview_with_all_keys() {
        assert_eq!(
            render_command_preview("myprofile", false, &KeyMode::AllKeys),
            "scripts/make-kanban-profile myprofile --all-keys"
        );
    }

    #[test]
    fn preview_with_explicit_keys() {
        assert_eq!(
            render_command_preview(
                "myprofile",
                false,
                &KeyMode::Explicit(vec!["A".to_string(), "B".to_string()])
            ),
            "scripts/make-kanban-profile myprofile --keys \"A B\""
        );
    }

    #[test]
    fn preview_flag_set_is_exactly_force_and_one_key_mode_flag() {
        // Structural guarantee: `render_command_preview` only ever emits
        // `--force` and one of `--all-keys`/`--keys "…"` — no other flag
        // exists in the match arm, so no unauthorized storage flag (D-06)
        // can appear. Asserted by full-string equality against every
        // documented case rather than a substring negative check.
        assert_eq!(
            render_command_preview("p", true, &KeyMode::AllKeys),
            "scripts/make-kanban-profile p --force --all-keys"
        );
    }

    #[test]
    fn preview_substitutes_the_placeholder_name_when_blank() {
        assert_eq!(
            render_command_preview("", false, &KeyMode::LlmOnly),
            "scripts/make-kanban-profile name"
        );
        assert_eq!(
            render_command_preview("   ", false, &KeyMode::LlmOnly),
            "scripts/make-kanban-profile name"
        );
    }
}
