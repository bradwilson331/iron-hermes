//! `ironhermes doctor` — configuration and dependency health check.
//!
//! Extracted from main.rs `cmd_doctor` (Phase 35.1 D-03).
//!
//! Called from `setup::run_setup` at wizard exit and from `main` dispatch.

use anyhow::Result;
use colored::Colorize;
use ironhermes_core::config::{
    Config, WEB_ANSWER_PROVIDERS, WEB_EXTRACT_PROVIDERS, WEB_SEARCH_PROVIDERS,
};
use ironhermes_core::provider::{
    canonical_api_key_env_name, provider_key_guard_allows, ProviderResolver,
};
use ironhermes_tools::credentials::ToolCredentials;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Run the IronHermes doctor preflight check.
///
/// Prints a formatted health summary covering home directory, config file,
/// .env file, API keys, state database, and gateway PID liveness. Always
/// returns `Ok(())` — issues are reported as `MISSING` lines, not errors.
///
/// Phase 41.3 Plan 09 (D-19): `async` because reporting web-tool provider
/// coverage requires resolving the same env -> config -> vault credential
/// snapshot the runtime resolves at startup (`ToolCredentials::resolve` is
/// itself `async` — the vault's `SecretStore::get_secret` is async). This
/// function's only call site is `main.rs`'s `Doctor` dispatch —
/// `setup.rs::run_setup`'s own wizard-exit check (D-03) is a separate,
/// lib-safe reimplementation of the same name in that file's own module
/// scope (see its doc comment), not a call into this function.
///
/// Phase 53 Plan 03 (D-06): `browser` gates an OPT-IN browser backend
/// diagnosis (`ironhermes doctor --browser`) — resolving the configured
/// binary and running the render probe standalone. `false` (the default,
/// and what plain `ironhermes doctor` always passes) performs no browser
/// work at all: no binary resolution for the purpose of probing, no session
/// spawn, no CDP round trip.
pub async fn run_doctor_check(browser: bool) -> Result<()> {
    println!("{}", "IronHermes Doctor".bold().cyan());
    // Phase 24 D-16: show which profile this doctor run is inspecting.
    println!("Profile: {}", ironhermes_cli::status_cmd::current_profile());
    println!("{}", "─".repeat(40));

    // Check home directory
    let home = ironhermes_core::get_hermes_home();
    print_check("Home directory", home.exists());

    // Check config
    let config_path = Config::config_path();
    print_check("Config file", config_path.exists());

    // Check .env
    let env_path = Config::env_path();
    print_check(".env file", env_path.exists());

    // API keys are reported per-provider by the coverage section below, which
    // resolves through `ProviderResolver` (env -> config -> vault). Two
    // hardcoded `std::env::var` checks for OPENROUTER/ANTHROPIC used to sit
    // here; they were removed because they covered only 2 of the configured
    // providers AND could contradict the section beneath them — a key supplied
    // via config.yaml or the vault reads as MISSING to `std::env::var` while
    // the resolver, and the running agent, both see it.

    // Phase 41.3 Plan 09 (D-08/D-19): web-tool provider coverage — resolved
    // from the SAME env -> config -> vault snapshot the runtime resolves at
    // startup (never a direct `std::env::var` read here), so this section
    // cannot disagree with what the model actually sees. A sealed/unreachable
    // vault is reported as a failed check, never a panic or a propagated
    // `Err` — `doctor` is the one place an operator can diagnose that
    // condition without booting the agent (D-19).
    let doctor_config = Config::load().unwrap_or_default();
    let (tool_credentials, vault_failed_checks) = resolve_doctor_credentials(&doctor_config).await;
    for msg in &vault_failed_checks {
        print_check(msg, false);
    }
    // Phase 53 follow-up (venice-401 debug): per-provider API-key coverage for
    // EVERY configured provider, not just the two canonical built-ins checked
    // above. A `venice.ai` misconfiguration was invisible to `doctor` precisely
    // because nothing here looked past OPENROUTER/ANTHROPIC — and venice.ai was
    // the ACTIVE provider.
    let (provider_resolver, resolver_failed_checks) =
        resolve_doctor_provider_resolver(&doctor_config).await;
    for msg in &resolver_failed_checks {
        print_check(msg, false);
    }
    if let Some(resolver) = provider_resolver.as_ref() {
        print_provider_key_coverage(&doctor_config, resolver);
    }

    let tool_credentials = std::sync::Arc::new(tool_credentials);
    // `register_defaults()` builds `WebSearchTool`/`WebAnswerTool` from
    // whatever snapshot is installed on the registry at call time —
    // `with_credentials` MUST run before `register_defaults()`, or the
    // constructed tools would carry the placeholder env-only default instead.
    let mut provider_registry = ironhermes_tools::ToolRegistry::new();
    provider_registry.with_credentials(tool_credentials.clone());
    provider_registry.register_defaults();
    println!();
    println!("{}", "Web-tool provider coverage:".bold());
    for line in render_tool_provider_status(&provider_registry, &tool_credentials) {
        println!("  {line}");
    }

    // Phase 41.3 Plan 09 (D-08): a misconfigured chain (unknown provider name,
    // empty chain) is a doctor-reported configuration error, so a typo is
    // caught before a turn silently falls through the whole chain.
    for msg in render_chain_validation(&doctor_config) {
        print_check(&msg, false);
    }

    // Phase 46.8 Plan 05 (D-03): vault subsystem status block.
    let vault_config = &doctor_config;
    print_vault_status(vault_config);

    // Phase 46.5 D-07: vision-capable model check (kanban image tasks).
    // Doctor-only visibility per research approach (a) — `model.vision_model`
    // stays unwired dead code; this reports on the effective MAIN model, which
    // is already vision-capable by default (anthropic/claude-sonnet-4).
    let vision_model_ok = Config::load()
        .map(|c| is_vision_capable_model(&c.model.default))
        .unwrap_or(false);
    print_check("Vision-capable model (kanban image tasks)", vision_model_ok);

    // Check state database
    let db_path = home.join("state.db");
    print_check("State database", db_path.exists());

    // Phase 24 D-16: gateway.pid liveness check (active profile only — no
    // cross-profile sweep per the deferred-ideas list).
    let pid_path = home.join("gateway.pid");
    if pid_path.exists() {
        let pid_ok = ironhermes_gateway::pid::read_gateway_pid(&home)
            .ok()
            .flatten()
            .map(|r| {
                matches!(
                    ironhermes_gateway::pid::is_pid_alive(r.pid),
                    ironhermes_gateway::pid::PidLiveness::Live
                        | ironhermes_gateway::pid::PidLiveness::LiveOtherUser
                )
            })
            .unwrap_or(false);
        print_check("Gateway PID (gateway.pid → live process)", pid_ok);
    } else {
        // Absent file = healthy (no gateway running). Use the "OK" branch.
        print_check("Gateway PID (not running)", true);
    }

    // 48.3-05 D-06: "is it running" and "where is its log" are the same
    // question for an operator — report the gateway log path and freshness
    // immediately after the PID liveness check above.
    print_gateway_log_status(&home);

    // Phase 54.8 Plan 10 (D-21): what the Telegram site-deploy flow is missing.
    // Doctor-only surface; reuses the config loaded above.
    print_site_preflight(&doctor_config, &home).await;

    // WR-03 (phase 47.4): flag profiles that may have been exfiltrated during the
    // CR-03 window, before plan 20 closed the `${...}` substitution path. A
    // pre-patch exploit persisted root's plaintext credential into the victim
    // profile's `.env`, and nothing else surfaces that. Detection only — a
    // disclosed credential is remediated by ROTATION, never by editing the file.
    print_profile_exposure_audit(&home);

    // Phase 57 Plan 15 (D-12): is the detection sidecar installed, pinned and
    // healthy, and what does its absence mean at the effective DEFCON level.
    print_security_sidecar_section(&doctor_config).await;

    // Phase 53 Plan 03 (D-06): opt-in only. The whole block is behind the
    // flag — plain `ironhermes doctor` never resolves a browser binary for
    // the purpose of probing, never spawns a session, never makes a CDP
    // round trip.
    if browser {
        print_browser_diagnosis(&doctor_config).await;
    }

    println!();
    println!("{}", "Run `ironhermes status` for more details.".dimmed());

    Ok(())
}

/// Phase 53 Plan 03 (Task 3, D-06): `ironhermes doctor --browser` — reports
/// the configured backend, the resolved binary (if any), and the render
/// probe's outcome. Goes through
/// `ironhermes_tools::browser_session::diagnose`, which itself goes through
/// `BrowserSession::spawn` — the SAME path the runtime uses, so there is no
/// second copy of the probe logic here. Never returns an error — every
/// outcome is a printed line, preserving `run_doctor_check`'s always-
/// `Ok(())` invariant. Rendering is factored into [`render_browser_diagnosis`]
/// (pure, no I/O) so the output content is unit-testable without a live CDP
/// connection, following this file's `render_tool_provider_status` /
/// `render_chain_validation` precedent.
async fn print_browser_diagnosis(config: &Config) {
    println!();
    println!("{}", "Browser backend (--browser):".bold());

    let diagnosis = ironhermes_tools::browser_session::diagnose(&config.browser).await;
    for line in render_browser_diagnosis(&diagnosis) {
        println!("{line}");
    }
}

/// Pure rendering half of [`print_browser_diagnosis`]. See its doc for why
/// this is split out.
fn render_browser_diagnosis(
    diagnosis: &ironhermes_tools::browser_session::BrowserDiagnosis,
) -> Vec<String> {
    use ironhermes_tools::browser_session::BrowserDiagnosisOutcome;

    let mut lines = vec![format!("  Backend: {}", diagnosis.mode)];
    lines.push(format!(
        "  [{}] Binary resolved",
        if diagnosis.binary_path.is_some() {
            "OK".green().to_string()
        } else {
            "MISSING".yellow().to_string()
        }
    ));
    if let Some(path) = &diagnosis.binary_path {
        lines.push(format!("    {}", path.display()));
    }

    match &diagnosis.outcome {
        BrowserDiagnosisOutcome::Ok => {
            lines.push(format!(
                "  [{}] Render support (screenshot + computed-style checks)",
                "OK".green()
            ));
        }
        BrowserDiagnosisOutcome::RenderUnsupported {
            screenshot_ok,
            hidden_reports_none,
        } => {
            // NOT the "MISSING" shape above (read as "could not run") — a
            // render refusal IS the finding, not an absent check. Same
            // idiom as print_profile_exposure_audit's "EXPOSED" line.
            lines.push(format!(
                "  [{}] Render support (screenshot_ok={screenshot_ok}, \
                 hidden_reports_none={hidden_reports_none})",
                "REFUSED".red().bold()
            ));
            lines.push(
                "    This binary appears to have been built without `--features render` \
                 (release archives publish this as the `-no-render` suffix) — layout is \
                 absent, so every browser_* tool would return plausible but wrong answers \
                 against it. Rebuild with `--features render` or point \
                 browser.obscura_path/OBSCURA_PATH at a render-capable build."
                    .to_string(),
            );
        }
        BrowserDiagnosisOutcome::Failed(message) => {
            lines.push(format!(
                "  [{}] Browser session start ({message})",
                "MISSING".yellow()
            ));
        }
    }
    lines
}

/// Report CR-03-window credential exposure across all profiles (WR-03).
///
/// Prints key NAMES only — never a value. See `profile_audit` for the detection
/// rules and for why same-name inheritance is deliberately not flagged.
fn print_profile_exposure_audit(home: &std::path::Path) {
    let profiles_root = home.join(ironhermes_core::PROFILES_SUBDIR);
    let findings =
        ironhermes_cli::profile_audit::audit_profiles_at(&profiles_root, &home.join(".env"));

    if findings.is_empty() {
        print_check("Profile credential exposure (CR-03 window)", true);
        return;
    }

    // NOT print_check's `false` branch: that renders "MISSING", which an operator
    // reads as "this check could not run" — the opposite of what a hit means here.
    println!(
        "  [{}] Profile credential exposure (CR-03 window)",
        "EXPOSED".red().bold()
    );
    for line in ironhermes_cli::profile_audit::render_findings(&findings) {
        if line.is_empty() {
            println!();
        } else if line.starts_with("    ") {
            println!("  {}", line.yellow());
        } else if line.starts_with("ROTATE") {
            println!("  {}", line.red().bold());
        } else {
            println!("  {line}");
        }
    }
}

fn print_check(name: &str, ok: bool) {
    let icon = if ok { "OK".green() } else { "MISSING".yellow() };
    println!("  [{icon}] {name}");
}

// -----------------------------------------------------------------------
// Phase 57 Plan 15 (D-12): security sidecar report.
//
// With the default level 3 (D-21) a missing sidecar blocks Warn-class tools
// (D-12), so "is it installed, pinned and answering" is the first thing an
// operator needs to see, together with what its absence means at their level.
// -----------------------------------------------------------------------

