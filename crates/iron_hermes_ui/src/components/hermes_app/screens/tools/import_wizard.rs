//! Phase 48.2 Plan 04 Task 2 (D-01/D-04/D-19/D-22): the DEFINE → VERIFY →
//! COMMIT import wizard, driven entirely by the Plan 02 admin API
//! (`crate::server::mcp_admin_api`). Reuses the profile create-dialog's step
//! interaction shape (`profile_shared/create_dialog.rs`) — a `Signal<WizardStep>`
//! spine, a back-navigation match table, and an explicit-trigger VERIFY
//! resource — but never that dialog's zero-radius kanban-board CSS chrome
//! (D-19 prohibition): every class here is new, styled against this
//! page's rounded `site.css`/`screens.css` token layer.
//!
//! # DEFINE owns every editable field; VERIFY/COMMIT are read-only
//!
//! Every operator-editable input (paste textarea, manual form) lives ONLY
//! in the DEFINE step's render branch. This is deliberate: it guarantees
//! the draft `probe_mcp_server` receives and the draft `commit_mcp_server`
//! receives are built from the SAME, un-mutated signal snapshot — "the
//! probed config IS the committed config" (D-04) would otherwise need a
//! separate snapshot-and-freeze mechanism if fields stayed editable after
//! leaving DEFINE.
//!
//! # COMMIT is a real gate, not a client-side guess (D-04)
//!
//! [`commit_is_enabled`] is driven by [`McpProbeResult::passed`] alone —
//! the SAME boolean `probe_mcp_server` computed server-side. The client
//! never re-derives "did the probe succeed" from status matching; it only
//! adds the `is_probing` bit (never enabled mid-flight).

use crate::server::mcp_admin_api::{
    McpDiscoveredTool, McpProbeResult, McpServerDraft, McpSnippetEntry, McpSnippetParse,
    McpTransportKind,
};
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

// =============================================================================
// Pure helpers (unit-tested — Task 2 acceptance criteria).
// =============================================================================

/// Step spine. Display-only click-to-jump is not offered (mirrors
/// `create_dialog.rs`'s own `WizardStep` — footer-driven `next`/`back`
/// only).
#[allow(dead_code)] // used inside ImportWizard rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WizardStep {
    Define,
    Verify,
    Commit,
}

/// Back-navigation table: `Define` maps to itself (nothing to go back to),
/// `Verify` back to `Define`, `Commit` back to `Verify`.
#[allow(dead_code)] // used inside ImportWizard; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn back_target(step: WizardStep) -> WizardStep {
    match step {
        WizardStep::Define => WizardStep::Define,
        WizardStep::Verify => WizardStep::Define,
        WizardStep::Commit => WizardStep::Verify,
    }
}

/// COMMIT's real gate (D-04): disabled with no probe result, disabled
/// while probing, disabled after a failed non-OAuth probe (`passed: false`),
/// enabled after a passing probe with zero tools OR an `AUTH REQUIRED`
/// result — both of which `probe_mcp_server` already reports as
/// `passed: true`, so this fn never re-derives pass/fail from status
/// matching, only adds the "never enabled mid-probe" rule.
#[allow(dead_code)] // used inside ImportWizard; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn commit_is_enabled(is_probing: bool, probe: &Option<McpProbeResult>) -> bool {
    if is_probing {
        return false;
    }
    probe.as_ref().is_some_and(|result| result.passed)
}

/// Parse a textarea of `KEY=VALUE` lines into ordered pairs — blank lines
/// and keyless lines are dropped, never panicking on malformed input.
#[allow(dead_code)] // used inside ImportWizard/build_manual_draft; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn parse_kv_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(2, '=');
            let key = parts.next()?.trim().to_string();
            let value = parts.next().unwrap_or("").trim().to_string();
            if key.is_empty() { None } else { Some((key, value)) }
        })
        .collect()
}

/// Render `KEY=VALUE` pairs back into the same textarea shape
/// [`parse_kv_lines`] reads, so re-selecting an entry round-trips cleanly.
#[allow(dead_code)] // used inside ImportWizard/apply_draft_to_manual_signals; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn kv_pairs_to_text(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Space-separated argv parsing for the stdio `args` field.
#[allow(dead_code)] // used inside ImportWizard/build_manual_draft; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn parse_args(text: &str) -> Vec<String> {
    text.split_whitespace().map(|s| s.to_string()).collect()
}

