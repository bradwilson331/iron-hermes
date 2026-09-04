//! Phase 47.4 Plan 07 (D-01/D-06/D-07/D-08/D-09/D-13): the Create Kanban
//! Profile wizard — the phase's headline surface (D-01). Three steps:
//! IDENTITY (client-side name validation, UX only), CONFIG & KEYS (three
//! key-inheritance modes plus a manual fallback, no secret-storage branch
//! per D-06), VERIFY (the real D-09 probe's four states).
//!
//! Phase 50.1 Plan 02 (D-10): relocated from `screens/kanban/wizard.rs` into
//! this shared module so both `screens/kanban.rs` and the Agents screen's
//! bot roster consume ONE implementation. A `context: ProfileDialogContext`
//! prop (added by this plan's Task 2) selects bot-flavored vs
//! kanban-flavored copy on the same component. `screens/kanban/wizard.rs`
//! is now a thin re-export shim at the old path.
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

use super::ProfileDialogContext;
use super::advanced::AdvancedProfilePane;
use super::secrets_source_picker::{SecretsSourcePicker, VAULT_REASON_BUILD_LACKS_FEATURE};
use crate::components::hermes_app::widgets::avatar_picker::AvatarPicker;
use crate::protocol::{
    BotAvatarDescriptor, CloneFromChoice, CreateProfileRequest, DuplicateProfileRequest, KeyMode,
    KeyRow, KeyStatus, ProfileHealth, SecretSource, VerifyOutcome, VerifyReport,
};
use dioxus::prelude::*;

/// Phase 49.4: the `kn-modal-*` / `kn-wizard` styles this dialog uses live in
/// `kanban.css`, which is linked per-screen (Kanban, Tools, Agents) but NOT
/// globally — so when the wizard mounts on the Soul screen (its `+ ADD PROFILE`
/// entry) it rendered completely unstyled. Linking the stylesheet from the
/// component itself makes it styled wherever it mounts. `document::Link`
/// dedupes by href, so this is a no-op on a screen that already links it.
const WIZARD_CSS: Asset = asset!("/assets/kanban.css");

/// DOM id shared by the CLONE FROM `<input>`'s `list` attribute and its
/// paired `<datalist>` — mirrors `kanban/modals.rs`'s
/// `KN_ASSIGNEE_DATALIST_ID` pattern (the crate's established
/// datalist-backed-input precedent).
#[allow(dead_code)] // used in CreateProfileWizard rsx!; dead_code fires under --all-features
const KN_CLONE_FROM_DATALIST_ID: &str = "kn-clone-from-profiles";

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

/// Phase 49.4.1 Plan 02 (D-07): the pre-existing `KeyMode` breadth
/// control's eyebrow — a named constant (not an inline literal at the rsx!
/// site) so [`render_step_two_sections_order`] and the actual render can
/// never drift apart.
const KEY_INHERITANCE_SECTION_LABEL: &str = "KEY INHERITANCE — FROM ~/.IRONHERMES/.ENV";

/// Phase 49.4.1 Plan 02 (D-07): step 2's section eyebrows in the ORDER they
/// are emitted — the test seat `wizard_step_two_renders_secrets_source_
/// above_key_inheritance` asserts `SECRETS SOURCE`'s index here is smaller
/// than `KEY INHERITANCE`'s. The rsx! below renders `SecretsSourcePicker`
/// (whose own label is `secrets_source_picker::SECRETS_SOURCE_SECTION_
/// LABEL`) before [`KEY_INHERITANCE_SECTION_LABEL`], so this list and the
/// actual render share the same two constants and cannot drift.
#[allow(dead_code)] // test seat only — consumed by wizard_pure_fn_tests, never by rsx! (the rsx! renders these same two constants directly)
pub(crate) fn render_step_two_sections_order() -> Vec<&'static str> {
    vec![
        super::secrets_source_picker::SECRETS_SOURCE_SECTION_LABEL,
        KEY_INHERITANCE_SECTION_LABEL,
    ]
}

/// Step spine. Display-only in this phase — no click-to-jump (matches the
/// canvas, which drives `next`/`back` only from the footer).
///
/// Phase 49.4 Plan 10 (D-15): `Entry` and `ClonePicker` are new, reachable
/// only when the caller opts in via `CreateProfileWizard`'s
/// `show_entry_step` prop (default `false` — the two pre-existing mounts,
/// Kanban board and Agents roster, omit it and land on `Identity` exactly
/// as before this plan).
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WizardStep {
    Entry,
    ClonePicker,
    Identity,
    ConfigKeys,
    Verify,
}

/// Phase 49.4 Plan 10 (D-15): the add-new-profile dialog's entry-point
/// choice — the UI-SPEC Copywriting Contract's two labels ("Start from a
/// template" / "Clone an existing profile"). `Template` proceeds into the
/// wizard's pre-existing steps unchanged (this plan's own `create_profile`
/// path); `Clone` routes through `ClonePicker` and submits via
/// `duplicate_profile` instead, reusing the wizard's pre-existing
/// `clone_from_choice`/`clone_source` signals (the same ones the bot-context
/// Advanced disclosure's "Clone existing bot" control already writes to) —
/// one submit branch, never two.
#[allow(dead_code)] // used in CreateProfileWizard; dead_code fires under --all-features
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CreateEntry {
    Template,
    Clone,
}