/// The tirith-core version the sidecar is pinned to (`ironhermes-tirith`'s
/// `tirith-core = "=0.4.2"`).
const PINNED_TIRITH_CORE: &str = "0.4.2";

/// Pure formatting half of [`security_sidecar_report`] (no I/O), so every case
/// is unit-testable. `version` is the result of a `version` round trip and
/// `ping_ms` the result of a `ping` round trip; the sidecar is only ever
/// reported healthy when both succeeded (T-57-61).
fn format_sidecar_report(
    found: Option<PathBuf>,
    version: Result<ironhermes_security::SidecarVersion, String>,
    ping_ms: Option<u128>,
    level: ironhermes_core::config::DefconLevel,
    main_version: &str,
) -> Vec<(String, bool)> {
    let mut report: Vec<(String, bool)> = Vec::new();
    let level_line = format!(
        "DEFCON {} (yolo bypass {})",
        level.level(),
        if level.yolo_bypass_active() {
            "active"
        } else {
            "inactive"
        }
    );

    // (a) absent: nothing was found to ask, so there is no version or health line.
    let Some(path) = found else {
        report.push((
            format!(
                "sidecar absent: {} not found",
                ironhermes_security::SIDECAR_BIN_NAME
            ),
            false,
        ));
        report.push((level_line, true));
        report.push((sidecar_consequence_line(level, "installed"), false));
        return report;
    };

    report.push((format!("sidecar present: {}", path.display()), true));

    // (b) present but silent: version failed, so there is no health to report.
    let version = match version {
        Ok(version) => version,
        Err(reason) => {
            report.push((format!("version: unavailable ({reason})"), false));
            report.push((format!("health: unavailable ({reason})"), false));
            report.push((level_line, true));
            report.push((sidecar_consequence_line(level, "healthy"), false));
            return report;
        }
    };

    // (c) answered: build match, pin, protocol, threat DB, health.
    let build_matches = version.sidecar == main_version;
    report.push((
        if build_matches {
            format!(
                "sidecar build: ironhermes-tirith {} matches ironhermes {main_version}",
                version.sidecar
            )
        } else {
            format!(
                "sidecar build: ironhermes-tirith {} does not match ironhermes {main_version} \
                 (reinstall both from the same build)",
                version.sidecar
            )
        },
        build_matches,
    ));
    let pinned = version.tirith_core == PINNED_TIRITH_CORE;
    report.push((
        if pinned {
            format!("tirith-core {PINNED_TIRITH_CORE} (pinned)")
        } else {
            format!(
                "tirith-core {} is not the pinned {PINNED_TIRITH_CORE}",
                version.tirith_core
            )
        },
        pinned,
    ));
    report.push((format!("protocol {}", version.protocol), true));
    report.push((format!("threat DB: {}", version.threat_db), true));
    let healthy = ping_ms.is_some();
    report.push((
        match ping_ms {
            Some(ms) => format!("health: ok ({ms} ms)"),
            None => "health: ping failed (no answer to a ping round trip)".to_string(),
        },
        healthy,
    ));
    report.push((level_line, true));
    if !healthy {
        report.push((sidecar_consequence_line(level, "healthy"), false));
    }
    report
}

/// What an absent or unhealthy sidecar means at `level` (D-12): levels 1-3
/// block Warn-class tools until it is back, levels 4-5 raise them to Warn with
/// a critical notice (and yolo bypass proceeds on that Warn).
fn sidecar_consequence_line(level: ironhermes_core::config::DefconLevel, until: &str) -> String {
    let n = level.level();
    if level.at_least_as_strict_as(3) {
        format!(
            "DEFCON {n}: terminal/execute_code will be blocked until the sidecar is {until} \
             (every Warn-class tool is blocked at levels 1-3 while it is unavailable)"
        )
    } else {
        format!(
            "DEFCON {n}: terminal/execute_code are raised to Warn with a critical notice while \
             the sidecar is unavailable (yolo bypass proceeds on the Warn; the notice is still \
             logged)"
        )
    }
}

/// Where the sidecar lookup tried, in lookup order, for the absent report.
fn describe_sidecar_lookup(
    current_exe: Option<&Path>,
    env_override: Option<&std::ffi::OsStr>,
) -> String {
    let name = ironhermes_security::SIDECAR_BIN_NAME;
    let mut sibling = current_exe
        .and_then(Path::parent)
        .map(|dir| dir.join(name).display().to_string())
        .unwrap_or_else(|| "(running executable unknown)".to_string());
    // The lookup also tries next to the executable's real file (symlinks resolved).
    if let Some(exe) = current_exe
        && let Ok(real) = std::fs::canonicalize(exe)
        && real != exe
        && let Some(dir) = real.parent()
    {
        sibling = format!("{sibling}, then {}", dir.join(name).display());
    }
    let over = match env_override.filter(|v| !v.is_empty()) {
        Some(v) => format!(
            "{}={}",
            ironhermes_security::SIDECAR_ENV_OVERRIDE,
            v.to_string_lossy()
        ),
        None => format!("{} unset", ironhermes_security::SIDECAR_ENV_OVERRIDE),
    };
    format!("next to ironhermes: {sibling}; {over}; PATH")
}

/// Locate the sidecar, spawn a short-lived client, ask it for its version and a
/// ping, and format the answer. Never `boot()`s: doctor must not install a
/// process-global service, and dropping the client kills and reaps the child.
async fn security_sidecar_report(config: &Config) -> Vec<(String, bool)> {
    let level = config.effective_defcon_level();
    let main_version = env!("CARGO_PKG_VERSION");

    let Some(path) = ironhermes_security::locate_sidecar_binary() else {
        let mut report = format_sidecar_report(
            None,
            Err("not found".to_string()),
            None,
            level,
            main_version,
        );
        let looked = describe_sidecar_lookup(
            std::env::current_exe().ok().as_deref(),
            std::env::var_os(ironhermes_security::SIDECAR_ENV_OVERRIDE).as_deref(),
        );
        if let Some(first) = report.first_mut() {
            first.0.push_str(&format!(" (looked at: {looked})"));
        }
        return report;
    };

    let mut opts =
        ironhermes_security::SidecarOptions::for_home(&ironhermes_core::get_hermes_home());
    opts.binary = Some(path.clone());
    let client = ironhermes_security::SidecarClient::spawn(opts);
    let version = client.version().await.map_err(|e| e.to_string());
    let ping_ms = if version.is_ok() {
        client.ping().await.ok().map(|d| d.as_millis())
    } else {
        None
    };
    drop(client);
    format_sidecar_report(Some(path), version, ping_ms, level, main_version)
}

async fn print_security_sidecar_section(config: &Config) {
    println!();
    println!("{}", "Security sidecar".bold());
    for (label, ok) in security_sidecar_report(config).await {
        print_check(&label, ok);
    }
}

// -----------------------------------------------------------------------
// 48.3-05 D-06: gateway log status. Three supervision mechanisms
// (systemd, launchd, the cron watchdog) converge on ONE canonical gateway
// log file — see `scripts/deploy/ironhermes-gateway.service`,
// `scripts/deploy/com.ironhermes.gateway.plist`, and
// `scripts/deploy/gateway-watchdog.sh`.
//
// 54.8-04 D-08: a gateway started WITHOUT a supervisor now writes its own
// daily file `logs/gateway.log.YYYY-MM-DD` (owner-only). The process writes
// the dated file when unsupervised; the supervisors write the canonical file;
// doctor reports whichever is current (`current_gateway_log`). This section
// names that path out loud so an operator never has to guess which file to
// grep.
// -----------------------------------------------------------------------

/// How long since the log's mtime before doctor reports it as stale. A
/// stale log next to a live gateway PID is the exact false-negative shape
/// that cost the 2026-08-27 investigation (48.3 D-06).
const GATEWAY_LOG_FRESH_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The single resolver for the CURRENT gateway log.
///
/// Phase 54.8 Plan 04 (D-08): there are now two writers. The supervisors
/// (`scripts/deploy/ironhermes-gateway.service`, `scripts/deploy/com.ironhermes.gateway.plist`,
/// `scripts/deploy/gateway-watchdog.sh`, `docker/web-entrypoint.sh`) redirect
/// stdout to the canonical `logs/gateway.log`; an unsupervised
/// `ironhermes gateway` writes `logs/gateway.log.YYYY-MM-DD` itself. Doctor
/// reports whichever is most recently modified, so it finds the live log on
/// supervised and unsupervised installs alike. Only suffixes that parse as a
/// date count (`gateway.log.pre-48.3` is ignored). Falls back to the canonical
/// path when nothing exists, which is the path a first supervised start creates.
fn current_gateway_log(home: &Path) -> PathBuf {
    let canonical = ironhermes_cli::gateway_log::canonical_gateway_log(home);
    let mut best: Option<(SystemTime, PathBuf)> = std::fs::metadata(&canonical)
        .ok()
        .filter(|m| m.is_file())
        .and_then(|m| m.modified().ok())
        .map(|t| (t, canonical.clone()));
    if let Ok(entries) = std::fs::read_dir(home.join("logs")) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().to_string();
            if ironhermes_cli::gateway_log::parse_dated_log_name(&name).is_none() {
                continue;
            }
            let Ok(meta) = std::fs::metadata(entry.path()) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if best.as_ref().is_none_or(|(t, _)| modified > *t) {
                best = Some((modified, entry.path()));
            }
        }
    }
    best.map(|(_, p)| p).unwrap_or(canonical)
}

/// Render the gateway log status as lines, without printing.
///
/// `now` is a parameter rather than read from the clock so the stale/fresh
/// boundary is testable without sleeping.
fn gateway_log_lines(home: &Path, now: SystemTime) -> Vec<String> {
    let path = current_gateway_log(home);
    let path_str = path.display().to_string();
    let mut lines = vec![format!("Gateway log: {path_str}")];

    match std::fs::metadata(&path) {
        Err(_) => {
            lines.push(
                "  does not exist yet — created by scripts/deploy/install.sh (or the cron watchdog) on first gateway start"
                    .to_string(),
            );
        }
        Ok(meta) => match meta.modified() {
            Ok(modified) => {
                let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
                if age > GATEWAY_LOG_FRESH_WINDOW {
                    let age_hours = age.as_secs() / 3600;
                    let age_days = age_hours / 24;
                    let age_desc = if age_days >= 1 {
                        format!("{age_days}d old")
                    } else {
                        format!("{age_hours}h old")
                    };
                    lines.push(format!(
                        "  stale — last written {age_desc} (older than the {}-hour freshness window)",
                        GATEWAY_LOG_FRESH_WINDOW.as_secs() / 3600
                    ));
                } else {
                    lines.push("  current — written within the last 24 hours".to_string());
                }
            }
            Err(_) => {
                lines.push("  exists, but its modification time could not be read".to_string());
            }
        },
    }

    lines
}

/// Print the gateway log status.
///
/// For the existence line, uses `print_check` (missing = healthy: a
/// gateway that never ran has no log). For the stale case, follows
/// `print_profile_exposure_audit`'s precedent and does NOT use
/// `print_check`'s `false` branch — its `MISSING` rendering reads as "this
/// check could not run" rather than "this is what I found."
fn print_gateway_log_status(home: &Path) {
    let path = current_gateway_log(home);
    let lines = gateway_log_lines(home, SystemTime::now());
    let label = format!("Gateway log ({})", path.display());

    let is_stale = lines.iter().any(|l| l.to_lowercase().contains("stale"));
    if is_stale {
        println!("  [{}] {label}", "STALE".yellow().bold());
    } else {
        // Missing-file and current-file are both healthy/informational, not
        // a failure: a gateway that has never run simply has no log yet.
        print_check(&label, true);
    }
    for line in lines.into_iter().skip(1) {
        println!("  {}", line.dimmed());
    }
}

// -----------------------------------------------------------------------
// Phase 41.3 Plan 09 (D-08/D-19): web-tool provider coverage + chain
// validation. Pure render functions return lines rather than printing
// directly, so tests can assert on rendered content without capturing
// stdout — `run_doctor_check` above is the only `println!` call site.
// -----------------------------------------------------------------------