/// Build the draft that will be probed/committed from the manual form's
/// plain field values — pure, so it never needs a running server to test.
#[allow(dead_code)] // used inside ImportWizard; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
#[allow(clippy::too_many_arguments)]
fn build_manual_draft(
    name: &str,
    transport: &McpTransportKind,
    command: &str,
    args_text: &str,
    env_text: &str,
    url: &str,
    headers_text: &str,
    oauth_provider: &str,
    allowed_issuer: &str,
    timeout: u64,
    connect_timeout: u64,
    enabled: bool,
) -> McpServerDraft {
    let is_stdio = matches!(transport, McpTransportKind::Stdio);
    McpServerDraft {
        name: name.trim().to_string(),
        transport: transport.clone(),
        command: if is_stdio && !command.trim().is_empty() {
            Some(command.trim().to_string())
        } else {
            None
        },
        args: if is_stdio { parse_args(args_text) } else { Vec::new() },
        env: if is_stdio { parse_kv_lines(env_text) } else { Vec::new() },
        url: if !is_stdio && !url.trim().is_empty() {
            Some(url.trim().to_string())
        } else {
            None
        },
        headers: if is_stdio { Vec::new() } else { parse_kv_lines(headers_text) },
        enabled,
        timeout,
        connect_timeout,
        oauth_provider: if !is_stdio && !oauth_provider.trim().is_empty() {
            Some(oauth_provider.trim().to_string())
        } else {
            None
        },
        allowed_issuer: if !is_stdio && !allowed_issuer.trim().is_empty() {
            Some(allowed_issuer.trim().to_string())
        } else {
            None
        },
    }
}

/// CR-01 mirror of `ironhermes_mcp::sanitize_server_name`'s round-trip check
/// (the same check `commit_mcp_server` now enforces server-side,
/// `mcp_admin_api.rs`). This crate's wasm client build never compiles
/// `ironhermes-mcp` (Cargo.toml `[target.'cfg(not(target_arch =
/// "wasm32"))'.dependencies]`), so this is a local, dependency-free copy of
/// the same `[A-Za-z0-9_]` character class rather than a call into that
/// native-only crate. A name is "sanitized" (round-trips) iff it is
/// non-empty and every character is already in the allowed class.
#[allow(dead_code)] // used inside ImportWizard/draft_ready_for_verify; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn name_is_sanitized(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// DEFINE→VERIFY gate: a name plus the transport's required field
/// (`command` for stdio, `url` for http) must be present before the
/// operator can advance — VERIFY's own server-side probe is the real
/// gate (D-04); this is only "is there enough to probe." CR-01: also
/// refuses a name containing characters outside `[A-Za-z0-9_]`, mirroring
/// `commit_mcp_server`'s server-side gate, so NEXT/COMMIT are visibly
/// disabled here rather than failing only at the commit round-trip (D-03:
/// disabled controls are visibly disabled, never live-but-silent).
#[allow(dead_code)] // used inside ImportWizard; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn draft_ready_for_verify(draft: &McpServerDraft) -> bool {
    if draft.name.trim().is_empty() {
        return false;
    }
    if !name_is_sanitized(&draft.name) {
        return false;
    }
    match draft.transport {
        McpTransportKind::Stdio => draft.command.is_some(),
        McpTransportKind::Http => draft.url.is_some(),
    }
}

/// Copy a successfully-parsed snippet entry's draft into the manual form
/// signals — used both by explicit row selection and by the DEFINE step's
/// auto-select-when-exactly-one-draft rule.
#[allow(dead_code)] // used inside ImportWizard; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
#[allow(clippy::too_many_arguments)]
fn apply_draft_to_manual_signals(
    draft: &McpServerDraft,
    mut manual_name: Signal<String>,
    mut manual_transport: Signal<McpTransportKind>,
    mut manual_command: Signal<String>,
    mut manual_args: Signal<String>,
    mut manual_env_text: Signal<String>,
    mut manual_url: Signal<String>,
    mut manual_headers_text: Signal<String>,
    mut manual_oauth_provider: Signal<String>,
    mut manual_allowed_issuer: Signal<String>,
    mut manual_timeout: Signal<u64>,
    mut manual_connect_timeout: Signal<u64>,
    mut manual_enabled: Signal<bool>,
) {
    manual_name.set(draft.name.clone());
    manual_transport.set(draft.transport.clone());
    manual_command.set(draft.command.clone().unwrap_or_default());
    manual_args.set(draft.args.join(" "));
    manual_env_text.set(kv_pairs_to_text(&draft.env));
    manual_url.set(draft.url.clone().unwrap_or_default());
    manual_headers_text.set(kv_pairs_to_text(&draft.headers));
    manual_oauth_provider.set(draft.oauth_provider.clone().unwrap_or_default());
    manual_allowed_issuer.set(draft.allowed_issuer.clone().unwrap_or_default());
    manual_timeout.set(draft.timeout);
    manual_connect_timeout.set(draft.connect_timeout);
    manual_enabled.set(draft.enabled);
}