/// Phase 49.4 Plan 10 (D-15, Claude's Discretion): the starter persona
/// written for a freshly template-created profile so the Soul editor never
/// opens on an empty textarea for a brand-new profile. Genuinely useful,
/// not a placeholder — the operator edits this in place immediately.
/// `create_profile`'s request DTO has no persona field (protocol.rs
/// `CreateProfileRequest`), so this is written via a follow-up
/// `save_profile_persona` call after `create_profile` succeeds — see
/// `submit_create`'s `Ok` arm.
#[allow(dead_code)] // used in CreateProfileWizard's submit_create; dead_code fires under --all-features
pub(crate) const STARTER_SOUL_MD: &str = "# Identity\n\nYou are a helpful assistant. Introduce yourself briefly on first contact and stay focused on the operator's actual request.\n\n# Voice\n\nSpeak plainly and concisely. Prefer short paragraphs and concrete next steps over hedging or filler.\n\n# Behaviour\n\nAsk a clarifying question when the request is ambiguous. Say what you did and why after completing a task, and flag anything you were unsure about.\n";

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

/// Phase 49.4.1 Plan 02 (UI-SPEC E5): the resolved-key preview row markup,
/// lifted out so both the live `Some(Ok(rows))` render and the E5 backstop
/// (dimmed previous rows while re-resolution runs) share ONE row
/// implementation — row markup, `--accent-primary` key names, and
/// `--success`/`--warn` status classes are unchanged from before this plan.
fn render_key_table_rows(rows: &[KeyRow]) -> Element {
    rsx! {
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
    context: ProfileDialogContext,
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
                        if matches!(context, ProfileDialogContext::Bot) {
                            div { style: "font-size: var(--fs-13); color: var(--success);", "\"{name}\" is ready to chat" }
                        } else {
                            div { style: "font-size: var(--fs-13); color: var(--success);", "profile \"{name}\" is ready for kanban dispatch" }
                        }
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
/// is today. Phase 50.1 Plan 02 (D-10): `context` selects bot-flavored vs
/// kanban-flavored copy on this one shared component — defaults to
/// `Kanban` so an omitted prop preserves the pre-lift behavior exactly.
#[component]
pub fn CreateProfileWizard(
    on_dismiss: EventHandler<()>,
    on_created: EventHandler<String>,
    #[props(default)] context: ProfileDialogContext,
    // Phase 49.4 Plan 10 (D-15): opt-in entry-point selection step ahead of
    // Identity. Defaults to `false` so the two pre-existing mounts (Kanban
    // board, Agents roster) keep landing on `Identity` first exactly as
    // before this plan — only the Soul page's new mount passes `true`.
    #[props(default)] show_entry_step: bool,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).

    // Phase 50.1 Plan 02 (OF-2): bot-context-only collapsed "Advanced"
    // disclosure — header only, no fields yet (plan 50.1-05 fills its
    // contents). Registered unconditionally alongside the wizard's other
    // hooks (Pattern E) even though it only renders in bot context.
    let mut advanced_disclosure_open: Signal<bool> = use_signal(|| false);

    // Phase 50.1 Plan 04 (D-12): the bot's avatar working copy. Registered
    // unconditionally (Pattern E) even though it only renders in bot
    // context. `None` means "no explicit choice yet" — AvatarPicker itself
    // resolves the deterministic seeded default from the typed name for
    // its live preview; this signal is only ever `Some` once the operator
    // makes an explicit shape/colour/upload/generate choice. The profile
    // does not exist yet while this dialog is open, so `on_save` below is
    // a no-op — `submit_create`'s success arm reads this signal AFTER
    // `create_profile` succeeds and persists it through `save_bot_meta`
    // then, never before the profile the descriptor is keyed to exists.
    let avatar_descriptor: Signal<Option<BotAvatarDescriptor>> = use_signal(|| None);

    // Phase 50.1 Plan 06 (D-17, UI-SPEC Component Inventory §3): the
    // clone-from control's working state — pre-creation, alongside Avatar
    // in the SAME Identity-step disclosure (never the drawer's Advanced
    // section, which mounts after a bot already exists; this mirrors
    // Plan 05's own "pre-creation fields live in the Identity-step
    // disclosure, post-creation fields live in the Verify-step one" split).
    // Registered unconditionally (Pattern E) even though it only renders
    // in bot context.
    let mut clone_from_choice: Signal<CloneFromChoice> = use_signal(CloneFromChoice::default);
    let mut clone_source: Signal<String> = use_signal(String::new);
    // Never a `?`-early-return `use_server_future` (this file's own
    // top-of-file discipline note) — a plain `use_resource`, matching
    // `preview_resource`/`verify_resource`'s existing shape in this file.
    let known_profiles_resource = use_resource(move || async move {
        crate::server::profile_api::list_profiles().await
    });

    // ---- Step spine ----
    // Phase 49.4 Plan 10 (D-15): starts on `Entry` only when the caller
    // opts in — the closure runs once at mount, so this never re-evaluates
    // `show_entry_step` mid-session (a `#[props]` value is immutable for
    // this component instance's lifetime anyway).
    let mut step: Signal<WizardStep> = use_signal(move || {
        if show_entry_step {
            WizardStep::Entry
        } else {
            WizardStep::Identity
        }
    });
    // Phase 49.4 Plan 10 (D-15): which entry-point the operator picked —
    // `None` until the Entry step's buttons are clicked (or always `None`
    // for the two pre-existing mounts, which never visit that step).
    let mut entry_choice: Signal<Option<CreateEntry>> = use_signal(|| None);

    // ---- Step 1: IDENTITY ----
    let mut name: Signal<String> = use_signal(String::new);

    // ---- Step 2: CONFIG & KEYS ----
    let mut key_mode_kind: Signal<KeyModeKind> = use_signal(|| KeyModeKind::LlmOnly);
    let mut explicit_keys_input: Signal<String> = use_signal(String::new);
    let mut force: Signal<bool> = use_signal(|| false);
    // Phase 49.4.1 Plan 03 (D-04/D-05/D-11): the Provided-keys working copy —
    // owned here, threaded into `SecretsSourcePicker` (the "one component,
    // two mounts" rule), and read back out at submit time. Typed rows and a
    // dropped/chosen file both populate this SAME signal — the picker's own
    // one key-carrying path.
    let mut manual_keys: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut create_error: Signal<Option<String>> = use_signal(|| None);
    let mut creating: Signal<bool> = use_signal(|| false);

    // Phase 49.4.1 Plan 02 (D-04): the operator's step-2 secrets-source
    // choice — registered unconditionally alongside the wizard's other
    // signals (Pattern E). `RootEnv` pre-selection matches the `default`
    // pill (UI-SPEC E1: "there is no zero-selection state").
    let secrets_source: Signal<SecretSource> = use_signal(|| SecretSource::RootEnv);
    // Phase 49.4.1 Plan 02 (D-10): a plain `use_resource` — never a
    // `use_server_future` with a `?` early return followed by `.restart()`,
    // the crate-wide discipline this file's own top-of-file note carries.
    let availability_resource = use_resource(move || async move {
        crate::server::profile_api::secrets_source_availability().await
    });
    // Phase 49.4.1 Plan 02 (UI-SPEC E5 backstop): the last successfully-
    // resolved preview rows, held so a source change dims the existing
    // table instead of clearing it to empty — an empty table must always
    // mean "this source resolved nothing", never "still working".
    let mut last_resolved_preview: Signal<Option<Vec<KeyRow>>> = use_signal(|| None);

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

    let command_preview = render_command_preview(&name_val, force_val, &current_key_mode);
    let current_step = *step.read();

    // ---- Clone-from derived values. ----
    let clone_choice_val = *clone_from_choice.read();
    let clone_source_val = clone_source.read().clone();
    let known_profile_names: Vec<String> = match known_profiles_resource() {
        Some(Ok(rows)) => rows.iter().map(|r| r.name.clone()).collect(),
        _ => Vec::new(),
    };

    // ---- Step 2 key-resolution preview (server-computed, D-13). ----
    // A dependent `use_resource` — Dioxus tracks the signal reads made
    // before the `async move` block and re-runs this resource whenever any
    // of them change. This is native reactive re-computation, never a
    // manually invoked resource-restart call.
    let preview_resource = use_resource(move || {
        let kind = *key_mode_kind.read();
        let explicit_raw = explicit_keys_input.read().clone();
        // Phase 49.4.1 Plan 03 (D-04/D-11): reading the picker's working
        // copy here (before the `async move` block) is what makes Dioxus
        // re-run this resource natively whenever a typed row or a completed
        // upload changes it — mirrors `key_mode_kind`/`explicit_keys_input`
        // immediately above. Never a manual resource-restart call.
        let manual_rows = manual_keys.read().clone();
        // Phase 49.4.1 Plan 02 (D-04): reading the signal here (before the
        // `async move` block) is what makes Dioxus re-run this resource
        // natively when the source changes — mirrors `key_mode_kind`/
        // `explicit_keys_input` immediately above. Never a manual
        // resource-restart call.
        let source = *secrets_source.read();
        async move {
            let mode = match kind {
                KeyModeKind::LlmOnly => KeyMode::LlmOnly,
                KeyModeKind::AllKeys => KeyMode::AllKeys,
                KeyModeKind::Explicit => KeyMode::Explicit(parse_explicit_names(&explicit_raw)),
            };
            let manual_keys =
                super::secrets_source_picker::submit_ready_manual_keys(&manual_rows);
            crate::server::profile_api::preview_resolved_keys(mode, manual_keys, source).await
        }
    });
    let preview_snapshot = preview_resource();

    // Phase 49.4.1 Plan 02 (UI-SPEC E5 backstop): mirror a successful
    // resolution into `last_resolved_preview` so the render below can show
    // those rows, dimmed, while a source change re-runs the resource above
    // rather than clearing the table to empty.
    use_effect(move || {
        if let Some(Ok(rows)) = preview_resource() {
            last_resolved_preview.set(Some(rows));
        }
    });

    let has_llm_key_resolved = matches!(
        &preview_snapshot,
        Some(Ok(rows)) if rows.iter().any(|r| {
            CLIENT_LLM_KEY_ALLOWLIST.contains(&r.name.as_str())
                && matches!(r.status, KeyStatus::Inherited | KeyStatus::ManuallySet)
        })
    );
    let show_manual_key_field = matches!(&preview_snapshot, Some(Ok(_))) && !has_llm_key_resolved;

    // Phase 49.4.1 Plan 03: the pre-existing single-field manual-key flow
    // always targeted MANUAL_KEY_TARGET as a hard-coded name — that
    // behaviour stays reachable now that entry lives in the picker's
    // Provided-keys row: the first time no LLM key resolves, seed an empty
    // working copy with a row pre-named MANUAL_KEY_TARGET so the "no LLM
    // provider key resolved" prompt still lands on the same key. Never
    // overwrites rows the operator has already started typing or uploaded.
    // Reads `preview_resource()` directly inside the effect (mirrors the
    // `last_resolved_preview` effect immediately above) — a plain bool
    // computed earlier in render does not subscribe an effect to anything,
    // so it would only ever fire once, on mount.
    use_effect(move || {
        if let Some(Ok(rows)) = preview_resource() {
            let resolved = rows.iter().any(|r| {
                CLIENT_LLM_KEY_ALLOWLIST.contains(&r.name.as_str())
                    && matches!(r.status, KeyStatus::Inherited | KeyStatus::ManuallySet)
            });
            if !resolved && manual_keys.peek().is_empty() {
                manual_keys.set(vec![(MANUAL_KEY_TARGET.to_string(), String::new())]);
            }
        }
    });

    // Phase 49.4.1 Plan 02 (D-10, UI-SPEC E1): while `availability_resource`
    // is unresolved, the vault row is born disabled with the build-level
    // reason rather than momentarily selectable — no row is ever
    // selectable-before-known. Both mounts (this one and the drawer's) read
    // from the SAME `secrets_source_availability` server fn.
    let (vault_available, vault_reason) = match availability_resource() {
        Some(Ok(availability)) => (availability.vault_available, availability.vault_reason),
        _ => (false, Some(VAULT_REASON_BUILD_LACKS_FEATURE.to_string())),
    };

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
            WizardStep::Entry => WizardStep::Entry,
            WizardStep::ClonePicker => WizardStep::Entry,
            WizardStep::Identity => {
                if show_entry_step {
                    WizardStep::Entry
                } else {
                    WizardStep::Identity
                }
            }
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
        // Phase 49.4.1 Plan 03 (D-04/D-11): the picker's Provided-keys
        // working copy — blank/partially-blank rows dropped here (UI-SPEC
        // E4), never raised as a submit-time error.
        let manual_keys_owned =
            super::secrets_source_picker::submit_ready_manual_keys(&manual_keys.read());
        // Phase 49.4.1 Plan 02 (D-04): the operator's step-2 choice.
        let secrets_source_owned = *secrets_source.read();
        // Phase 50.1 Plan 06 (D-17): clone-from mode replaces the
        // create_profile call with duplicate_profile entirely — the two
        // contracts are incompatible (see protocol.rs's DuplicateProfileRequest
        // doc), so step 2's key-mode/manual-key/force inputs are simply
        // unused in this branch rather than being folded into a merged
        // payload.
        let clone_choice_owned = *clone_from_choice.read();
        let clone_source_owned = clone_source.read().clone();
        // Phase 49.4 Plan 10 (D-15): read BEFORE spawn (Pattern B) — decides
        // whether the success arm below writes the starter persona.
        let entry_choice_owned = *entry_choice.read();
        creating.set(true);
        create_error.set(None);
        let mut creating_sig = creating;
        let mut create_error_sig = create_error;
        let mut step_sig = step;
        let mut manual_keys_sig = manual_keys;
        let name_for_created = name_owned.clone();
        let avatar_descriptor_for_submit = avatar_descriptor;
        spawn(async move {
            let result: Result<String, ServerFnError> = if clone_choice_owned
                == CloneFromChoice::CloneExisting
                && !clone_source_owned.trim().is_empty()
            {
                let req = DuplicateProfileRequest {
                    source: clone_source_owned.trim().to_string(),
                    target: name_owned.clone(),
                };
                crate::server::profile_api::duplicate_profile(req).await
            } else {
                let req = CreateProfileRequest {
                    name: name_owned.clone(),
                    key_mode: key_mode_owned,
                    force: force_owned,
                    manual_keys: manual_keys_owned,
                    secret_source: secrets_source_owned,
                };
                crate::server::profile_api::create_profile(req)
                    .await
                    .map(|_rows| name_owned.clone())
            };
            creating_sig.set(false);
            match result {
                Ok(_created_name) => {
                    // D-13: drop the typed/uploaded rows immediately after
                    // the write, whether or not they were actually used —
                    // never redisplayed, mirroring providers.rs:541-542.
                    manual_keys_sig.set(Vec::new());
                    step_sig.set(WizardStep::Verify);
                    // Phase 49.4 Plan 10 (D-15): the template path writes
                    // the starter persona NOW that the profile exists —
                    // never before, same "only after the write it depends
                    // on has actually succeeded" discipline the avatar save
                    // below already follows. The clone path never reaches
                    // here (clone_choice_owned routed through
                    // duplicate_profile above, which copies the source's
                    // own real persona) — writing a starter over a cloned
                    // persona would silently discard what was just copied.
                    // A failure here is intentionally swallowed: the
                    // profile itself was already created successfully, and
                    // an empty-but-real SOUL.md is not a failure state (the
                    // editor's own empty-state is a valid, documented case)
                    // — see this plan's SUMMARY for the sequencing note.
                    if entry_choice_owned == Some(CreateEntry::Template) {
                        let _ = crate::server::profile_api::save_profile_persona(
                            name_for_created.clone(),
                            STARTER_SOUL_MD.to_string(),
                        )
                        .await;
                    }
                    // Phase 50.1 Plan 04 (D-12): persist the avatar choice
                    // NOW that the profile exists — never before, since a
                    // save keyed to a not-yet-created profile name would
                    // pre-create a stray profile directory ahead of
                    // create_profile's own write. Awaited (not a second
                    // spawned task) so the drawer this wizard hands off
                    // into next always sees the saved avatar on its first
                    // fetch, never a race against a still-in-flight save.
                    // Pattern B (clippy.toml signal-borrow discipline): the
                    // owned local drops the GenerationalRef guard at this
                    // `;`, before the `if let`'s body ever reaches the
                    // `.await` below — `if let`'s scrutinee temporary
                    // otherwise lives across the whole block.
                    let avatar_snapshot = avatar_descriptor_for_submit.peek().clone();
                    if let Some(avatar) = avatar_snapshot {
                        let patch = crate::protocol::BotMetaPatch {
                            name: name_for_created.clone(),
                            title: None,
                            description: None,
                            avatar: Some(avatar),
                            group: None,
                            preview: None,
                            preview_at_ms: None,
                        };
                        let _ = crate::server::bot_meta_api::save_bot_meta(patch).await;
                    }
                    on_created.call(name_for_created);
                }
                Err(e) => create_error_sig.set(Some(format!("{e}"))),
            }
        });
    };

    rsx! {
        // Phase 49.4: pull in the kn-modal / kn-wizard styles so this dialog is
        // styled on every screen it can open from (Soul included), not just the
        // screens that happen to link kanban.css themselves.
        document::Link { rel: "stylesheet", href: WIZARD_CSS }
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
                    // Phase 50.1 Plan 02 (D-10/OF-1): the script-name eyebrow
                    // is kanban-specific copy — never rendered in bot
                    // context.
                    if matches!(context, ProfileDialogContext::Kanban) {
                        span {
                            style: "color: var(--accent-primary); font-size: var(--fs-11);",
                            "// SCRIPTS / MAKE-KANBAN-PROFILE"
                        }
                    }
                    h3 { class: "kn-modal-title", id: "kn-wizard-title", "{context.wizard_title()}" }
                    button {
                        class: "kn-drawer-close",
                        "aria-label": "Close create profile",
                        onclick: move |_| on_dismiss.call(()),
                        "✕"
                    }
                }
                // Phase 49.4 Plan 10 (D-15): the 1/2/3 step spine only
                // covers the wizard's pre-existing three steps — hidden
                // while on the new Entry/ClonePicker steps rather than
                // rendering with none of its three segments marked active.
                if !matches!(current_step, WizardStep::Entry | WizardStep::ClonePicker) {
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
                }
                div { class: "kn-modal-body",
                    if current_step == WizardStep::Entry {
                        p {
                            style: "font-size: var(--fs-12); color: var(--fg-dim); margin: 0 0 var(--sp-3) 0;",
                            "How do you want to create this profile?"
                        }
                        button {
                            class: "kn-modal-btn kn-modal-btn--submit",
                            style: "width: 100%; justify-content: flex-start; margin-bottom: var(--sp-2);",
                            onclick: move |_| {
                                entry_choice.set(Some(CreateEntry::Template));
                                clone_from_choice.set(CloneFromChoice::Empty);
                                step.set(WizardStep::Identity);
                            },
                            "Start from a template"
                        }
                        button {
                            class: "kn-modal-btn",
                            style: "width: 100%; justify-content: flex-start;",
                            onclick: move |_| {
                                entry_choice.set(Some(CreateEntry::Clone));
                                clone_from_choice.set(CloneFromChoice::CloneExisting);
                                step.set(WizardStep::ClonePicker);
                            },
                            "Clone an existing profile"
                        }
                    } else if current_step == WizardStep::ClonePicker {
                        div {
                            style: "font-size: var(--fs-11); color: var(--fg-dim); padding-bottom: var(--sp-2); overflow-wrap: anywhere;",
                            "SOUL.md and config only — API keys and secrets are never copied."
                        }
                        match known_profiles_resource() {
                            None => rsx! {
                                div { class: "kn-drawer-loading", "Loading profiles…" }
                            },
                            Some(Err(_)) => rsx! {
                                div { class: "kn-modal-error", "Could not read profiles. Check permissions and retry." }
                            },
                            Some(Ok(ref rows)) if rows.is_empty() => rsx! {
                                div { class: "kn-drawer-empty", "No existing profiles to clone from yet." }
                            },
                            Some(Ok(rows)) => rsx! {
                                div { class: "clone-picker",
                                    for row in rows.iter().cloned() {
                                        {
                                            let row_name = row.name.clone();
                                            let dot_color = if row.health == ProfileHealth::Incomplete {
                                                "var(--amber)"
                                            } else {
                                                "var(--accent-primary)"
                                            };
                                            rsx! {
                                                div {
                                                    key: "{row.name}",
                                                    class: "clone-picker-row",
                                                    title: "{row.name}",
                                                    onclick: move |_| {
                                                        clone_source.set(row_name.clone());
                                                        step.set(WizardStep::Identity);
                                                    },
                                                    span {
                                                        style: "overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1 1 auto;",
                                                        "{row.name}"
                                                    }
                                                    span {
                                                        style: "display:inline-block;width:6px;height:6px;border-radius:50%;background:{dot_color};margin-left:6px;flex-shrink:0;",
                                                        "aria-hidden": "true",
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    } else if current_step == WizardStep::Identity {
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
                        // Phase 50.1 Plan 02 (D-10/OF-1): this warning names
                        // kanban's non-interactive worker invocation — never
                        // rendered in bot context. Bot context instead shows
                        // the collapsed "Advanced" disclosure (OF-2) — header
                        // only, no fields yet; plan 50.1-05 fills its
                        // contents.
                        if matches!(context, ProfileDialogContext::Kanban) {
                            div {
                                style: "border-left: 2px solid var(--warn); background: color-mix(in srgb, var(--warn) 12%, var(--w-bg-3)); padding: var(--sp-2) var(--sp-4); font-size: var(--fs-11); color: var(--fg-dim);",
                                "Kanban workers run non-interactively (ironhermes --profile NAME chat -q), so the first-run wizard is skipped. Without config.yaml and a provider key the worker crashes at judge-build ~1s after spawn."
                            }
                        } else {
                            div { class: "kn-drawer-section",
                                button {
                                    class: "kn-action-btn",
                                    "aria-expanded": if *advanced_disclosure_open.read() { "true" } else { "false" },
                                    onclick: move |_| {
                                        let cur = *advanced_disclosure_open.read();
                                        advanced_disclosure_open.set(!cur);
                                    },
                                    if *advanced_disclosure_open.read() { "▾ ADVANCED" } else { "▸ ADVANCED" }
                                }
                                if *advanced_disclosure_open.read() {
                                    div { style: "padding-top: var(--sp-2);",
                                        // Phase 50.1 Plan 06 (D-17, UI-SPEC
                                        // Component Inventory §3): the
                                        // clone-from control — pre-creation
                                        // only, lives here alongside Avatar
                                        // for the same reason avatar does
                                        // (both are choosable before
                                        // `create_profile`/`duplicate_profile`
                                        // even runs). Never in the drawer —
                                        // a one-time creation operation, not
                                        // a live field.
                                        // Phase 49.4 Plan 10 (D-15): hidden
                                        // when `show_entry_step` is true —
                                        // the new Entry step already owns
                                        // this decision for that caller (the
                                        // Soul page), so this control would
                                        // otherwise let the operator set a
                                        // second, conflicting clone source.
                                        // The two pre-existing mounts never
                                        // set `show_entry_step`, so this
                                        // stays visible for them exactly as
                                        // before this plan.
                                        if !show_entry_step {
                                            div { class: "kn-drawer-section-label", "CLONE FROM" }
                                            div { class: "kn-modal-segmented",
                                                button {
                                                    class: if clone_choice_val == CloneFromChoice::Empty { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                                                    onclick: move |_| clone_from_choice.set(CloneFromChoice::Empty),
                                                    "Empty"
                                                }
                                                button {
                                                    class: if clone_choice_val == CloneFromChoice::CloneExisting { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                                                    onclick: move |_| clone_from_choice.set(CloneFromChoice::CloneExisting),
                                                    "Clone existing bot ▾"
                                                }
                                                button {
                                                    class: "kn-modal-seg",
                                                    disabled: true,
                                                    "aria-label": "Import (not available yet)",
                                                    "Import"
                                                }
                                            }
                                            if clone_choice_val == CloneFromChoice::CloneExisting {
                                                input {
                                                    class: "kn-modal-input",
                                                    placeholder: "bot to clone",
                                                    list: KN_CLONE_FROM_DATALIST_ID,
                                                    value: "{clone_source_val}",
                                                    oninput: move |evt| clone_source.set(evt.value()),
                                                }
                                                datalist { id: KN_CLONE_FROM_DATALIST_ID,
                                                    for known_name in known_profile_names.iter().cloned() {
                                                        option { key: "{known_name}", value: "{known_name}" }
                                                    }
                                                }
                                            }
                                            div {
                                                style: "font-size: var(--fs-11); color: var(--fg-dim); padding-bottom: var(--sp-2);",
                                                "Importing isn't available in this phase."
                                            }
                                        }
                                        div { class: "kn-drawer-section-label", "AVATAR" }
                                        AvatarPicker {
                                            bot_name: display_name_or_placeholder(&name_val),
                                            descriptor: avatar_descriptor,
                                            on_save: move |_: BotAvatarDescriptor| {
                                                // Phase 50.1 Plan 04: no-op here — the
                                                // profile does not exist yet while this
                                                // dialog is open. submit_create's success
                                                // arm reads avatar_descriptor.peek() and
                                                // persists it through save_bot_meta once
                                                // create_profile has actually succeeded.
                                            },
                                        }
                                    }
                                    div {
                                        style: "font-size: var(--fs-11); color: var(--fg-dim); padding-top: var(--sp-2);",
                                        "Model override, persona, and skills become available once your bot is created — expand ADVANCED again on the Verify step."
                                    }
                                }
                            }
                        }
                    } else if current_step == WizardStep::ConfigKeys {
                        // Phase 49.4.1 Plan 02 (D-04/D-07): SECRETS SOURCE
                        // renders FIRST — reading order top-to-bottom is
                        // provenance, then breadth (UI-SPEC "Visual
                        // hierarchy at the two mounts"). Both mounts share
                        // ONE component (D-05) and the SAME
                        // `secrets_source_availability` payload.
                        SecretsSourcePicker {
                            source: secrets_source,
                            vault_available,
                            vault_reason: vault_reason.clone(),
                            disabled: *creating.read(),
                            manual_keys,
                        }
                        label { class: "kn-modal-label", "{KEY_INHERITANCE_SECTION_LABEL}" }
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
                                    // Phase 49.4.1 Plan 02 (UI-SPEC E5
                                    // backstop): a source change re-runs
                                    // `preview_resource` (None while
                                    // in-flight) — if a previous resolution
                                    // exists, show it dimmed instead of
                                    // clearing to empty. An empty table must
                                    // always mean "this source resolved
                                    // nothing", never "still working".
                                    None => match last_resolved_preview.read().clone() {
                                        Some(previous_rows) => rsx! {
                                            div { style: "opacity: 0.5;", {render_key_table_rows(&previous_rows)} }
                                        },
                                        None => rsx! {
                                            div { class: "kn-drawer-loading", "Resolving keys from ~/.ironhermes/.env…" }
                                        },
                                    },
                                    Some(Err(_)) => rsx! {
                                        div { class: "kn-modal-error", "Could not read ~/.ironhermes/.env. Check permissions and retry." }
                                    },
                                    Some(Ok(rows)) => render_key_table_rows(rows),
                                }
                            }
                        }
                        if show_manual_key_field {
                            // Phase 49.4.1 Plan 03 (D-04/D-05): the manual
                            // key field this hint used to render inline now
                            // lives inside the SecretsSourcePicker's
                            // Provided-keys row (D-05's one component) —
                            // pre-seeded with MANUAL_KEY_TARGET by the
                            // effect above, so this stays a pointer to that
                            // row rather than a second input.
                            div { style: "font-size: var(--fs-11); color: var(--fg-dim);",
                                "No LLM provider key resolved above. A kanban worker with no provider key crashes at judge-build. Select \"Provided keys\" above and enter one, or add it to the root .env and switch mode."
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
                            // Phase 49.4.1 Plan 02 (D-06, UI-SPEC Copywriting
                            // Contract): the secondary explanatory line
                            // beneath the D-06 fail-loud box — same
                            // `.kn-modal-hint--info` treatment the drawer's
                            // SYNC error uses.
                            div { class: "kn-modal-hint--info",
                                "Add the key to that source, or pick a different source, then try again."
                            }
                        }
                    } else {
                        // ---- Step 3: VERIFY ----
                        div { style: "font-weight: 700; font-size: var(--fs-13); color: var(--fg);",
                            span { "aria-hidden": "true", "▊ " }
                            "VERIFY — IRONHERMES --PROFILE {name_val} DOCTOR"
                        }
                        {render_verify_doctor_block(&name_val, &resolved_report, context)}
                        // Phase 50.1 Plan 05 (D-15/D-16, OF-2 amendment): the
                        // collapsed "Advanced" disclosure plan 02 shipped in
                        // the Identity step covers only the avatar — choosable
                        // before the profile exists. Model override, persona
                        // and skills all write through a profile's own
                        // config.yaml / workspace directory, neither of which
                        // exists until `create_profile` has already
                        // succeeded — reached by this Verify step, never step
                        // 1. Reuses the SAME `advanced_disclosure_open` signal
                        // plan 02 registered, so an operator who expanded
                        // "Advanced" in step 1 sees it still expanded here.
                        // Never rendered for Kanban — clone-from/model/
                        // persona/skills are D-15/D-16 bot concepts.
                        if matches!(context, ProfileDialogContext::Bot) {
                            div { class: "kn-drawer-section",
                                button {
                                    class: "kn-action-btn",
                                    "aria-expanded": if *advanced_disclosure_open.read() { "true" } else { "false" },
                                    onclick: move |_| {
                                        let cur = *advanced_disclosure_open.read();
                                        advanced_disclosure_open.set(!cur);
                                    },
                                    if *advanced_disclosure_open.read() { "▾ ADVANCED" } else { "▸ ADVANCED" }
                                }
                                if *advanced_disclosure_open.read() {
                                    div { style: "padding-top: var(--sp-2);",
                                        AdvancedProfilePane {
                                            key: "{name_val}",
                                            bot_name: name_val.clone(),
                                            on_saved: move |_| {},
                                        }
                                    }
                                }
                            }
                        }
                        // Phase 50.1 Plan 02 (D-10/OF-1): kanban-dispatch CLI
                        // assignment copy — never rendered in bot context.
                        if matches!(context, ProfileDialogContext::Kanban) {
                            div { class: "kn-drawer-section",
                                div { class: "kn-drawer-section-label", "ASSIGN WORK TO THIS PROFILE" }
                                pre {
                                    style: "font-family: var(--font-mono); font-size: var(--fs-11); color: var(--fg-dim); white-space: pre-wrap; overflow-wrap: anywhere;",
                                    "ironhermes kanban create \"your task title\" --assignee {name_val}\n# or reassign an existing task\nironhermes kanban assign <task_id> {name_val}"
                                }
                            }
                        }
                    }
                }
                div { class: "kn-wizard-footer",
                    // Phase 50.1 Plan 02 (D-10/OF-1): the live
                    // `scripts/make-kanban-profile …` command preview is
                    // kanban-specific CLI copy — never rendered in bot
                    // context.
                    if matches!(context, ProfileDialogContext::Kanban) {
                        span { class: "kn-wizard-preview", "$ {command_preview}" }
                    }
                    div { style: "display: flex; gap: var(--sp-2);",
                        button {
                            class: "kn-modal-btn",
                            onclick: move |_| {
                                match current_step {
                                    WizardStep::Entry => on_dismiss.call(()),
                                    WizardStep::Identity if !show_entry_step => on_dismiss.call(()),
                                    _ => go_back(),
                                }
                            },
                            if current_step == WizardStep::Entry
                                || (current_step == WizardStep::Identity && !show_entry_step)
                            {
                                "CANCEL"
                            } else {
                                "← BACK"
                            }
                        }
                        if current_step == WizardStep::Entry || current_step == WizardStep::ClonePicker {
                            // Phase 49.4 Plan 10 (D-15): no footer action
                            // button on these two steps — progression
                            // happens via the body's own controls (the
                            // entry-choice buttons, or clicking a
                            // clone-picker row).
                        } else if current_step == WizardStep::Identity {
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

    /// D-07: step 2's SECRETS SOURCE eyebrow must render ABOVE the
    /// pre-existing KEY INHERITANCE eyebrow — provenance before breadth,
    /// per UI-SPEC "Visual hierarchy at the two mounts". Machine-checkable
    /// via the SAME ordered definition the rsx! mounts from
    /// ([`render_step_two_sections_order`]), so the seat and the actual
    /// render cannot drift apart.
    #[test]
    fn wizard_step_two_renders_secrets_source_above_key_inheritance() {
        let order = render_step_two_sections_order();
        let secrets_source_idx = order
            .iter()
            .position(|s| *s == super::super::secrets_source_picker::SECRETS_SOURCE_SECTION_LABEL)
            .expect("SECRETS SOURCE eyebrow must be present");
        let key_inheritance_idx = order
            .iter()
            .position(|s| *s == KEY_INHERITANCE_SECTION_LABEL)
            .expect("KEY INHERITANCE eyebrow must be present");
        assert!(
            secrets_source_idx < key_inheritance_idx,
            "SECRETS SOURCE must render before KEY INHERITANCE: {order:?}"
        );
    }
}