/// D-19: resolve the exact env -> config -> vault credential snapshot the
/// runtime resolves at `build_app_runtime_bundle` — only opening the vault
/// store when the operator enabled it (T-41.3-53: an operator who never
/// turned the vault on can never be stopped by it, and `doctor` must not
/// diverge from that on the open side either). Unlike startup, a resolution
/// failure here is never allowed to escape as an `Err` (D-19: doctor must
/// survive the exact condition it exists to diagnose) — it is instead
/// rendered as one failed-check line naming the configured backend, and the
/// returned snapshot degrades to env/config-tier-only so the rest of this
/// section still reports something real rather than nothing.
async fn resolve_doctor_credentials(config: &Config) -> (ToolCredentials, Vec<String>) {
    if !config.vault.enabled {
        // No store to open — `resolve(config, None)` never reaches the vault
        // arm, so this branch cannot itself produce a sealed-vault failure.
        let creds = ToolCredentials::resolve(config, None)
            .await
            .unwrap_or_default();
        return (creds, Vec::new());
    }

    match ironhermes_vault::open_store(&crate::vault_cmd::resolve_vault_config(config)) {
        Ok(store) => resolve_doctor_credentials_with_store(config, Some(store.as_ref())).await,
        Err(_e) => {
            // `open_store` itself failed (unknown backend, feature not
            // compiled in, etc.) — never format `_e` verbatim here; the
            // backend name alone identifies the problem without risking any
            // wrapped context string that a future error variant might add.
            let creds = ToolCredentials::resolve(config, None)
                .await
                .unwrap_or_default();
            (
                creds,
                vec![format!(
                    "Vault ({}): failed to open the configured backend — tool credentials \
                     resolved from env/config tiers only",
                    config.vault.backend
                )],
            )
        }
    }
}

/// D-19: the store-taking half of [`resolve_doctor_credentials`], split out
/// so tests can drive it with a fake `SecretStore` (sealed, corrupt, or
/// healthy) without going through real backend selection. Mirrors
/// `build_app_runtime_bundle`'s hard-error posture at the boundary — but
/// converts the `Err` into one failed-check line instead of propagating it,
/// per this file's own doc comment: doctor never errors.
async fn resolve_doctor_credentials_with_store(
    config: &Config,
    store: Option<&dyn ironhermes_vault::SecretStore>,
) -> (ToolCredentials, Vec<String>) {
    match ToolCredentials::resolve(config, store).await {
        Ok(creds) => (creds, Vec::new()),
        Err(_e) => {
            // T-41.3-62: never format `_e` — `sealed_vault_error_carries_no_secret_material`
            // (Plan 10) already proves the underlying error string is safe, but this
            // file's own discipline is to name only the backend, never any wrapped
            // context. Degrade to an env/config-tier-only snapshot (no store) so the
            // rest of this section still reports something real.
            let creds = ToolCredentials::resolve(config, None)
                .await
                .unwrap_or_default();
            (
                creds,
                vec![format!(
                    "Vault ({}): tool-credential resolution failed (sealed, locked, or \
                     unreachable) — reporting env/config tiers only",
                    config.vault.backend
                )],
            )
        }
    }
}

/// D-08: `tool_name` -> its legal provider chain in `ironhermes_core::config`
/// — the SAME const arrays `ToolsConfig::validate_chains()` reads. Doctor
/// reads the array's length only (never restates its contents) to catch a
/// future drift class the group-count rendering alone cannot: an operator
/// adding a 5th legal provider to one of these arrays without also adding a
/// matching `Prerequisite::grouped_env_var` would otherwise silently make
/// that provider unconfigurable through the wizard/doctor surface. Returns
/// `None` for a tool name this phase does not know about (no cross-check).
fn legal_provider_chain_len(tool_name: &str) -> Option<usize> {
    match tool_name {
        "web_search" => Some(WEB_SEARCH_PROVIDERS.len()),
        "web_answer" => Some(WEB_ANSWER_PROVIDERS.len()),
        "web_extract" => Some(WEB_EXTRACT_PROVIDERS.len()),
        _ => None,
    }
}

/// D-08/D-19: one rendered line per provider group across every registered
/// tool — `<tool>: <satisfied>/<total> providers configured`, using
/// `group_members`/`satisfied_group_members_with_credentials` (Plan 06/09) so
/// this count agrees with what `is_available()` would answer for the same
/// credential state by construction, not by coincidence. Never formats a
/// credential value — only env-var **names** and the **tier** that satisfied
/// each (`source_of`), matching this file's existing `Vault sealed-state`
/// idiom of reporting shape/provenance, never content (T-41.3-44).
fn render_tool_provider_status(
    registry: &ironhermes_tools::ToolRegistry,
    credentials: &ToolCredentials,
) -> Vec<String> {
    use ironhermes_tools::registry::{group_members, satisfied_group_members_with_credentials};

    let mut lines = Vec::new();
    for name in registry.list_tools() {
        let Some(tool) = registry.get(name) else {
            continue;
        };
        let prereqs = tool.prerequisites();
        let mut groups: Vec<&str> = prereqs.iter().filter_map(|p| p.group.as_deref()).collect();
        groups.sort_unstable();
        groups.dedup();

        for group in groups {
            let members = group_members(&prereqs, group);
            let satisfied =
                satisfied_group_members_with_credentials(&prereqs, group, Some(credentials));

            let mut configured = Vec::new();
            let mut missing = Vec::new();
            for m in &members {
                if credentials.has_credential(&m.name) {
                    let tier = credentials
                        .source_of(&m.name)
                        .map(|s| format!("{s:?}"))
                        .unwrap_or_else(|| "unknown".to_string());
                    configured.push(format!("{} ({tier})", m.name));
                } else {
                    missing.push(m.name.as_str());
                }
            }

            let mut line = format!("{name}: {satisfied}/{} providers configured", members.len());
            if !configured.is_empty() {
                line.push_str(&format!(" — configured: {}", configured.join(", ")));
            }
            if !missing.is_empty() {
                line.push_str(&format!(" — set to add: {}", missing.join(", ")));
            }
            if satisfied == 0 && tool.is_available() {
                line.push_str(" (tool remains available via its keyless default)");
            }
            // D-08 drift check: a keyed group member count that falls short
            // of the tool's own legal provider chain length (config.rs's
            // WEB_*_PROVIDERS array, minus exactly one keyless terminal per
            // D-09/D-10) means a provider was added to the chain's legal
            // values without a matching wizard/doctor-visible prerequisite —
            // it would silently be unconfigurable through this surface.
            if let Some(legal_len) = legal_provider_chain_len(name)
                && members.len() + 1 < legal_len
            {
                line.push_str(&format!(
                    " [drift: only {} of {} legal chain providers have a prerequisite entry]",
                    members.len(),
                    legal_len
                ));
            }
            lines.push(line);
        }
    }
    lines
}

/// D-08: one failed-check line per `ToolsConfig::validate_chains()` problem —
/// each message already names both the offending provider value and the
/// dotted key path (`unknown provider '<value>' in tools.<tool>.chain`), so a
/// typo is findable without bisecting. A valid config renders nothing.
fn render_chain_validation(config: &Config) -> Vec<String> {
    match config.tools.validate_chains() {
        Ok(()) => Vec::new(),
        Err(problems) => problems,
    }
}

/// Phase 46.8 Plan 05 (D-03): report vault enabled / backend / data-dir /
/// sealed-state via `print_check` — informational only, never a secret value
/// (D-15). Always returns (no `Result`) — any store-open failure while
/// deriving `sealed-state` is caught and printed as a status string, never
/// propagated (doctor never errors, per this file's own doc comment).
fn print_vault_status(config: &Config) {
    let vault_cfg = &config.vault;

    print_check("Vault enabled", vault_cfg.enabled);
    print_check(&format!("Vault backend: {}", vault_cfg.backend), true);

    let data_dir = crate::vault_cmd::resolve_vault_config(config)
        .rusty_vault
        .data_dir;
    print_check(
        &format!("Vault data-dir present ({})", data_dir.display()),
        data_dir.exists(),
    );

    let sealed_state = vault_sealed_state(vault_cfg, &data_dir);
    let sealed_ok = sealed_state == "unsealed" || sealed_state == "n/a";
    print_check(&format!("Vault sealed-state: {sealed_state}"), sealed_ok);
}

/// Derive the rusty-vault backend's sealed-state by attempting
/// `RustyVaultStore::open` at the resolved data dir, then querying the
/// opened store's ACTUAL seal state via `is_sealed()` (WR-01, 46.8-gap) —
/// NOT by inferring "unsealed" from `open()` returning `Ok`. `open()`
/// returns `Ok` even in `unseal_mode = "passphrase"`, where it deliberately
/// leaves the underlying `Core` sealed until an operator runs
/// `ironhermes vault unlock`; treating that `Ok` as "unsealed" was WR-01's
/// false-positive bug. An `open()` `Err` whose message names
/// "sealed"/"not initialized" still maps to that state for backward
/// compatibility with pre-init/corrupt-keyfile cases; anything else is
/// reported verbatim as an opaque backend error string (never a secret
/// value; `VaultError`'s `Display` impl is static/code-shaped by
/// construction, D-08/D-15 safe to format).
#[cfg(feature = "rusty-vault")]
fn vault_sealed_state(
    vault_cfg: &ironhermes_vault::VaultConfig,
    data_dir: &std::path::Path,
) -> String {
    if vault_cfg.backend != "rusty-vault" {
        return "n/a".to_string();
    }
    let mut rusty_vault_cfg = vault_cfg.rusty_vault.clone();
    rusty_vault_cfg.data_dir = data_dir.to_path_buf();

    match ironhermes_vault::RustyVaultStore::open(&rusty_vault_cfg) {
        Ok(store) => match store.is_sealed() {
            Ok(true) => "sealed".to_string(),
            Ok(false) => "unsealed".to_string(),
            Err(e) => format!("error: {e}"),
        },
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not initialized") {
                "not initialized".to_string()
            } else if msg.contains("sealed") {
                "sealed".to_string()
            } else {
                format!("error: {msg}")
            }
        }
    }
}

#[cfg(not(feature = "rusty-vault"))]
fn vault_sealed_state(
    vault_cfg: &ironhermes_vault::VaultConfig,
    _data_dir: &std::path::Path,
) -> String {
    if vault_cfg.backend == "rusty-vault" {
        "rusty-vault feature not compiled in".to_string()
    } else {
        "n/a".to_string()
    }
}

/// Phase 46.5 D-07: is `model` (an OpenRouter-style `provider/model-id`
/// string, or a bare model id) on the known vision-capable allowlist?
///
/// Doctor-only surfacing (research approach (a)) — this does NOT wire
/// `model.vision_model` (verified dead code, RESEARCH.md Pitfall 1). It
/// reports on the effective MAIN model resolved from `config.model.default`.
///
/// Matched by case-insensitive substring against the current (2026-07)
/// OpenRouter vision-model ecosystem: Claude 3/4.x family, GPT-4o/4.1/5,
/// Gemini, Grok vision variants, GLM-4V/4.6V, Pixtral, Qwen-VL.
fn is_vision_capable_model(model: &str) -> bool {
    let m = model.to_lowercase();
    const VISION_ALLOWLIST: &[&str] = &[
        "claude-3",
        "claude-4",
        "claude-sonnet",
        "claude-opus",
        "claude-haiku",
        "gpt-4o",
        "gpt-4.1",
        "gpt-5",
        "gemini",
        "grok-2-vision",
        "grok-4",
        "grok-vision",
        "glm-4v",
        "glm-4.6v",
        "pixtral",
        "qwen-vl",
    ];
    VISION_ALLOWLIST.iter().any(|needle| m.contains(needle))
        || (m.contains("qwen") && m.contains("vl"))
}

// -----------------------------------------------------------------------
// Per-provider API-key coverage (venice-401 debug follow-up)
//
// `doctor` previously checked only OPENROUTER_API_KEY and ANTHROPIC_API_KEY
// via direct `std::env::var`. Every other provider — including a custom one
// selected as `model.provider` — was unaudited, so an unusable key on the
// ACTIVE provider produced a fully green `doctor` run.
// -----------------------------------------------------------------------

/// Build the resolver `doctor` reports provider coverage from, applying the
/// vault tier when one is configured.
///
/// Mirrors [`resolve_doctor_credentials`]'s discipline exactly: a sealed or
/// unopenable vault degrades to an env/config-only answer plus a failed check
/// line, never a panic or a propagated `Err`, and the error value is NEVER
/// formatted — only the backend name is named, so no wrapped context string
/// can leak secret material.
async fn resolve_doctor_provider_resolver(
    config: &Config,
) -> (Option<ProviderResolver>, Vec<String>) {
    let mut resolver = match ProviderResolver::build(config) {
        Ok(r) => r,
        Err(_e) => {
            // Never format `_e`: resolver build errors quote config values.
            return (
                None,
                vec![
                    "Provider resolver: could not be built from config — \
                     per-provider key coverage unavailable"
                        .to_string(),
                ],
            );
        }
    };

    if !config.vault.enabled {
        return (Some(resolver), Vec::new());
    }

    match ironhermes_vault::open_store(&crate::vault_cmd::resolve_vault_config(config)) {
        Ok(store) => {
            if resolver.apply_vault_fallback(store.as_ref()).await.is_err() {
                return (
                    Some(resolver),
                    vec![format!(
                        "Vault ({}): provider keys resolved from env/config tiers only",
                        config.vault.backend
                    )],
                );
            }
            (Some(resolver), Vec::new())
        }
        Err(_e) => (
            Some(resolver),
            vec![format!(
                "Vault ({}): failed to open the configured backend — provider keys \
                 resolved from env/config tiers only",
                config.vault.backend
            )],
        ),
    }
}