/// Fire the COMMIT write (D-04: the same draft that was probed). A plain
/// fn, not a shared closure — `scope`/`draft` are owned per call so this
/// can be invoked from both the VERIFY step's COMMIT button and the
/// COMMIT step's RETRY COMMIT button without a move conflict (closures
/// capturing non-`Copy` values can only be used from one call site).
#[allow(dead_code)] // used inside ImportWizard rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
#[allow(clippy::too_many_arguments)]
fn spawn_commit(
    scope: ConfigScope,
    draft: McpServerDraft,
    mut step: Signal<WizardStep>,
    mut committing: Signal<bool>,
    mut commit_error: Signal<Option<String>>,
    mut refresh_tick: Signal<u32>,
    on_close: EventHandler<()>,
) {
    step.set(WizardStep::Commit);
    committing.set(true);
    commit_error.set(None);
    spawn(async move {
        match crate::server::mcp_admin_api::commit_mcp_server(scope, draft).await {
            Ok(_row) => {
                committing.set(false);
                let cur = *refresh_tick.read();
                refresh_tick.set(cur + 1);
                on_close.call(());
            }
            Err(e) => {
                committing.set(false);
                commit_error.set(Some(e.to_string()));
            }
        }
    });
}

// =============================================================================
// Components
// =============================================================================