/// Print one line per configured provider saying whether its API key resolved.
///
/// Resolution comes from [`ProviderResolver`] — the same env -> config -> vault
/// snapshot the runtime resolves at startup — so this section cannot disagree
/// with what the model actually sees. That is the precedent the web-tool
/// coverage section sets, and the reason this does not read `std::env::var`.
///
/// Keylessness is judged by [`provider_key_guard_allows`], the runtime's OWN
/// guard, so a loopback endpoint (ollama / atomic / a local llama) reports
/// healthy rather than as a missing key — matching whether the agent will
/// actually refuse to boot on it. A whitespace-only key counts as absent,
/// which is what that guard already encodes.
/// Build the per-provider coverage lines as `(label, ok)` pairs.
///
/// Split out from [`print_provider_key_coverage`] so the decision logic is
/// unit-testable without capturing stdout — the same split this file already
/// uses for `gateway_log_lines` / `print_gateway_log_status`.
fn provider_key_coverage_lines(
    config: &Config,
    resolver: &ProviderResolver,
) -> Vec<(String, bool)> {
    let mut names = resolver.endpoint_names();
    names.sort();
    let main = resolver.main_provider().to_string();
    let mut out = Vec::new();

    for name in names {
        let Some(endpoint) = resolver.resolve(&name) else {
            continue;
        };
        // Mark the provider an ordinary chat turn will actually use, so a
        // broken ACTIVE provider cannot be mistaken for a dormant one.
        let active = if name == main { " [ACTIVE]" } else { "" };

        if provider_key_guard_allows(endpoint.api_key.as_deref(), &endpoint.base_url) {
            let how = if endpoint
                .api_key
                .as_deref()
                .is_some_and(|k| !k.trim().is_empty())
            {
                "key resolved".to_string()
            } else {
                // The guard passed with no key, which it only does for a
                // loopback host.
                format!("keyless, local endpoint {}", endpoint.base_url)
            };
            out.push((format!("provider {name}{active} — {how}"), true));
        } else {
            // Name the variable the operator should set. `api_key_env` wins;
            // otherwise the canonical built-in name; otherwise there is no
            // name to report and the operator must declare one.
            let hint = config
                .providers
                .get(&name)
                .and_then(|p| p.api_key_env.clone())
                .or_else(|| canonical_api_key_env_name(&name).map(str::to_string))
                .map(|v| format!("set {v}"))
                .unwrap_or_else(|| format!("set providers.{name}.api_key_env"));
            out.push((
                format!("provider {name}{active} — no usable key ({hint})"),
                false,
            ));
        }
    }
    out
}

/// Print one line per configured provider saying whether its API key resolved.
///
/// Resolution comes from [`ProviderResolver`] — the same env -> config -> vault
/// snapshot the runtime resolves at startup — so this section cannot disagree
/// with what the model actually sees. That is the precedent the web-tool
/// coverage section sets, and the reason this does not read `std::env::var`.
///
/// Keylessness is judged by [`provider_key_guard_allows`], the runtime's OWN
/// guard, so a loopback endpoint (ollama / atomic / a local llama) reports
/// healthy rather than as a missing key — matching whether the agent will
/// actually refuse to boot on it. A whitespace-only key counts as absent,
/// which is what that guard already encodes.
fn print_provider_key_coverage(config: &Config, resolver: &ProviderResolver) {
    let lines = provider_key_coverage_lines(config, resolver);
    if lines.is_empty() {
        return;
    }
    println!("Provider API-key coverage:");
    for (label, ok) in lines {
        print_check(&label, ok);
    }
}

// -----------------------------------------------------------------------
// Phase 54.8 Plan 10 (D-21, D-13, D-10): Telegram site-deploy preflight.
//
// `ironhermes doctor` names, by name only, everything the site flow is
// missing: tools on PATH, token variables (set / not set, never a value),
// the terminal env allowlist, skills, OAuth MCP auth, and per-site repo,
// clone and base-branch state. Rendering is a pure async function over an
// injectable probe so every rule is unit-testable; `RealProbe` runs each
// child under the SAME environment the bot's terminal tool builds, so the
// verdicts describe the credential the bot will actually use (RESEARCH F4).
// -----------------------------------------------------------------------

/// Deadline for every probe child (`node --version`, `gh api ...`).
const SITE_PROBE_DEADLINE: Duration = Duration::from_secs(10);

/// Token variables the site flow needs. Only presence is ever reported.
const SITE_TOKEN_VARS: [&str; 3] = [
    "CLOUDFLARE_API_TOKEN",
    "CLOUDFLARE_ACCOUNT_ID",
    "GITHUB_TOKEN",
];

/// A terminal-tool timeout cap below this makes a deploy impossible.
const MIN_TERMINAL_TIMEOUT_SECS: i64 = 300;

/// Verdict about the EFFECTIVE prohibition on pushing straight to the base
/// branch with the bot's own credential (D-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchProtection {
    /// A pull-request rule this credential cannot bypass.
    Effective,
    /// A pull-request rule exists but this credential can bypass it.
    Bypassable,
    /// No pull-request requirement applies.
    NotProtected,
    /// Not decidable with this credential. Never reported as protected.
    Unknown,
}

#[async_trait::async_trait]
pub(crate) trait SitePreflightProbe: Send + Sync {
    /// Is `cmd` an executable on the bot's terminal PATH?
    fn on_path(&self, cmd: &str) -> bool;
    /// Major version of `node`, when it can be read.
    async fn node_major(&self) -> Option<u32>;
    /// Is the named variable set (non-empty) in doctor's own environment?
    fn env_is_set(&self, name: &str) -> bool;
    /// `Some(visible)` when skill visibility could be determined.
    fn skill_visible(&self, name: &str) -> Option<bool>;
    /// `Some(present)` when the OAuth token store could be consulted.
    async fn mcp_token_present(&self, namespace: &str) -> Option<bool>;
    /// `Some(reachable)`; `None` when the probe itself could not run.
    async fn repo_reachable(&self, repo: &str) -> Option<bool>;
    /// Effective base-branch protection for the bot's credential.
    async fn branch_protection(&self, repo: &str, branch: &str) -> BranchProtection;
}

/// Render the preflight as lines (first line is the section header).
///
/// With no `sites:` configured this is the header plus one line and makes no
/// probe call at all. Never formats an environment value.
pub(crate) async fn site_preflight_lines(
    config: &Config,
    home: &Path,
    probe: &dyn SitePreflightProbe,
) -> Vec<String> {
    let mut lines = vec!["Site deploy preflight".to_string()];
    if config.sites.is_empty() {
        lines.push(
            "  no sites configured (sites: in config.yaml — see docs/SITE-DEPLOY-FROM-CHAT.md)"
                .to_string(),
        );
        return lines;
    }

    // Tools.
    for cmd in ["git", "gh", "npm"] {
        if probe.on_path(cmd) {
            lines.push(format!("  {cmd}: ok"));
        } else {
            lines.push(format!("  {cmd}: missing"));
        }
    }
    if probe.on_path("node") {
        match probe.node_major().await {
            Some(major) if major >= 22 => lines.push(format!("  node: v{major} ok")),
            Some(major) => lines.push(format!(
                "  node: v{major} — needs >= 22 for current wrangler"
            )),
            None => lines.push("  node: present (version unreadable)".to_string()),
        }
    } else {
        lines.push("  node: missing — needs >= 22 for current wrangler".to_string());
    }
    if probe.on_path("wrangler") {
        lines.push("  wrangler: ok".to_string());
    } else {
        lines.push(
            "  wrangler: missing — install the pinned version globally; the site-deploy skill runs the installed binary only"
                .to_string(),
        );
    }

    // Tokens: presence only, and whether the terminal will pass them on.
    let allowlist = &config.terminal.terminal_env_allowlist;
    for name in SITE_TOKEN_VARS {
        let state = if probe.env_is_set(name) {
            "set"
        } else {
            "not set"
        };
        lines.push(format!("  {name}: {state}"));
        if !allowlist.iter().any(|a| a == name) {
            lines.push(format!(
                "  {name}: not in terminal.terminal_env_allowlist — wrangler/gh will not see it"
            ));
        }
    }

    if let Some(secs) = config.tools.timeout_overrides.get("terminal")
        && *secs < MIN_TERMINAL_TIMEOUT_SECS
    {
        lines.push(format!(
            "  terminal timeout: tools.timeout_overrides.terminal = {secs} s caps every terminal call; a deploy needs up to {MIN_TERMINAL_TIMEOUT_SECS} s — raise it"
        ));
    }

    // Skills.
    for name in ["wrangler", "site-deploy"] {
        match probe.skill_visible(name) {
            Some(true) => lines.push(format!("  skill {name}: visible")),
            Some(false) => lines.push(format!(
                "  skill {name}: not visible — install it and make sure it has no unmet requires_mcp_servers"
            )),
            None => lines.push(format!("  skill {name}: unknown")),
        }
    }

    // OAuth MCP servers.
    let mut servers: Vec<&String> = config.mcp_servers.keys().collect();
    servers.sort();
    for server in servers {
        let Ok(cfg) = serde_yaml::from_value::<ironhermes_mcp::McpServerConfig>(
            config.mcp_servers[server].clone(),
        ) else {
            continue;
        };
        let Some(ns) = cfg.oauth_provider.as_deref() else {
            continue;
        };
        match probe.mcp_token_present(ns).await {
            Some(true) => lines.push(format!("  MCP {server}: authorized")),
            Some(false) => lines.push(format!(
                "  MCP {server}: not authorized — ironhermes mcp connect {server} --no-browser"
            )),
            None => lines.push(format!("  MCP {server}: unknown")),
        }
    }

    // Sites.
    let gh_available = probe.on_path("gh");
    for (name, parsed) in config.validated_sites() {
        let site = match parsed {
            Ok(site) => site,
            Err(e) => {
                lines.push(format!(
                    "  site {name}: INVALID — {}: {}",
                    e.field, e.reason
                ));
                continue;
            }
        };
        let clone_dir = match site.resolve_clone_dir(home) {
            Ok(dir) => dir,
            Err(e) => {
                lines.push(format!(
                    "  site {name}: INVALID — {}: {}",
                    e.field, e.reason
                ));
                continue;
            }
        };
        lines.push(format!("  site {name} ({})", site.repo));
        if clone_dir.exists() {
            lines.push("    clone: present".to_string());
        } else {
            lines.push("    clone: absent (cloned on first use)".to_string());
        }
        let branch = &site.base_branch;
        if !gh_available {
            lines.push("    repo: unknown (gh missing)".to_string());
            lines.push(format!(
                "    base branch {branch}: unknown — gh is not available to read protection"
            ));
            continue;
        }
        match probe.repo_reachable(&site.repo).await {
            Some(true) => lines.push("    repo: reachable".to_string()),
            Some(false) => lines.push("    repo: unreachable or no access".to_string()),
            None => lines.push("    repo: unknown".to_string()),
        }
        let verdict = probe.branch_protection(&site.repo, branch).await;
        lines.push(format!(
            "    base branch {branch}: {}",
            branch_protection_text(verdict)
        ));
    }
    lines
}

/// The `: <verdict>` text of a base-branch line. Plan 14 greps for
/// `: unknown` and `NOT protected`, so those literals are load-bearing.
fn branch_protection_text(verdict: BranchProtection) -> &'static str {
    match verdict {
        BranchProtection::Effective => {
            "protected (pull request required; this token cannot bypass it)"
        }
        BranchProtection::Bypassable => {
            "NOT protected — a pull-request rule exists but this token can bypass it (remove the bypass or use a token without it)"
        }
        BranchProtection::NotProtected => "NOT protected — enable a PR-required rule (D-10)",
        BranchProtection::Unknown => {
            "unknown — protection not readable with this token (use a branch ruleset, which it can read, or confirm in GitHub)"
        }
    }
}

/// Output of one probe child.
struct ChildOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Probe that runs real children under the bot's terminal environment.
struct RealProbe {
    deadline: Duration,
    /// Exactly what the terminal tool passes after `env_clear()`.
    env: std::collections::HashMap<String, String>,
    home: PathBuf,
    skills_config: ironhermes_core::config::SkillsConfig,
    skills: std::sync::OnceLock<ironhermes_core::SkillRegistry>,
}

impl RealProbe {
    fn from_config(config: &Config, home: &Path) -> Self {
        Self::with_env(
            SITE_PROBE_DEADLINE,
            ironhermes_core::build_terminal_safe_env(&config.terminal.terminal_env_allowlist, &[]),
            home,
            config.skills.clone(),
        )
    }

    fn with_env(
        deadline: Duration,
        env: std::collections::HashMap<String, String>,
        home: &Path,
        skills_config: ironhermes_core::config::SkillsConfig,
    ) -> Self {
        Self {
            deadline,
            env,
            home: home.to_path_buf(),
            skills_config,
            skills: std::sync::OnceLock::new(),
        }
    }

    /// Resolve `cmd` against the bot's terminal PATH (not doctor's own).
    fn resolve(&self, cmd: &str) -> Option<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let path = self.env.get("PATH")?;
        std::env::split_paths(path)
            .map(|dir| dir.join(cmd))
            .find(|candidate| {
                std::fs::metadata(candidate)
                    .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            })
    }

    /// Run one child asynchronously: env cleared then set to the terminal
    /// env, stdin closed, both pipes drained by `output()`, bounded by the
    /// deadline, and killed if the future is dropped. `None` means the child
    /// could not run or did not finish in time.
    async fn run(&self, cmd: &str, args: &[&str]) -> Option<ChildOut> {
        let program = self.resolve(cmd)?;
        let mut child = tokio::process::Command::new(program);
        child
            .args(args)
            .env_clear()
            .envs(&self.env)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let out = tokio::time::timeout(self.deadline, child.output())
            .await
            .ok()?
            .ok()?;
        Some(ChildOut {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[async_trait::async_trait]
impl SitePreflightProbe for RealProbe {
    fn on_path(&self, cmd: &str) -> bool {
        self.resolve(cmd).is_some()
    }

    async fn node_major(&self) -> Option<u32> {
        let out = self.run("node", &["--version"]).await?;
        if !out.ok {
            return None;
        }
        out.stdout
            .trim()
            .trim_start_matches('v')
            .split('.')
            .next()?
            .parse()
            .ok()
    }

    fn env_is_set(&self, name: &str) -> bool {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    }

    fn skill_visible(&self, name: &str) -> Option<bool> {
        let registry = self.skills.get_or_init(|| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            ironhermes_core::SkillRegistry::load_with_config(&cwd, &self.skills_config)
        });
        let Some(record) = registry.find(name) else {
            return Some(false);
        };
        if name == "wrangler" {
            let needs_mcp = record
                .hermes_metadata
                .as_ref()
                .is_some_and(|m| !m.requires_mcp_servers.is_empty());
            return Some(!needs_mcp);
        }
        Some(true)
    }

    async fn mcp_token_present(&self, namespace: &str) -> Option<bool> {
        let path = self.home.join("auth.json");
        if !path.exists() {
            return Some(false);
        }
        let store = ironhermes_core::AuthStore::open(path).await.ok()?;
        Some(store.get_token(namespace).await.is_some())
    }

    async fn repo_reachable(&self, repo: &str) -> Option<bool> {
        let out = self
            .run("gh", &["api", &format!("repos/{repo}"), "--jq", ".name"])
            .await?;
        Some(out.ok)
    }

    async fn branch_protection(&self, repo: &str, branch: &str) -> BranchProtection {
        // (1) Active ruleset rules for the branch, paginated. A failed call is
        // `None`: it cannot rule a ruleset out.
        let rules = match self
            .run(
                "gh",
                &[
                    "api",
                    "--paginate",
                    &format!("repos/{repo}/rules/branches/{branch}"),
                    "--jq",
                    ".[] | [.type, ((.ruleset_id // 0) | tostring)] | @tsv",
                ],
            )
            .await
        {
            Some(out) if out.ok => parse_rules_tsv(&out.stdout),
            _ => None,
        };

        // (2) The bot credential's own bypass state, per pull-request ruleset.
        let mut ruleset_bypass = std::collections::HashMap::new();
        if let Some(rules) = rules.as_ref() {
            for id in rules
                .iter()
                .filter(|(kind, _)| kind == "pull_request")
                .filter_map(|(_, id)| *id)
            {
                if ruleset_bypass.contains_key(&id) {
                    continue;
                }
                let bypass = match self
                    .run(
                        "gh",
                        &[
                            "api",
                            &format!("repos/{repo}/rulesets/{id}"),
                            "--jq",
                            ".current_user_can_bypass // empty",
                        ],
                    )
                    .await
                {
                    Some(out) if out.ok && !out.stdout.trim().is_empty() => {
                        Some(out.stdout.trim().to_string())
                    }
                    _ => None,
                };
                ruleset_bypass.insert(id, bypass);
            }
        }

        // (3) Classic protection. Needs Administration read, so the D-13
        // token usually gets a 403 here, which stays `Unreadable`.
        let classic = match self
            .run(
                "gh",
                &[
                    "api",
                    &format!("repos/{repo}/branches/{branch}/protection"),
                    "--jq",
                    "[(.required_pull_request_reviews != null), (.enforce_admins.enabled // false)] | @tsv",
                ],
            )
            .await
        {
            Some(out) if out.ok => parse_classic_tsv(&out.stdout),
            Some(out)
                if format!("{}\n{}", out.stdout, out.stderr).contains("Branch not protected") =>
            {
                ClassicProtection::NotProtected
            }
            _ => ClassicProtection::Unreadable,
        };

        // (4) Is this token an admin of the repo? Only matters when a classic
        // PR rule does not enforce admins.
        let token_is_admin = if matches!(
            classic,
            ClassicProtection::Protected {
                pr_required: true,
                enforce_admins: false
            }
        ) {
            match self
                .run(
                    "gh",
                    &[
                        "api",
                        &format!("repos/{repo}"),
                        "--jq",
                        ".permissions.admin",
                    ],
                )
                .await
            {
                Some(out) if out.ok => match out.stdout.trim() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                },
                _ => None,
            }
        } else {
            None
        };

        branch_protection_verdict(&ProtectionInputs {
            rules,
            ruleset_bypass,
            classic,
            token_is_admin,
        })
    }
}

/// What the classic branch-protection endpoint told us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClassicProtection {
    /// Readable. `pr_required` is the presence of `required_pull_request_reviews`.
    Protected {
        pr_required: bool,
        enforce_admins: bool,
    },
    /// The API said `Branch not protected`.
    NotProtected,
    /// Forbidden, failed or unparseable (the 403 the D-13 token gets).
    Unreadable,
}

/// Everything the verdict is computed from.
#[derive(Debug, Clone)]
pub(crate) struct ProtectionInputs {
    /// Active ruleset rules as `(type, ruleset_id)`; `None` when the read failed.
    pub(crate) rules: Option<Vec<(String, Option<u64>)>>,
    /// `current_user_can_bypass` per PR-rule ruleset id; `None` when unreadable.
    pub(crate) ruleset_bypass: std::collections::HashMap<u64, Option<String>>,
    pub(crate) classic: ClassicProtection,
    /// `permissions.admin` of the requesting credential.
    pub(crate) token_is_admin: Option<bool>,
}

/// Parse `gh api --paginate ... --jq '.[] | [.type, ruleset_id] | @tsv'`
/// output. Ruleset id `0` means absent. A malformed line makes the whole parse
/// `None` rather than silently dropping a rule.
pub(crate) fn parse_rules_tsv(output: &str) -> Option<Vec<(String, Option<u64>)>> {
    let mut rules = Vec::new();
    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (kind, id) = line.split_once('\t')?;
        let id: u64 = id.trim().parse().ok()?;
        if kind.is_empty() {
            return None;
        }
        rules.push((kind.to_string(), if id == 0 { None } else { Some(id) }));
    }
    Some(rules)
}

/// Parse the `[pr_required, enforce_admins] | @tsv` line of the classic read.
fn parse_classic_tsv(output: &str) -> ClassicProtection {
    let Some((pr, admins)) = output.trim().split_once('\t') else {
        return ClassicProtection::Unreadable;
    };
    let flag = |v: &str| match v.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    match (flag(pr), flag(admins)) {
        (Some(pr_required), Some(enforce_admins)) => ClassicProtection::Protected {
            pr_required,
            enforce_admins,
        },
        _ => ClassicProtection::Unreadable,
    }
}

/// Decide whether the base branch is protected FOR THIS CREDENTIAL (D-10).
///
/// `Effective` requires a pull-request rule this token cannot bypass. A rule it
/// can bypass is `Bypassable`; no PR requirement is `NotProtected`; anything
/// undecidable is `Unknown`. A failed or unreadable read can never produce
/// `Effective`, and a classic HTTP 200 without a PR requirement is not
/// protection.
pub(crate) fn branch_protection_verdict(inputs: &ProtectionInputs) -> BranchProtection {
    let pr_rulesets: Vec<Option<u64>> = inputs
        .rules
        .iter()
        .flatten()
        .filter(|(kind, _)| kind == "pull_request")
        .map(|(_, id)| *id)
        .collect();
    let bypass_of = |id: Option<u64>| -> Option<&str> {
        id.and_then(|i| inputs.ruleset_bypass.get(&i))
            .and_then(|b| b.as_deref())
    };

    // Effective: a PR rule this credential cannot bypass.
    if pr_rulesets
        .iter()
        .any(|id| matches!(bypass_of(*id), Some("never" | "pull_requests_only")))
    {
        return BranchProtection::Effective;
    }
    if let ClassicProtection::Protected {
        pr_required: true,
        enforce_admins,
    } = inputs.classic
        && (enforce_admins || inputs.token_is_admin == Some(false))
    {
        return BranchProtection::Effective;
    }

    // Otherwise only a fully readable picture can say NotProtected/Bypassable:
    // every PR ruleset's bypass state must be a known bypassing value.
    let rules_known = inputs.rules.is_some()
        && pr_rulesets
            .iter()
            .all(|id| matches!(bypass_of(*id), Some("always" | "exempt")));
    // `Some(true)` = a classic PR rule this token can bypass; `None` = unknown.
    let classic_pr_bypassable = match inputs.classic {
        ClassicProtection::NotProtected
        | ClassicProtection::Protected {
            pr_required: false, ..
        } => Some(false),
        ClassicProtection::Protected {
            pr_required: true, ..
        } => inputs.token_is_admin.map(|_| true),
        ClassicProtection::Unreadable => None,
    };
    match (rules_known, classic_pr_bypassable) {
        (true, Some(classic_bypassable)) => {
            if classic_bypassable || !pr_rulesets.is_empty() {
                BranchProtection::Bypassable
            } else {
                BranchProtection::NotProtected
            }
        }
        _ => BranchProtection::Unknown,
    }
}