/// The DEFINE → VERIFY → COMMIT wizard. Mounted from `mcp_section.rs`,
/// gated on a local `wizard_open` signal there; `scope`/`refresh_tick` are
/// the section's own shared signals so a successful COMMIT refreshes the
/// row list (and, via the shared page-level tick, the catalog's MCP
/// display group).
#[component]
pub fn ImportWizard(
    scope: ConfigScope,
    mut refresh_tick: Signal<u32>,
    on_close: EventHandler<()>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut step: Signal<WizardStep> = use_signal(|| WizardStep::Define);

    // ---- DEFINE: paste path ----
    let mut paste_text: Signal<String> = use_signal(String::new);
    let snippet_parse: Signal<Option<McpSnippetParse>> = use_signal(|| None);
    let mut parsing: Signal<bool> = use_signal(|| false);
    let mut selected_entry_name: Signal<Option<String>> = use_signal(|| None);

    // ---- DEFINE: manual form (the ONLY editable field surface — see
    // module doc for why VERIFY/COMMIT never expose these) ----
    let mut manual_name: Signal<String> = use_signal(String::new);
    let mut manual_transport: Signal<McpTransportKind> = use_signal(|| McpTransportKind::Stdio);
    let mut manual_command: Signal<String> = use_signal(String::new);
    let mut manual_args: Signal<String> = use_signal(String::new);
    let mut manual_env_text: Signal<String> = use_signal(String::new);
    let mut manual_url: Signal<String> = use_signal(String::new);
    let mut manual_headers_text: Signal<String> = use_signal(String::new);
    let mut manual_oauth_provider: Signal<String> = use_signal(String::new);
    let mut manual_allowed_issuer: Signal<String> = use_signal(String::new);
    let manual_timeout: Signal<u64> = use_signal(|| 120);
    let manual_connect_timeout: Signal<u64> = use_signal(|| 60);
    let manual_enabled: Signal<bool> = use_signal(|| true);

    // ---- VERIFY ----
    let mut probe_trigger: Signal<u32> = use_signal(|| 0);
    let commit_error: Signal<Option<String>> = use_signal(|| None);
    let committing: Signal<bool> = use_signal(|| false);

    // Auto-select the single successfully-parsed entry into the manual
    // form (D-01/D-02 partial-parse UX — "when exactly one draft results,
    // it is preselected").
    use_effect(move || {
        if let Some(parse) = snippet_parse.read().clone() {
            let successful: Vec<&McpSnippetEntry> =
                parse.entries.iter().filter(|e| e.draft.is_some()).collect();
            if successful.len() == 1 {
                if let Some(draft) = &successful[0].draft {
                    selected_entry_name.set(Some(successful[0].name.clone()));
                    apply_draft_to_manual_signals(
                        draft,
                        manual_name,
                        manual_transport,
                        manual_command,
                        manual_args,
                        manual_env_text,
                        manual_url,
                        manual_headers_text,
                        manual_oauth_provider,
                        manual_allowed_issuer,
                        manual_timeout,
                        manual_connect_timeout,
                        manual_enabled,
                    );
                }
            }
        }
    });

    // ---- Derived values (read BEFORE rsx!/async — no signal borrow held
    // across the macro, iron_hermes_ui/clippy.toml). ----
    let current_step = *step.read();
    let name_val = manual_name.read().clone();
    let transport_val = manual_transport.read().clone();
    let command_val = manual_command.read().clone();
    let args_val = manual_args.read().clone();
    let env_val = manual_env_text.read().clone();
    let url_val = manual_url.read().clone();
    let headers_val = manual_headers_text.read().clone();
    let oauth_provider_val = manual_oauth_provider.read().clone();
    let allowed_issuer_val = manual_allowed_issuer.read().clone();
    let timeout_val = *manual_timeout.read();
    let connect_timeout_val = *manual_connect_timeout.read();
    let enabled_val = *manual_enabled.read();
    let is_stdio = matches!(transport_val, McpTransportKind::Stdio);

    let current_draft = build_manual_draft(
        &name_val,
        &transport_val,
        &command_val,
        &args_val,
        &env_val,
        &url_val,
        &headers_val,
        &oauth_provider_val,
        &allowed_issuer_val,
        timeout_val,
        connect_timeout_val,
        enabled_val,
    );
    let ready_for_verify = draft_ready_for_verify(&current_draft);
    // CR-01/D-03: show WHY the NAME field blocks NEXT, rather than leaving
    // NEXT silently disabled with no visible reason. Checked against
    // `current_draft.name` (already trimmed by `build_manual_draft`) so this
    // agrees exactly with what `draft_ready_for_verify` just evaluated.
    let name_invalid = !current_draft.name.is_empty() && !name_is_sanitized(&current_draft.name);

    // ---- VERIFY probe resource — only actually calls the server while
    // `step == Verify` (create_dialog.rs verify_resource precedent);
    // `probe_trigger` forces RETRY without ever calling a resource-restart
    // method. Reads the draft's plain values in the sync prefix so a
    // signal change on ANY manual field re-probes automatically (harmless:
    // fields are read-only once this step is reached, per module doc). ----
    let scope_for_probe = scope.clone();
    let draft_for_probe = current_draft.clone();
    let probe_resource = use_resource(move || {
        let on_verify_step = *step.read() == WizardStep::Verify;
        let _trigger = probe_trigger();
        let scope_value = scope_for_probe.clone();
        let draft_value = draft_for_probe.clone();
        async move {
            if !on_verify_step {
                return None;
            }
            Some(crate::server::mcp_admin_api::probe_mcp_server(scope_value, draft_value).await)
        }
    });
    let probe_snapshot = probe_resource();
    let is_probing = current_step == WizardStep::Verify
        && matches!(probe_snapshot, None | Some(None));
    let (probe_result, probe_infra_error): (Option<McpProbeResult>, Option<String>) =
        match probe_snapshot {
            Some(Some(Ok(result))) => (Some(result), None),
            Some(Some(Err(e))) => (None, Some(e.to_string())),
            _ => (None, None),
        };
    let commit_enabled = commit_is_enabled(is_probing, &probe_result);

    // ---- Handlers ----
    let mut go_back = move |_| {
        let cur = *step.read();
        step.set(back_target(cur));
    };

    rsx! {
        div { class: "mcp-wizard-overlay", role: "presentation",
            div {
                class: "mcp-wizard",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "mcp-wizard-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        on_close.call(());
                    }
                },
                div { class: "mcp-wizard-header",
                    h3 { class: "mcp-wizard-title", id: "mcp-wizard-title", "IMPORT MCP SERVER" }
                    button {
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Close import wizard",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                div { class: "mcp-wizard-steps",
                    span {
                        class: if current_step == WizardStep::Define { "mcp-wizard-seg mcp-wizard-seg--active" } else { "mcp-wizard-seg mcp-wizard-seg--done" },
                        "1 · DEFINE"
                    }
                    span {
                        class: if current_step == WizardStep::Verify {
                            "mcp-wizard-seg mcp-wizard-seg--active"
                        } else if current_step == WizardStep::Commit {
                            "mcp-wizard-seg mcp-wizard-seg--done"
                        } else {
                            "mcp-wizard-seg"
                        },
                        "2 · VERIFY"
                    }
                    span {
                        class: if current_step == WizardStep::Commit { "mcp-wizard-seg mcp-wizard-seg--active" } else { "mcp-wizard-seg" },
                        "3 · COMMIT"
                    }
                }
                div { class: "mcp-wizard-body",
                    if current_step == WizardStep::Define {
                        label { class: "tools-settings-label", "PASTE — CLAUDE-DESKTOP JSON OR mcp_servers: YAML" }
                        textarea {
                            class: "mcp-wizard-textarea",
                            placeholder: "Paste a Claude-Desktop-style mcpServers JSON block, or a raw mcp_servers: YAML fragment",
                            value: "{paste_text}",
                            oninput: move |evt| paste_text.set(evt.value()),
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: paste_text.read().trim().is_empty() || *parsing.read(),
                            onclick: move |_| {
                                let text = paste_text.read().clone();
                                parsing.set(true);
                                let mut parsing_sig = parsing;
                                let mut snippet_parse_sig = snippet_parse;
                                spawn(async move {
                                    let result = crate::server::mcp_admin_api::parse_mcp_snippet(text).await;
                                    parsing_sig.set(false);
                                    if let Ok(parse) = result {
                                        snippet_parse_sig.set(Some(parse));
                                    }
                                });
                            },
                            if *parsing.read() { "PARSING…" } else { "PARSE SNIPPET" }
                        }
                        if let Some(parse) = snippet_parse.read().clone() {
                            SnippetEntryList {
                                parse,
                                selected_entry_name: selected_entry_name.read().clone(),
                                on_select: move |draft: McpServerDraft| {
                                    selected_entry_name.set(Some(draft.name.clone()));
                                    apply_draft_to_manual_signals(
                                        &draft,
                                        manual_name,
                                        manual_transport,
                                        manual_command,
                                        manual_args,
                                        manual_env_text,
                                        manual_url,
                                        manual_headers_text,
                                        manual_oauth_provider,
                                        manual_allowed_issuer,
                                        manual_timeout,
                                        manual_connect_timeout,
                                        manual_enabled,
                                    );
                                },
                            }
                        }
                        label { class: "tools-settings-label", "OR FILL THE FORM" }
                        div { class: "tools-settings-field-group",
                            label { class: "tools-settings-label", "NAME" }
                            input {
                                class: "tools-settings-input",
                                value: "{name_val}",
                                oninput: move |evt| manual_name.set(evt.value()),
                            }
                            if name_invalid {
                                div { class: "mcp-wizard-probe-error",
                                    "server name '{current_draft.name}' contains characters outside [A-Za-z0-9_]; rename it before committing"
                                }
                            }
                        }
                        div { class: "tools-settings-field-group",
                            label { class: "tools-settings-label", "TRANSPORT" }
                            select {
                                class: "tools-settings-select",
                                onchange: move |evt| {
                                    manual_transport.set(
                                        if evt.value() == "http" { McpTransportKind::Http } else { McpTransportKind::Stdio },
                                    );
                                },
                                option { value: "stdio", selected: is_stdio, "stdio" }
                                option { value: "http", selected: !is_stdio, "http" }
                            }
                        }
                        if is_stdio {
                            div { class: "tools-settings-field-group",
                                label { class: "tools-settings-label", "COMMAND" }
                                input {
                                    class: "tools-settings-input",
                                    value: "{command_val}",
                                    oninput: move |evt| manual_command.set(evt.value()),
                                }
                            }
                            div { class: "tools-settings-field-group",
                                label { class: "tools-settings-label", "ARGS — SPACE-SEPARATED" }
                                input {
                                    class: "tools-settings-input",
                                    value: "{args_val}",
                                    oninput: move |evt| manual_args.set(evt.value()),
                                }
                            }
                            div { class: "tools-settings-field-group",
                                label { class: "tools-settings-label", "ENV — ONE KEY=VALUE PER LINE" }
                                textarea {
                                    class: "mcp-wizard-textarea mcp-wizard-textarea--sm",
                                    value: "{env_val}",
                                    oninput: move |evt| manual_env_text.set(evt.value()),
                                }
                            }
                        } else {
                            div { class: "tools-settings-field-group",
                                label { class: "tools-settings-label", "URL" }
                                input {
                                    class: "tools-settings-input",
                                    value: "{url_val}",
                                    oninput: move |evt| manual_url.set(evt.value()),
                                }
                            }
                            div { class: "tools-settings-field-group",
                                label { class: "tools-settings-label", "HEADERS — ONE KEY=VALUE PER LINE" }
                                textarea {
                                    class: "mcp-wizard-textarea mcp-wizard-textarea--sm",
                                    value: "{headers_val}",
                                    oninput: move |evt| manual_headers_text.set(evt.value()),
                                }
                            }
                            div { class: "tools-settings-field-group",
                                label { class: "tools-settings-label", "OAUTH PROVIDER — OPTIONAL" }
                                input {
                                    class: "tools-settings-input",
                                    value: "{oauth_provider_val}",
                                    oninput: move |evt| manual_oauth_provider.set(evt.value()),
                                }
                            }
                            div { class: "tools-settings-field-group",
                                label { class: "tools-settings-label", "ALLOWED ISSUER — OPTIONAL" }
                                input {
                                    class: "tools-settings-input",
                                    value: "{allowed_issuer_val}",
                                    oninput: move |evt| manual_allowed_issuer.set(evt.value()),
                                }
                            }
                        }
                    } else if current_step == WizardStep::Verify {
                        div { class: "tools-settings-label", "VERIFY — {name_val}" }
                        if is_probing {
                            div { class: "mcp-wizard-probing",
                                div { class: "mcp-wizard-probing-bar" }
                                span { "PROBING…" }
                            }
                        } else if let Some(reason) = probe_infra_error.clone() {
                            div { class: "mcp-wizard-probe-error",
                                "COULD NOT REACH SERVER — {reason}"
                            }
                            button {
                                class: "btn btn--ghost btn--sm",
                                onclick: move |_| {
                                    let cur = *probe_trigger.read();
                                    probe_trigger.set(cur + 1);
                                },
                                "RETRY"
                            }
                        } else if let Some(result) = probe_result.clone() {
                            if !result.passed {
                                div { class: "mcp-wizard-probe-error",
                                    "COULD NOT REACH SERVER — {result.message.clone().unwrap_or_default()}"
                                }
                                button {
                                    class: "btn btn--ghost btn--sm",
                                    onclick: move |_| {
                                        let cur = *probe_trigger.read();
                                        probe_trigger.set(cur + 1);
                                    },
                                    "RETRY"
                                }
                            } else if matches!(result.status, crate::server::mcp_admin_api::McpServerStatus::AuthRequired) {
                                div { class: "mcp-wizard-probe-pass",
                                    "AUTH REQUIRED — this server needs OAuth authorization. The CONNECT action on its row completes authorization after COMMIT."
                                }
                            } else if result.tools.is_empty() {
                                div { class: "tools-empty-heading", "NO TOOLS DISCOVERED" }
                                div { class: "tools-empty-body",
                                    "The server responded but exposed no tools. Check its configuration, or COMMIT anyway to register it for later."
                                }
                            } else {
                                DiscoveredToolsChecklist { tools: result.tools.clone() }
                            }
                        }
                    } else {
                        div { class: "tools-settings-label", "COMMIT — {name_val}" }
                        if *committing.read() {
                            div { "COMMITTING…" }
                        } else if let Some(err) = commit_error.read().clone() {
                            span { class: "pill red tools-save-failed-chip", "SAVE FAILED — {err}." }
                        }
                    }
                }
                div { class: "mcp-wizard-footer",
                    button {
                        class: "btn btn--ghost btn--sm",
                        disabled: current_step == WizardStep::Commit && *committing.read(),
                        onclick: move |evt| {
                            if current_step == WizardStep::Define {
                                on_close.call(());
                            } else {
                                go_back(evt);
                            }
                        },
                        if current_step == WizardStep::Define { "CANCEL" } else { "← BACK" }
                    }
                    if current_step == WizardStep::Define {
                        button {
                            class: "btn",
                            disabled: !ready_for_verify,
                            onclick: move |_| {
                                if ready_for_verify {
                                    step.set(WizardStep::Verify);
                                }
                            },
                            "NEXT →"
                        }
                    } else if current_step == WizardStep::Verify {
                        button {
                            class: "btn",
                            disabled: !commit_enabled,
                            onclick: {
                                let scope_for_commit = scope.clone();
                                let draft_for_commit = current_draft.clone();
                                move |_| {
                                    if commit_enabled {
                                        spawn_commit(
                                            scope_for_commit.clone(),
                                            draft_for_commit.clone(),
                                            step,
                                            committing,
                                            commit_error,
                                            refresh_tick,
                                            on_close,
                                        );
                                    }
                                }
                            },
                            "COMMIT"
                        }
                    } else if let Some(_err) = commit_error.read().clone() {
                        button {
                            class: "btn",
                            onclick: {
                                let scope_for_retry = scope.clone();
                                let draft_for_retry = current_draft.clone();
                                move |_| {
                                    spawn_commit(
                                        scope_for_retry.clone(),
                                        draft_for_retry.clone(),
                                        step,
                                        committing,
                                        commit_error,
                                        refresh_tick,
                                        on_close,
                                    );
                                }
                            },
                            "RETRY COMMIT"
                        }
                    }
                }
            }
        }
    }
}

/// Per-entry parse results at the DEFINE→VERIFY handoff (D-01/D-02
/// partial-parse isolation): each successfully-parsed entry is a
/// selectable draft; each failed entry names its own reason. Only
/// successfully-parsed entries are clickable.
#[component]
fn SnippetEntryList(
    parse: McpSnippetParse,
    selected_entry_name: Option<String>,
    on_select: EventHandler<McpServerDraft>,
) -> Element {
    rsx! {
        div { class: "mcp-wizard-snippet-list",
            for entry in parse.entries.iter().cloned() {
                {
                    let is_selected = selected_entry_name.as_deref() == Some(entry.name.as_str());
                    match entry.draft.clone() {
                        Some(draft) => rsx! {
                            button {
                                key: "{entry.name}",
                                class: if is_selected { "mcp-wizard-snippet-entry mcp-wizard-snippet-entry--selected" } else { "mcp-wizard-snippet-entry" },
                                r#type: "button",
                                onclick: move |_| on_select.call(draft.clone()),
                                "✓ {entry.name}"
                            }
                        },
                        None => rsx! {
                            div {
                                key: "{entry.name}",
                                class: "mcp-wizard-snippet-entry mcp-wizard-snippet-entry--error",
                                "✕ {entry.name} — {entry.error.clone().unwrap_or_default()}"
                            }
                        },
                    }
                }
            }
        }
    }
}