/// Print the site deploy preflight section.
async fn print_site_preflight(config: &Config, home: &Path) {
    let probe = RealProbe::from_config(config, home);
    let lines = site_preflight_lines(config, home, &probe).await;
    println!();
    let mut iter = lines.into_iter();
    if let Some(header) = iter.next() {
        println!("{}", header.bold());
    }
    for line in iter {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};

    use ironhermes_core::config::Config;
    use ironhermes_core::config::DefconLevel;
    use ironhermes_security::SidecarVersion;
    use secrecy::SecretString;

    // -------------------------------------------------------------------
    // Phase 57 Plan 15 (D-12): the pure half of the security sidecar report.
    // -------------------------------------------------------------------

    fn sidecar_version(core: &str, build: &str) -> SidecarVersion {
        SidecarVersion {
            sidecar: build.to_string(),
            tirith_core: core.to_string(),
            protocol: 1,
            threat_db: "absent".to_string(),
        }
    }

    fn level(n: u8) -> DefconLevel {
        DefconLevel::try_from(n).expect("valid level")
    }

    fn labels(report: &[(String, bool)]) -> Vec<&str> {
        report.iter().map(|(l, _)| l.as_str()).collect()
    }

    #[test]
    fn security_sidecar_report_pure() {
        // Present, pinned, same build, answering: every line is OK.
        let report = format_sidecar_report(
            Some(PathBuf::from("/opt/bin/ironhermes-tirith")),
            Ok(sidecar_version("0.4.2", "0.5.0")),
            Some(4),
            level(3),
            "0.5.0",
        );
        assert_eq!(
            report,
            vec![
                (
                    "sidecar present: /opt/bin/ironhermes-tirith".to_string(),
                    true
                ),
                (
                    "sidecar build: ironhermes-tirith 0.5.0 matches ironhermes 0.5.0".to_string(),
                    true
                ),
                ("tirith-core 0.4.2 (pinned)".to_string(), true),
                ("protocol 1".to_string(), true),
                ("threat DB: absent".to_string(), true),
                ("health: ok (4 ms)".to_string(), true),
                ("DEFCON 3 (yolo bypass inactive)".to_string(), true),
            ]
        );

        // Absent at level 3: the consequence is spelled out and is a failure.
        let report = format_sidecar_report(
            None,
            Err("ironhermes-tirith not found".to_string()),
            None,
            level(3),
            "0.5.0",
        );
        let ls = labels(&report);
        assert!(ls[0].starts_with("sidecar absent"), "{ls:?}");
        assert!(!report[0].1, "absent is a failed check: {report:?}");
        assert!(
            ls.iter().any(|l| l
                .contains("terminal/execute_code will be blocked until the sidecar is installed")),
            "{ls:?}"
        );
        assert!(
            !ls.iter().any(|l| l.contains("health: ok")),
            "an absent sidecar is never reported healthy: {ls:?}"
        );

        // A different tirith-core and build: both pin checks fail and name 0.4.2.
        let report = format_sidecar_report(
            Some(PathBuf::from("/x/ironhermes-tirith-fake")),
            Ok(sidecar_version("fake", "fake")),
            Some(1),
            level(3),
            "0.5.0",
        );
        let pin = report
            .iter()
            .find(|(l, _)| l.starts_with("tirith-core"))
            .expect("pin line");
        assert!(!pin.1, "{pin:?}");
        assert!(pin.0.contains("fake") && pin.0.contains("0.4.2"), "{pin:?}");
        let build = report
            .iter()
            .find(|(l, _)| l.starts_with("sidecar build"))
            .expect("build line");
        assert!(!build.1, "{build:?}");
        assert!(build.0.contains("0.5.0"), "{build:?}");

        // Present but not answering: unhealthy, with the level-4 consequence.
        let report = format_sidecar_report(
            Some(PathBuf::from("/opt/bin/ironhermes-tirith")),
            Err("sidecar unavailable (timeout): no answer within 3s".to_string()),
            None,
            level(4),
            "0.5.0",
        );
        let ls = labels(&report);
        assert!(
            report.iter().any(|(l, ok)| l.starts_with("health") && !ok),
            "{ls:?}"
        );
        assert!(
            ls.iter().any(|l| l.contains("DEFCON 4")
                && l.contains("Warn")
                && l.contains("critical notice")),
            "{ls:?}"
        );
        assert!(
            ls.iter().any(|l| l.contains("yolo bypass active")),
            "levels 4-5 state that bypass is active: {ls:?}"
        );
    }

    #[test]
    fn sidecar_lookup_description_names_every_place_tried() {
        let text = describe_sidecar_lookup(
            Some(Path::new("/opt/ironhermes/bin/ironhermes")),
            Some(std::ffi::OsStr::new("/custom/tirith")),
        );
        assert!(
            text.contains("/opt/ironhermes/bin/ironhermes-tirith"),
            "{text}"
        );
        assert!(
            text.contains("IRONHERMES_TIRITH_BIN=/custom/tirith"),
            "{text}"
        );
        assert!(text.contains("PATH"), "{text}");
        let unset = describe_sidecar_lookup(None, None);
        assert!(unset.contains("IRONHERMES_TIRITH_BIN unset"), "{unset}");
    }

    // -------------------------------------------------------------------
    // Per-provider key coverage (venice-401 debug follow-up).
    //
    // These assert the DECISION logic, not the formatting: a provider with a
    // resolvable key is healthy, a keyless LOOPBACK provider is healthy (the
    // runtime waives the key there), and a keyless remote provider is not —
    // and names the variable to set.
    // -------------------------------------------------------------------

    /// Build a `Config` with the given providers, so the resolver has real
    /// endpoints to report on.
    fn coverage_config(entries: &[(&str, Option<&str>, &str)], main: &str) -> Config {
        use ironhermes_core::config::ProviderConfig;
        let mut config = Config::default();
        for (name, key_env, base_url) in entries {
            config.providers.insert(
                (*name).to_string(),
                ProviderConfig {
                    base_url: Some((*base_url).to_string()),
                    api_key_env: key_env.map(|s| s.to_string()),
                    ..Default::default()
                },
            );
        }
        config.model.provider = main.to_string();
        config
    }

    fn line_for<'a>(lines: &'a [(String, bool)], provider: &str) -> &'a (String, bool) {
        lines
            .iter()
            .find(|(l, _)| l.contains(&format!("provider {provider} ")))
            .unwrap_or_else(|| panic!("no coverage line for {provider}: {lines:?}"))
    }

    #[test]
    fn keyless_loopback_provider_is_reported_healthy() {
        // A local endpoint needs no key — the runtime's own guard waives it,
        // so doctor must not cry MISSING and send the operator hunting for a
        // variable that should not exist.
        let config = coverage_config(
            &[("localllm", None, "http://127.0.0.1:1337/v1")],
            "localllm",
        );
        let resolver = ProviderResolver::build(&config).expect("resolver builds");
        let lines = provider_key_coverage_lines(&config, &resolver);
        let (label, ok) = line_for(&lines, "localllm");
        assert!(ok, "keyless loopback must be healthy: {label}");
        assert!(
            label.contains("keyless"),
            "must say WHY it is healthy: {label}"
        );
    }

    #[test]
    fn keyless_remote_provider_is_flagged_and_names_the_variable() {
        // The venice-401 case: a remote provider with no resolvable key. The
        // operator needs the variable NAME, not just a red line.
        let config = coverage_config(
            &[("acme", Some("ACME_DOES_NOT_EXIST_IN_ENV"), "https://api.acme.test/v1")],
            "acme",
        );
        let resolver = ProviderResolver::build(&config).expect("resolver builds");
        let lines = provider_key_coverage_lines(&config, &resolver);
        let (label, ok) = line_for(&lines, "acme");
        assert!(!ok, "keyless remote provider must be flagged: {label}");
        assert!(
            label.contains("ACME_DOES_NOT_EXIST_IN_ENV"),
            "must name the env var to set: {label}"
        );
    }

    #[test]
    fn the_active_provider_is_marked() {
        // A broken ACTIVE provider must be distinguishable from a dormant
        // one — that distinction is the whole reason the venice failure was
        // hard to spot.
        let config = coverage_config(
            &[
                ("acme", Some("ACME_ABSENT"), "https://api.acme.test/v1"),
                ("other", Some("OTHER_ABSENT"), "https://api.other.test/v1"),
            ],
            "acme",
        );
        let resolver = ProviderResolver::build(&config).expect("resolver builds");
        let lines = provider_key_coverage_lines(&config, &resolver);
        assert!(
            line_for(&lines, "acme").0.contains("[ACTIVE]"),
            "the main provider must be marked: {lines:?}"
        );
        assert!(
            !line_for(&lines, "other").0.contains("[ACTIVE]"),
            "a non-main provider must NOT be marked: {lines:?}"
        );
    }

    #[test]
    fn is_vision_capable_model_true_cases() {
        assert!(is_vision_capable_model("anthropic/claude-sonnet-4"));
        assert!(is_vision_capable_model("openai/gpt-4o"));
        assert!(is_vision_capable_model("google/gemini-2.0-flash"));
    }

    #[test]
    fn is_vision_capable_model_false_cases() {
        assert!(!is_vision_capable_model("mistralai/mistral-7b-instruct"));
        assert!(!is_vision_capable_model("deepseek/deepseek-chat"));
    }

    // -----------------------------------------------------------------
    // Phase 41.3 Plan 09 (D-08/D-19): web-tool provider coverage tests.
    //
    // An async-aware `tokio::sync::Mutex`, not `std::sync::Mutex` — every
    // test here is `#[tokio::test]` and holds the guard across
    // `ToolCredentials::resolve(...).await`, which `clippy::await_holding_lock`
    // (denied under this workspace's `-D warnings` gate) forbids for a std
    // Mutex guard. Mirrors `tests/tool_credentials.rs`'s (Plan 10) identical
    // convention.
    // -----------------------------------------------------------------

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Remove every `TOOL_CREDENTIAL_KEYS` member from the process env so a
    /// developer's real `.env`/shell exports never leak into these tests —
    /// mirrors `tool_credentials.rs`'s `clear_all_credential_env_vars`.
    fn clear_all_web_provider_env_vars() {
        for key in ironhermes_tools::credentials::TOOL_CREDENTIAL_KEYS {
            // SAFETY: caller holds env_lock(); single-threaded test binary
            // (--test-threads=1, project-wide convention for env-mutating tests).
            unsafe { std::env::remove_var(key) };
        }
    }

    #[derive(Clone)]
    enum FakeVaultResponse {
        Found(String),
        Sealed,
    }

    /// Test-only `SecretStore` — mirrors `tests/tool_credentials.rs`'s
    /// `MockStore` shape, trimmed to what `resolve_doctor_credentials_with_store`
    /// actually calls (`get_secret` only).
    struct FakeVaultStore {
        responses: HashMap<String, FakeVaultResponse>,
    }

    #[async_trait::async_trait]
    impl ironhermes_vault::SecretStore for FakeVaultStore {
        async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
            match self.responses.get(key) {
                Some(FakeVaultResponse::Found(v)) => Ok(Some(SecretString::from(v.clone()))),
                Some(FakeVaultResponse::Sealed) => Err(anyhow::anyhow!(
                    "vault is sealed — run `ironhermes vault unlock`"
                )),
                None => Ok(None),
            }
        }
        async fn put_secret(&self, _key: &str, _value: SecretString) -> anyhow::Result<()> {
            unimplemented!("doctor never writes secrets")
        }
        async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
            unimplemented!("doctor never deletes secrets")
        }
        async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
            unimplemented!("doctor never lists secrets")
        }
    }

    /// Build a registry with `credentials` installed and defaults registered
    /// — the exact order `run_doctor_check` itself follows (`with_credentials`
    /// before `register_defaults`, or `WebSearchTool`/`WebAnswerTool` would be
    /// constructed against the placeholder env-only snapshot instead).
    fn registry_with_credentials(
        credentials: Arc<ToolCredentials>,
    ) -> ironhermes_tools::ToolRegistry {
        let mut registry = ironhermes_tools::ToolRegistry::new();
        registry.with_credentials(credentials);
        registry.register_defaults();
        registry
    }

    #[tokio::test]
    async fn tool_provider_status_renders_a_group_count() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();
        // SAFETY: env_lock held, --test-threads=1.
        unsafe { std::env::set_var("EXA_API_KEY", "test-value") };

        let config = Config::default();
        let creds = Arc::new(
            ToolCredentials::resolve(&config, None)
                .await
                .expect("resolve must succeed with no store"),
        );
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        unsafe { std::env::remove_var("EXA_API_KEY") };

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("1/3"),
            "one of three web_search providers configured -> \"1/3\": {web_search_line}"
        );
    }

    #[tokio::test]
    async fn tool_provider_status_reports_zero_without_hard_failing() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let config = Config::default();
        let creds = Arc::new(
            ToolCredentials::resolve(&config, None)
                .await
                .expect("resolve must succeed with no store"),
        );
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("0/3"),
            "zero providers configured -> \"0/3\": {web_search_line}"
        );
        assert!(
            web_search_line.contains("keyless default"),
            "a 0/3 must note the tool stays available (D-09), not read as broken: {web_search_line}"
        );
    }

    #[test]
    fn chain_validation_reports_an_unknown_provider() {
        let mut config = Config::default();
        config.tools.web_search.chain = vec!["not_a_real_provider".to_string()];

        let lines = render_chain_validation(&config);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("not_a_real_provider") && l.contains("tools.web_search.chain")),
            "must name both the offending value and the key path: {lines:?}"
        );
    }

    #[test]
    fn chain_validation_is_silent_on_a_valid_config() {
        let config = Config::default();
        assert!(
            render_chain_validation(&config).is_empty(),
            "a default (valid) config must produce no chain-validation lines"
        );
    }

    #[tokio::test]
    async fn provider_status_counts_a_key_configured_only_in_the_config_tier() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let mut config = Config::default();
        config
            .tools
            .credentials
            .insert("BRAVE_API_KEY".to_string(), "config-tier-value".to_string());

        let creds = Arc::new(
            ToolCredentials::resolve(&config, None)
                .await
                .expect("resolve must succeed with no store"),
        );
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("1/3"),
            "a config-tier-only key must be counted, not treated as a false alarm (D-19): {web_search_line}"
        );
        assert!(
            web_search_line.contains("BRAVE_API_KEY (Config)"),
            "the satisfying tier must be named: {web_search_line}"
        );
    }

    #[tokio::test]
    async fn provider_status_counts_a_key_configured_only_in_the_vault() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let config = Config::default();
        let mut responses = HashMap::new();
        responses.insert(
            "TAVILY_API_KEY".to_string(),
            FakeVaultResponse::Found("vault-tier-value".to_string()),
        );
        let store = FakeVaultStore { responses };

        let (creds, failed) = resolve_doctor_credentials_with_store(&config, Some(&store)).await;
        assert!(
            failed.is_empty(),
            "a healthy vault must not produce a failed check: {failed:?}"
        );

        let creds = Arc::new(creds);
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("1/3"),
            "a vault-tier-only key must be counted (D-19): {web_search_line}"
        );
        assert!(
            web_search_line.contains("TAVILY_API_KEY (Vault)"),
            "the satisfying tier must be named: {web_search_line}"
        );
    }

    #[tokio::test]
    async fn provider_status_never_prints_a_configured_value() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();
        // SAFETY: env_lock held, --test-threads=1.
        unsafe { std::env::set_var("EXA_API_KEY", "sentinel-env-9f3a") };

        let mut config = Config::default();
        config.tools.credentials.insert(
            "BRAVE_API_KEY".to_string(),
            "sentinel-config-7b21".to_string(),
        );
        let mut responses = HashMap::new();
        responses.insert(
            "TAVILY_API_KEY".to_string(),
            FakeVaultResponse::Found("sentinel-vault-c405".to_string()),
        );
        let store = FakeVaultStore { responses };

        let (creds, _failed) = resolve_doctor_credentials_with_store(&config, Some(&store)).await;
        let creds = Arc::new(creds);
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);

        unsafe { std::env::remove_var("EXA_API_KEY") };

        let rendered = lines.join("\n");
        for sentinel in [
            "sentinel-env-9f3a",
            "sentinel-config-7b21",
            "sentinel-vault-c405",
        ] {
            assert!(
                !rendered.contains(sentinel),
                "rendered output must never contain a configured value; found {sentinel} in: {rendered}"
            );
        }
        // Positive control: prove all three tiers were actually exercised, so
        // the negative assertion above isn't vacuously true.
        let web_search_line = lines
            .iter()
            .find(|l| l.starts_with("web_search:"))
            .unwrap_or_else(|| panic!("expected a web_search line, got: {lines:?}"));
        assert!(
            web_search_line.contains("3/3"),
            "all three tiers (env/config/vault) must be counted: {web_search_line}"
        );
    }

    #[tokio::test]
    async fn a_sealed_vault_is_a_failed_check_not_a_panic() {
        let _g = env_lock().lock().await;
        clear_all_web_provider_env_vars();

        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();

        let mut responses = HashMap::new();
        responses.insert("EXA_API_KEY".to_string(), FakeVaultResponse::Sealed);
        let store = FakeVaultStore { responses };

        // Must not panic and must not propagate an Err — doctor never errors.
        let (creds, failed) = resolve_doctor_credentials_with_store(&config, Some(&store)).await;

        assert!(
            !failed.is_empty(),
            "a sealed vault must render a failed check, not silence"
        );
        assert!(
            failed.iter().any(|m| m.contains("rusty-vault")),
            "the failed check must name the configured backend: {failed:?}"
        );
        for msg in &failed {
            assert!(
                !msg.to_lowercase().contains("sentinel"),
                "a failed-check message must never carry secret material: {msg}"
            );
        }

        // Doctor keeps going with a degraded snapshot — never panics rendering it.
        let creds = Arc::new(creds);
        let registry = registry_with_credentials(creds.clone());
        let lines = render_tool_provider_status(&registry, &creds);
        assert!(
            !lines.is_empty(),
            "doctor must still render provider status after a sealed vault"
        );
    }

    // -----------------------------------------------------------------
    // 48.3-05 D-06: gateway log status (`gateway_log_lines`) tests.
    //
    // `now` is always injected explicitly so the stale/fresh boundary is
    // testable without sleeping — see `gateway_log_lines`'s doc comment.
    // -----------------------------------------------------------------

    #[test]
    fn gateway_log_lines_always_names_the_absolute_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let now = std::time::SystemTime::now();
        let expected_path = current_gateway_log(home);
        let expected_path_str = expected_path.to_string_lossy().to_string();

        // Missing state.
        let lines = gateway_log_lines(home, now);
        assert!(
            lines.iter().any(|l| l.contains(&expected_path_str)),
            "missing-file lines must name the absolute canonical path: {lines:?}"
        );

        // Fresh state.
        std::fs::create_dir_all(expected_path.parent().unwrap()).unwrap();
        std::fs::write(&expected_path, b"log line\n").unwrap();
        let lines = gateway_log_lines(home, now);
        assert!(
            lines.iter().any(|l| l.contains(&expected_path_str)),
            "fresh-file lines must name the absolute canonical path: {lines:?}"
        );

        // Stale state.
        let stale_now = now
            .checked_add(GATEWAY_LOG_FRESH_WINDOW + std::time::Duration::from_secs(60))
            .unwrap();
        let lines = gateway_log_lines(home, stale_now);
        assert!(
            lines.iter().any(|l| l.contains(&expected_path_str)),
            "stale-file lines must name the absolute canonical path: {lines:?}"
        );
    }

    #[test]
    fn gateway_log_lines_reports_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let now = std::time::SystemTime::now();

        let lines = gateway_log_lines(home, now);
        let joined = lines.join("\n").to_lowercase();
        assert!(
            joined.contains("does not exist") || joined.contains("not exist"),
            "missing-file lines must say the file does not exist yet: {lines:?}"
        );
        assert!(
            joined.contains("install.sh"),
            "missing-file lines must name the installer as the thing that creates it: {lines:?}"
        );
    }

    #[test]
    fn gateway_log_lines_reports_stale_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let log_path = current_gateway_log(home);
        std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        std::fs::write(&log_path, b"old\n").unwrap();

        // now is far enough past the file's mtime (roughly "now" at write
        // time) to land outside the freshness window.
        let now = std::time::SystemTime::now()
            .checked_add(GATEWAY_LOG_FRESH_WINDOW + std::time::Duration::from_secs(3600))
            .unwrap();

        let lines = gateway_log_lines(home, now);
        let joined = lines.join("\n").to_lowercase();
        assert!(joined.contains("stale"), "stale lines must say stale: {lines:?}");
        assert!(
            joined.contains("age") || joined.contains("old") || joined.contains("hour") || joined.contains("day"),
            "stale lines must state the age: {lines:?}"
        );
    }

    #[test]
    fn gateway_log_lines_reports_fresh_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let log_path = current_gateway_log(home);
        std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        std::fs::write(&log_path, b"fresh\n").unwrap();

        let now = std::time::SystemTime::now();

        let lines = gateway_log_lines(home, now);
        let joined = lines.join("\n").to_lowercase();
        assert!(
            joined.contains("current"),
            "fresh-file lines must report the log as current: {lines:?}"
        );
        assert!(
            !joined.contains("stale"),
            "fresh-file lines must not say stale: {lines:?}"
        );
    }

    // -----------------------------------------------------------------
    // 54.8-04 D-08: doctor names the CURRENT log — the newest of the
    // canonical `gateway.log` and the dated `gateway.log.YYYY-MM-DD`.
    // -----------------------------------------------------------------

    /// Write `name` under `<home>/logs/` and set its mtime `age` in the past.
    fn write_log_aged(home: &Path, name: &str, age: Duration) -> PathBuf {
        let logs = home.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join(name);
        std::fs::write(&path, b"line\n").unwrap();
        let when = SystemTime::now().checked_sub(age).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        path
    }

    #[test]
    fn gateway_log_lines_reports_newest_rolled_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        write_log_aged(home, "gateway.log", Duration::from_secs(3 * 24 * 3600));
        let dated = write_log_aged(home, "gateway.log.2026-10-17", Duration::ZERO);

        let lines = gateway_log_lines(home, SystemTime::now());
        assert_eq!(lines[0], format!("Gateway log: {}", dated.display()));
        let joined = lines.join("\n").to_lowercase();
        assert!(
            joined.contains("current"),
            "dated file is current: {lines:?}"
        );
        assert!(
            !joined.contains("stale"),
            "dated file is not stale: {lines:?}"
        );
    }

    #[test]
    fn gateway_log_lines_prefers_canonical_when_newest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let canonical = write_log_aged(home, "gateway.log", Duration::ZERO);
        write_log_aged(
            home,
            "gateway.log.2026-10-15",
            Duration::from_secs(2 * 24 * 3600),
        );
        write_log_aged(
            home,
            "gateway.log.2026-10-16",
            Duration::from_secs(24 * 3600),
        );

        let lines = gateway_log_lines(home, SystemTime::now());
        assert_eq!(lines[0], format!("Gateway log: {}", canonical.display()));
    }

    #[test]
    fn gateway_log_lines_ignores_non_date_suffixes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let canonical = write_log_aged(home, "gateway.log", Duration::from_secs(3600));
        write_log_aged(home, "gateway.log.pre-48.3", Duration::ZERO);

        let lines = gateway_log_lines(home, SystemTime::now());
        assert_eq!(
            lines[0],
            format!("Gateway log: {}", canonical.display()),
            "a non-date suffix must never be chosen even when newest"
        );
    }

    // =========================================================================
    // Phase 53 (Plan 03 Task 3, D-06): ironhermes doctor --browser
    // =========================================================================

    #[test]
    fn doctor_with_the_browser_flag_reports_the_configured_backend() {
        let diagnosis = ironhermes_tools::browser_session::BrowserDiagnosis {
            backend: ironhermes_core::config::BrowserBackend::Obscura,
            mode: "Obscura (local)",
            binary_path: Some(std::path::PathBuf::from("/usr/local/bin/obscura")),
            outcome: ironhermes_tools::browser_session::BrowserDiagnosisOutcome::Ok,
        };
        let joined = render_browser_diagnosis(&diagnosis).join("\n");
        assert!(
            joined.contains("Obscura (local)"),
            "must name the configured backend, got:\n{joined}"
        );
        assert!(
            joined.contains("/usr/local/bin/obscura"),
            "must name the resolved binary path, got:\n{joined}"
        );
    }

    #[test]
    fn doctor_with_the_browser_flag_reports_no_binary_resolved() {
        let diagnosis = ironhermes_tools::browser_session::BrowserDiagnosis {
            backend: ironhermes_core::config::BrowserBackend::Obscura,
            mode: "Obscura (local)",
            binary_path: None,
            outcome: ironhermes_tools::browser_session::BrowserDiagnosisOutcome::Failed(
                "Obscura binary not found. Set OBSCURA_PATH or browser.obscura_path".to_string(),
            ),
        };
        let joined = render_browser_diagnosis(&diagnosis).join("\n");
        assert!(
            joined.contains("MISSING"),
            "no resolved binary must be surfaced as MISSING, got:\n{joined}"
        );
    }

    #[test]
    fn doctor_with_the_browser_flag_reports_a_render_refusal_by_name() {
        use ironhermes_tools::browser_session::BrowserDiagnosisOutcome;
        let diagnosis = ironhermes_tools::browser_session::BrowserDiagnosis {
            backend: ironhermes_core::config::BrowserBackend::Obscura,
            mode: "Obscura (local)",
            binary_path: Some(std::path::PathBuf::from("/opt/obscura/obscura")),
            outcome: BrowserDiagnosisOutcome::RenderUnsupported {
                screenshot_ok: false,
                hidden_reports_none: false,
            },
        };
        let joined = render_browser_diagnosis(&diagnosis).join("\n");
        assert!(
            joined.contains("--features render"),
            "a render refusal must name --features render, got:\n{joined}"
        );
        assert!(
            joined.contains("REFUSED"),
            "a render refusal is a finding, not a silent MISSING, got:\n{joined}"
        );
    }

    #[test]
    fn doctor_without_the_browser_flag_performs_no_browser_work() {
        // The browser block must be gated tightly by `if browser {`, not
        // merely reachable somewhere in the function — a source-shape proof
        // rather than a timing-based one.
        let source = include_str!("doctor.rs");
        let needle = "pub async fn run_doctor_check(browser: bool) -> Result<()> {";
        let start = source
            .find(needle)
            .expect("run_doctor_check must exist in this file");
        let after_start = &source[start..];
        let call_site = "print_browser_diagnosis(&doctor_config).await;";
        let call_idx = after_start
            .find(call_site)
            .expect("print_browser_diagnosis call must exist");
        let preceding = &after_start[..call_idx];
        let guard_idx = preceding
            .rfind("if browser {")
            .expect("the call must be behind an `if browser {` guard");
        assert!(
            call_idx - guard_idx < 200,
            "the browser block must tightly gate the call, not merely appear somewhere \
             earlier in the function"
        );
    }

    // -------------------------------------------------------------------
    // Phase 54.8 Plan 10 (D-21, D-10): site deploy preflight.
    // -------------------------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    const DEMO_SITE_YAML: &str = "repo: example-org/demo-site\nclone_path: workspace/demo-site\ndeploy_mode: wrangler\nstaging:\n  deploy_args: [\"--env\", \"staging\"]\n  url: \"https://demo-site-staging.example.workers.dev\"\nproduction:\n  url: \"https://demo-site.example.workers.dev\"\n";

    fn config_with_demo_site() -> Config {
        let mut config = Config::default();
        config.sites.insert(
            "demo-site".to_string(),
            serde_yaml::from_str(DEMO_SITE_YAML).unwrap(),
        );
        config
    }

    struct FakeProbe {
        missing: Vec<&'static str>,
        node: Option<u32>,
        env_set: Vec<&'static str>,
        skills: HashMap<&'static str, Option<bool>>,
        mcp: HashMap<&'static str, Option<bool>>,
        repo: Option<bool>,
        protection: BranchProtection,
        forbid_network: bool,
        network_calls: AtomicUsize,
    }

    impl FakeProbe {
        fn healthy() -> Self {
            Self {
                missing: Vec::new(),
                node: Some(22),
                env_set: SITE_TOKEN_VARS.to_vec(),
                skills: HashMap::new(),
                mcp: HashMap::new(),
                repo: Some(true),
                protection: BranchProtection::Effective,
                forbid_network: false,
                network_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl SitePreflightProbe for FakeProbe {
        fn on_path(&self, cmd: &str) -> bool {
            !self.missing.contains(&cmd)
        }
        async fn node_major(&self) -> Option<u32> {
            self.node
        }
        fn env_is_set(&self, name: &str) -> bool {
            self.env_set.contains(&name)
        }
        fn skill_visible(&self, name: &str) -> Option<bool> {
            self.skills.get(name).copied().unwrap_or(Some(true))
        }
        async fn mcp_token_present(&self, namespace: &str) -> Option<bool> {
            self.mcp.get(namespace).copied().unwrap_or(Some(true))
        }
        async fn repo_reachable(&self, _repo: &str) -> Option<bool> {
            self.network_calls.fetch_add(1, Ordering::SeqCst);
            assert!(!self.forbid_network, "network probe must not run");
            self.repo
        }
        async fn branch_protection(&self, _repo: &str, _branch: &str) -> BranchProtection {
            self.network_calls.fetch_add(1, Ordering::SeqCst);
            assert!(!self.forbid_network, "network probe must not run");
            self.protection
        }
    }

    async fn preflight(config: &Config, probe: &FakeProbe) -> String {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("workspace")).unwrap();
        site_preflight_lines(config, home.path(), probe)
            .await
            .join("\n")
    }

    #[tokio::test]
    async fn preflight_no_sites_is_one_line_and_offline() {
        let mut probe = FakeProbe::healthy();
        probe.forbid_network = true;
        let home = tempfile::tempdir().unwrap();
        let lines = site_preflight_lines(&Config::default(), home.path(), &probe).await;
        assert_eq!(lines.len(), 2, "header plus one line: {lines:?}");
        assert_eq!(lines[0], "Site deploy preflight");
        assert!(lines[1].contains("no sites configured"));
        assert_eq!(probe.network_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preflight_reports_missing_tools() {
        let mut probe = FakeProbe::healthy();
        probe.missing = vec!["gh", "wrangler"];
        let text = preflight(&config_with_demo_site(), &probe).await;
        assert!(text.contains("gh: missing"), "{text}");
        assert!(
            text.contains("wrangler: missing — install the pinned version globally"),
            "{text}"
        );
        assert!(!text.contains("npx"), "must not suggest npx: {text}");
    }

    #[tokio::test]
    async fn preflight_warns_old_node() {
        let mut probe = FakeProbe::healthy();
        probe.node = Some(20);
        let text = preflight(&config_with_demo_site(), &probe).await;
        assert!(text.contains("needs >= 22"), "{text}");
        probe.node = Some(22);
        let text = preflight(&config_with_demo_site(), &probe).await;
        assert!(!text.contains("needs >= 22"), "{text}");
    }

    #[tokio::test]
    async fn preflight_flags_token_not_allowlisted() {
        let mut config = config_with_demo_site();
        let mut probe = FakeProbe::healthy();
        probe.env_set = vec!["CLOUDFLARE_API_TOKEN"];
        let text = preflight(&config, &probe).await;
        assert!(text.contains("CLOUDFLARE_API_TOKEN: set"), "{text}");
        assert!(
            text.contains("CLOUDFLARE_API_TOKEN: not in terminal.terminal_env_allowlist"),
            "{text}"
        );
        config.terminal.terminal_env_allowlist = vec!["CLOUDFLARE_API_TOKEN".to_string()];
        let text = preflight(&config, &probe).await;
        assert!(
            !text.contains("CLOUDFLARE_API_TOKEN: not in terminal.terminal_env_allowlist"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn preflight_timeout_override_warning() {
        let probe = FakeProbe::healthy();
        let mut config = config_with_demo_site();
        let text = preflight(&config, &probe).await;
        assert!(!text.contains("terminal timeout"), "{text}");
        config
            .tools
            .timeout_overrides
            .insert("terminal".to_string(), 60);
        let text = preflight(&config, &probe).await;
        assert!(text.contains("terminal timeout"), "{text}");
        config
            .tools
            .timeout_overrides
            .insert("terminal".to_string(), 300);
        let text = preflight(&config, &probe).await;
        assert!(!text.contains("terminal timeout"), "{text}");
    }

    #[tokio::test]
    async fn preflight_reports_skill_visibility() {
        let mut probe = FakeProbe::healthy();
        probe.skills.insert("wrangler", Some(false));
        probe.skills.insert("site-deploy", Some(true));
        let text = preflight(&config_with_demo_site(), &probe).await;
        assert!(text.contains("skill wrangler: not visible"), "{text}");
        assert!(text.contains("skill site-deploy: visible"), "{text}");
    }

    #[tokio::test]
    async fn preflight_reports_mcp_auth() {
        let mut config = config_with_demo_site();
        config.mcp_servers.insert(
            "cloudflare_bindings".to_string(),
            serde_yaml::from_str(
                "url: \"https://bindings.example.com/mcp\"\noauth_provider: cloudflare_bindings\n",
            )
            .unwrap(),
        );
        config.mcp_servers.insert(
            "plain".to_string(),
            serde_yaml::from_str("url: \"https://plain.example.com/mcp\"\n").unwrap(),
        );
        let mut probe = FakeProbe::healthy();
        probe.mcp.insert("cloudflare_bindings", Some(false));
        let text = preflight(&config, &probe).await;
        assert!(
            text.contains("ironhermes mcp connect cloudflare_bindings --no-browser"),
            "{text}"
        );
        assert!(
            !text.contains("MCP plain"),
            "a server without oauth_provider is not reported: {text}"
        );
        probe.mcp.insert("cloudflare_bindings", Some(true));
        let text = preflight(&config, &probe).await;
        assert!(
            text.contains("MCP cloudflare_bindings: authorized"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn preflight_reports_branch_protection_states() {
        let cases = [
            (BranchProtection::Effective, "base branch main: protected ("),
            (
                BranchProtection::Bypassable,
                "NOT protected — a pull-request rule exists but this token can bypass it",
            ),
            (BranchProtection::NotProtected, "NOT protected — enable"),
            (BranchProtection::Unknown, "base branch main: unknown"),
        ];
        for (verdict, expected) in cases {
            let mut probe = FakeProbe::healthy();
            probe.protection = verdict;
            let text = preflight(&config_with_demo_site(), &probe).await;
            assert!(text.contains(expected), "{verdict:?}: {text}");
        }
    }

    #[tokio::test]
    async fn preflight_reports_invalid_site_by_field() {
        let mut config = Config::default();
        config.sites.insert(
            "broken".to_string(),
            serde_yaml::from_str("repo: x\ndeploy_mode: bogus\n").unwrap(),
        );
        let mut probe = FakeProbe::healthy();
        probe.forbid_network = true;
        let text = preflight(&config, &probe).await;
        assert!(text.contains("site broken: INVALID"), "{text}");
        assert!(text.contains("entry"), "{text}");
        assert_eq!(probe.network_calls.load(Ordering::SeqCst), 0);
    }

    // -- effective-protection verdict (D-10, review 10 HIGH) ---------------

    fn rule(kind: &str, id: u64) -> (String, Option<u64>) {
        (kind.to_string(), Some(id))
    }

    fn inputs(
        rules: Option<Vec<(String, Option<u64>)>>,
        bypass: &[(u64, Option<&str>)],
        classic: ClassicProtection,
        admin: Option<bool>,
    ) -> ProtectionInputs {
        ProtectionInputs {
            rules,
            ruleset_bypass: bypass
                .iter()
                .map(|(id, v)| (*id, v.map(str::to_string)))
                .collect(),
            classic,
            token_is_admin: admin,
        }
    }

    #[test]
    fn branch_protection_verdict_table() {
        use BranchProtection::*;
        use ClassicProtection::{NotProtected as ClassicNone, Protected, Unreadable};
        let pr1 = || Some(vec![rule("pull_request", 1)]);
        let cases: Vec<(&str, ProtectionInputs, BranchProtection)> = vec![
            (
                "PR ruleset, bypass never",
                inputs(pr1(), &[(1, Some("never"))], Unreadable, None),
                Effective,
            ),
            (
                "PR ruleset, bypass pull_requests_only",
                inputs(pr1(), &[(1, Some("pull_requests_only"))], Unreadable, None),
                Effective,
            ),
            (
                "PR ruleset, bypass always",
                inputs(pr1(), &[(1, Some("always"))], ClassicNone, None),
                Bypassable,
            ),
            (
                "PR ruleset, bypass exempt",
                inputs(pr1(), &[(1, Some("exempt"))], ClassicNone, None),
                Bypassable,
            ),
            (
                "PR ruleset, bypass unreadable",
                inputs(pr1(), &[(1, None)], ClassicNone, None),
                Unknown,
            ),
            (
                "no rules, classic none",
                inputs(Some(vec![]), &[], ClassicNone, None),
                NotProtected,
            ),
            (
                "no rules, classic unreadable",
                inputs(Some(vec![]), &[], Unreadable, None),
                Unknown,
            ),
            (
                "ruleset without a PR rule",
                inputs(Some(vec![rule("deletion", 2)]), &[], ClassicNone, None),
                NotProtected,
            ),
            (
                "rules call failed",
                inputs(None, &[], ClassicNone, None),
                Unknown,
            ),
            (
                "classic PR required, admins enforced",
                inputs(
                    None,
                    &[],
                    Protected {
                        pr_required: true,
                        enforce_admins: true,
                    },
                    None,
                ),
                Effective,
            ),
            (
                "classic HTTP 200 with the PR requirement disabled",
                inputs(
                    Some(vec![]),
                    &[],
                    Protected {
                        pr_required: false,
                        enforce_admins: true,
                    },
                    None,
                ),
                NotProtected,
            ),
            (
                "classic PR required, admins not enforced, admin token",
                inputs(
                    Some(vec![]),
                    &[],
                    Protected {
                        pr_required: true,
                        enforce_admins: false,
                    },
                    Some(true),
                ),
                Bypassable,
            ),
            (
                "classic PR required, admins not enforced, non-admin token",
                inputs(
                    Some(vec![]),
                    &[],
                    Protected {
                        pr_required: true,
                        enforce_admins: false,
                    },
                    Some(false),
                ),
                Effective,
            ),
            (
                "classic PR required, admins not enforced, admin bit unknown",
                inputs(
                    Some(vec![]),
                    &[],
                    Protected {
                        pr_required: true,
                        enforce_admins: false,
                    },
                    None,
                ),
                Unknown,
            ),
        ];
        for (name, input, expected) in cases {
            assert_eq!(branch_protection_verdict(&input), expected, "{name}");
        }
    }

    #[test]
    fn parse_paginated_rules_lines() {
        assert_eq!(
            parse_rules_tsv("pull_request\t11\ndeletion\t12\npull_request\t13"),
            Some(vec![
                rule("pull_request", 11),
                rule("deletion", 12),
                rule("pull_request", 13)
            ])
        );
        assert_eq!(parse_rules_tsv(""), Some(vec![]));
        assert_eq!(parse_rules_tsv("\n"), Some(vec![]));
        assert_eq!(parse_rules_tsv("pull_request\t11\nnot-a-rule-line"), None);
        assert_eq!(parse_rules_tsv("pull_request\tabc"), None);
        // ruleset_id 0 (absent) maps to None.
        assert_eq!(
            parse_rules_tsv("pull_request\t0"),
            Some(vec![("pull_request".to_string(), None)])
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_probe_kills_a_hung_child_within_deadline() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let pid_file = bin.path().join("gh.pid");
        let script = format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
            pid_file.display()
        );
        let gh = bin.path().join("gh");
        std::fs::write(&gh, script).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = HashMap::new();
        env.insert(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", bin.path().display()),
        );
        let home = tempfile::tempdir().unwrap();
        let probe = RealProbe::with_env(
            Duration::from_millis(1500),
            env,
            home.path(),
            ironhermes_core::config::SkillsConfig::default(),
        );
        let started = std::time::Instant::now();
        assert_eq!(probe.repo_reachable("example-org/demo-site").await, None);
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the deadline must bound the probe, took {:?}",
            started.elapsed()
        );
        let pid = std::fs::read_to_string(&pid_file)
            .expect("the fake gh must have started")
            .trim()
            .to_string();
        // kill_on_drop reaps asynchronously; poll briefly for the child to go.
        let mut alive = true;
        for _ in 0..40 {
            let status = tokio::process::Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .unwrap();
            alive = status.success();
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !alive,
            "the hung gh child (pid {pid}) must not survive the deadline"
        );
    }
}