/// The probe's discovered tools — `DISCOVERED TOOLS ({count})` with a
/// numeral header, no singular/plural copy branch, bounded-height and
/// scrolling past ~20 rows.
#[component]
fn DiscoveredToolsChecklist(tools: Vec<McpDiscoveredTool>) -> Element {
    let count = tools.len();
    rsx! {
        div { class: "tools-settings-label", "DISCOVERED TOOLS ({count})" }
        div { class: "mcp-wizard-tools-checklist",
            for tool in tools.iter().cloned() {
                div { key: "{tool.name}", class: "mcp-wizard-tool-row",
                    div { class: "mcp-wizard-tool-name", title: "{tool.name}", "{tool.name}" }
                    div { class: "mcp-wizard-tool-desc", title: "{tool.description}", "{tool.description}" }
                }
            }
        }
    }
}

// =============================================================================
// Pure-fn tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::mcp_admin_api::McpServerStatus;

    // --- back_target -------------------------------------------------------

    #[test]
    fn back_target_covers_all_three_steps() {
        assert_eq!(back_target(WizardStep::Define), WizardStep::Define);
        assert_eq!(back_target(WizardStep::Verify), WizardStep::Define);
        assert_eq!(back_target(WizardStep::Commit), WizardStep::Verify);
    }

    // --- commit_is_enabled ---------------------------------------------------

    fn passing_probe(status: McpServerStatus, tools: Vec<McpDiscoveredTool>) -> McpProbeResult {
        McpProbeResult { passed: true, status, tools, message: None }
    }

    fn failing_probe() -> McpProbeResult {
        McpProbeResult {
            passed: false,
            status: McpServerStatus::SpawnFailed { reason: "boom".to_string() },
            tools: Vec::new(),
            message: Some("could not connect".to_string()),
        }
    }

    #[test]
    fn commit_disabled_with_no_probe_result() {
        assert!(!commit_is_enabled(false, &None));
    }

    #[test]
    fn commit_disabled_while_probing() {
        let some_pass = Some(passing_probe(McpServerStatus::Connected, Vec::new()));
        assert!(!commit_is_enabled(true, &some_pass));
        assert!(!commit_is_enabled(true, &None));
    }

    #[test]
    fn commit_disabled_after_a_failed_non_oauth_probe() {
        assert!(!commit_is_enabled(false, &Some(failing_probe())));
    }

    #[test]
    fn commit_enabled_after_a_passing_probe_with_zero_tools() {
        let pass = Some(passing_probe(McpServerStatus::Connected, Vec::new()));
        assert!(commit_is_enabled(false, &pass));
    }

    #[test]
    fn commit_enabled_after_an_auth_required_probe_result() {
        let pass = Some(passing_probe(McpServerStatus::AuthRequired, Vec::new()));
        assert!(commit_is_enabled(false, &pass));
    }

    // --- parse_kv_lines / kv_pairs_to_text ----------------------------------

    #[test]
    fn parse_kv_lines_drops_blank_and_keyless_lines() {
        let parsed = parse_kv_lines("A=1\n\n  \nB=2\n=novalue\nC=");
        assert_eq!(
            parsed,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string()),
                ("C".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn kv_pairs_round_trip_through_text() {
        let pairs = vec![("X".to_string(), "1".to_string()), ("Y".to_string(), "2".to_string())];
        let text = kv_pairs_to_text(&pairs);
        assert_eq!(parse_kv_lines(&text), pairs);
    }

    // --- build_manual_draft / draft_ready_for_verify ------------------------

    #[test]
    fn stdio_draft_requires_command_not_url() {
        let draft = build_manual_draft(
            "srv", &McpTransportKind::Stdio, "npx", "-y pkg", "K=V", "https://ignored",
            "H=1", "prov", "issuer", 120, 60, true,
        );
        assert_eq!(draft.command.as_deref(), Some("npx"));
        assert!(draft.url.is_none(), "stdio draft must never carry a url");
        assert!(draft_ready_for_verify(&draft));
    }

    #[test]
    fn http_draft_requires_url_not_command() {
        let draft = build_manual_draft(
            "srv", &McpTransportKind::Http, "ignored", "ignored", "ignored",
            "https://example.test/mcp", "H=1", "prov", "issuer", 120, 60, true,
        );
        assert!(draft.command.is_none(), "http draft must never carry a command");
        assert_eq!(draft.url.as_deref(), Some("https://example.test/mcp"));
        assert!(draft_ready_for_verify(&draft));
    }

    #[test]
    fn draft_missing_required_field_is_not_ready() {
        let draft = build_manual_draft(
            "srv", &McpTransportKind::Stdio, "", "", "", "", "", "", "", 120, 60, true,
        );
        assert!(!draft_ready_for_verify(&draft));
    }

    #[test]
    fn draft_missing_name_is_not_ready() {
        let draft = build_manual_draft(
            "", &McpTransportKind::Stdio, "npx", "", "", "", "", "", "", 120, 60, true,
        );
        assert!(!draft_ready_for_verify(&draft));
    }

    // --- CR-01: name_is_sanitized / draft_ready_for_verify name gate -------

    #[test]
    fn name_is_sanitized_accepts_only_alnum_and_underscore() {
        assert!(name_is_sanitized("my_server"));
        assert!(name_is_sanitized("MyServer123"));
        assert!(!name_is_sanitized(""), "empty name must not round-trip");
        assert!(!name_is_sanitized("my-server"), "hyphen is outside [A-Za-z0-9_]");
        assert!(!name_is_sanitized("my server"), "space is outside [A-Za-z0-9_]");
        assert!(!name_is_sanitized("my.server"), "dot is outside [A-Za-z0-9_]");
        assert!(!name_is_sanitized("my/server"), "slash is outside [A-Za-z0-9_]");
    }

    #[test]
    fn draft_with_unsanitized_name_is_not_ready_even_with_required_field_present() {
        let draft = build_manual_draft(
            "my server", &McpTransportKind::Stdio, "npx", "", "", "", "", "", "", 120, 60, true,
        );
        assert!(
            !draft_ready_for_verify(&draft),
            "a name containing characters outside [A-Za-z0-9_] must block NEXT, \
             mirroring commit_mcp_server's server-side CR-01 gate"
        );
    }
}
