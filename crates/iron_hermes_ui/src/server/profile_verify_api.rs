//! Phase 47.4 Plan 06 (D-09 / D-11 / D-14): the real, profile-scoped judge
//! probe.
//!
//! This phase exists to prevent a kanban worker crashing at judge-build time
//! roughly one second after spawn. A step that printed "judge model
//! reachable" from a presence check alone would be exactly the class of
//! self-verifying green signal that has produced false confidence on this
//! project before — so `verify_profile` (added by this file's second task)
//! performs the three synchronous disk checks D-11 already established PLUS
//! an actual attempt to build the judge closure and issue a minimal provider
//! call, scoped to the TARGET profile's own keys via the D-14 override map,
//! never the server process's own environment. As of Plan 17 (CR-01) this
//! guarantee is upheld by the STRICT resolvers —
//! `resolve_judge_model_and_endpoint_with_env_overrides_strict` and
//! `build_runtime_judge_fn_with_env_overrides_strict` — which disable the
//! process-environment fallback entirely; see "CR-01" below for why the
//! non-strict siblings this file used through Plan 11 were not sufficient.
//!
//! ## The GAP-1 corollary hole (Phase 47.4 Plan 11)
//!
//! The disk checks and the judge-closure build alone are not sufficient:
//! `bdev01`'s `model.roles.kanban_judge` resolves to `openrouter`, for which
//! that profile carries a valid key, so
//! `resolve_judge_model_and_endpoint_with_env_overrides` /
//! `build_runtime_judge_fn_with_env_overrides` both return `Ok` — this probe
//! would otherwise report `Success` for the exact profile that crashed a
//! real kanban worker with a 401, because the real agent turn resolves its
//! configured MAIN provider (`model.provider`), not the judge role.
//! `build_probe_setup` therefore also runs
//! `ironhermes_core::dispatch_gate::evaluate_profile_dispatch` — the
//! SAME shared predicate the CLI's hard pre-spawn dispatch gate (Plan 10)
//! uses — and short-circuits to `Failure` before any judge closure is built
//! or any network call is attempted whenever the MAIN provider has no
//! resolvable key.
//!
//! ## CR-01 (Phase 47.4 Plan 17): a fourth recurrence, closed
//!
//! This is the FOURTH occurrence of this bug class in this phase (GAP-1 →
//! GAP-7 → GAP-8 → CR-01) — each prior fix moved the false-confidence hole
//! one call site downstream instead of closing the class. CR-01's finding:
//! `build_probe_setup` resolved its judge cascade through the NON-strict
//! `ProviderResolver::build_with_env_overrides` (via
//! `resolve_judge_model_and_endpoint_with_env_overrides` /
//! `build_runtime_judge_fn_with_env_overrides`), which falls back to
//! `std::env::var` whenever the D-14 override map has no entry for a name.
//! `iron_hermes_ui`'s own server process loads the ROOT `~/.ironhermes/.env`
//! into its own process environment at startup (`main.rs:44-48`), so any
//! judge-tier key absent from the TARGET profile's `.env` but present in
//! root silently resolved — VERIFY could report `Success` for a profile
//! whose spawned worker (which runs under `.env_clear()` + only its own
//! `.env`) would crash at judge-build time moments later.
//!
//! **The decision this closes the class on:** VERIFY is a profile-scoped
//! claim, not an operator-interactive resolution. "Reachable" must be a
//! claim the check earns, and the only environment in which that claim is
//! meaningful is the scrubbed one the worker actually runs in — so
//! `build_probe_setup`'s two call sites now use the STRICT resolvers
//! exclusively. The permissive `_with_env_overrides` siblings remain
//! correct and are deliberately UNCHANGED: they serve the operator's own
//! interactive CLI resolution (`hermes kanban judge` / decompose flows),
//! where falling back to the process environment is the intended behavior,
//! not a bug.
//!
//! A comment-stripped source shape-lock —
//! `verify_probe_never_calls_the_process_env_falling_back_judge_resolvers`
//! (`tests/profile_verify_classification.rs`) — pins both call sites to the
//! strict siblings so a fifth recurrence cannot land silently.
//!
//! ## Module placement
//!
//! This probe lives in its own file rather than in `profile_api.rs` because
//! it is the only surface in this phase that performs network I/O and the
//! only one that depends on `ironhermes-cli` — keeping it separate makes the
//! dependency boundary, and the "this one does a real network call"
//! property, visible at the file level.
//!
//! ## Why the disk checks run inside `spawn_blocking` and the network call
//! does not
//!
//! Dioxus fullstack polls each websocket server-fn invocation inside a
//! per-connection task context where the ordinary in-place-blocking bridge
//! (`tokio::task`'s helper for calling blocking code from an async context)
//! is unsafe to reach for — it can panic there even though the surrounding
//! runtime is multi-threaded, and the runtime flavor alone does not predict
//! it. `verify_profile` avoids that hazard entirely by structure, not by a
//! bridge: the three synchronous disk checks AND the synchronous judge-fn
//! construction (no `.await` inside either) run inside `spawn_blocking`,
//! while the actual network probe call is awaited directly in this fn's own
//! async body once `spawn_blocking` has returned. No sync-to-async bridge is
//! ever needed.
//!
//! ## What this file never does
//!
//! No function in this file mutates the server process environment directly
//! — every key lookup is scoped through the D-14 override map instead. No
//! function shells out to a subprocess, and no function force-unwraps a
//! `Result`/`Option` — every error is propagated as a value. No mocked or
//! fixture judge closure is ever substituted for the real provider call in
//! this file's production code path — the network path is manual-UAT-only
//! by design (see `tests/profile_verify_classification.rs`'s module doc for
//! the full rationale). `verify_profile` is registered in `server/mod.rs`
//! behind `register_server_functions()`, which `require_auth` wraps
//! (`main.rs:119,162`) — it is never called from `profile_api.rs`'s
//! `list_profiles`, any dropdown open handler, or any per-row render (D-11:
//! the health dot claims CONFIGURED only; this probe is the wizard/detail-panel
//! action surface).

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::path::Path;

// `VerifyOutcome`/`VerifyReport` are part of `verify_profile`'s public
// signature — compiled unconditionally like every other `#[server]` fn's
// DTOs, so the `#[server]` macro's client-side HTTP stub has the types it
// needs on the wasm target too.
use crate::protocol::{VerifyOutcome, VerifyReport};

/// Phase 47.4 Plan 06 (D-09): the bound the UI's Timeout copy interpolates
/// (`no response after {N}s`). The whole judge call is wrapped in a bounded
/// async timeout against this bound so a slow or unreachable provider
/// cannot hang the request indefinitely.
#[cfg(feature = "server")]
pub(crate) const PROBE_TIMEOUT_SECONDS: u64 = 20;

/// Phase 47.4 Plan 06 (D-09): the three synchronous disk checks, returning a
/// `VerifyReport` whose `outcome` is a placeholder — this fn does zero
/// network I/O and never claims a network outcome. The caller
/// (`build_probe_setup` / `verify_profile`) always overwrites `outcome`
/// before the report reaches the wire.
///
/// `config_ok` requires BOTH that `config.yaml` parses AND that
/// `model.default` is non-empty — a file that exists but names no model
/// still crashes a worker at judge-build time. On unix, `env_ok` requires
/// the `.env` to exist AND to be mode 0600. `first_key` is a variable NAME,
/// never any part of a value.
#[cfg(feature = "server")]
pub(crate) fn run_disk_checks(name: &str) -> VerifyReport {
    let profile_dir = crate::server::profile_api::profile_dir_for(name);
    let dir_ok = profile_dir.is_dir();

    let config_path = profile_dir.join("config.yaml");
    let (config_ok, model_default) = if config_path.is_file() {
        match ironhermes_core::config::Config::load_from(&config_path) {
            Ok(cfg) if !cfg.model.default.is_empty() => (true, Some(cfg.model.default.clone())),
            _ => (false, None),
        }
    } else {
        (false, None)
    };

    let env_path = profile_dir.join(".env");
    let env_ok = env_path.is_file() && env_mode_is_0600(&env_path);

    let env_map = crate::server::profile_api::read_env_keys(&env_path).unwrap_or_default();
    let key_count = env_map.len();
    let mut key_names: Vec<&String> = env_map.keys().collect();
    key_names.sort();
    let first_key = key_names.first().map(|s| s.to_string());

    VerifyReport {
        dir_ok,
        config_ok,
        model_default,
        env_ok,
        key_count,
        first_key,
        outcome: VerifyOutcome::Failure {
            summary: String::new(),
        },
    }
}

/// Phase 47.4 Plan 06: `.env` existence + mode-0600 check on unix. On
/// non-unix targets (this crate is native-only server code, but the check is
/// still target-gated defensively) existence alone stands in.
#[cfg(all(feature = "server", unix))]
fn env_mode_is_0600(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777 == 0o600)
        .unwrap_or(false)
}

#[cfg(all(feature = "server", not(unix)))]
fn env_mode_is_0600(path: &Path) -> bool {
    path.is_file()
}

/// Phase 47.4 Plan 06 (D-09 / T-47.4-06-I1): strip any occurrence of
/// `secret` from `err`, then truncate to 200 characters on a character
/// boundary — mirrors the existing judge parser's bounded-disclosure preview
/// idiom (`raw_text.chars().take(200).collect()`,
/// `ironhermes-cli/src/kanban/commands.rs`). Never indexes by byte offset,
/// so it never panics on multibyte input.
#[cfg(feature = "server")]
pub(crate) fn summarize_probe_error(err: &str, secret: Option<&str>) -> String {
    let scrubbed = match secret {
        Some(value) if !value.is_empty() => err.replace(value, "<redacted>"),
        _ => err.to_string(),
    };
    scrubbed.chars().take(200).collect()
}

/// Phase 47.4 Plan 06 (D-09): classify a probe attempt into exactly the
/// three terminal `VerifyOutcome` variants. `timed_out` wins over any
/// concurrently-observed result — a provider that both times out AND
/// eventually errors is reported as `Timeout`, not `Failure`.
#[cfg(feature = "server")]
pub(crate) fn classify_probe_result(
    result: Result<(), String>,
    timed_out: bool,
    secret: Option<&str>,
) -> VerifyOutcome {
    if timed_out {
        return VerifyOutcome::Timeout {
            seconds: PROBE_TIMEOUT_SECONDS,
        };
    }
    match result {
        Ok(()) => VerifyOutcome::Success,
        Err(e) => VerifyOutcome::Failure {
            summary: summarize_probe_error(&e, secret),
        },
    }
}

/// Phase 47.4 Plan 06: the outcome of the synchronous, `spawn_blocking`-run
/// half of the probe. `ShortCircuit` carries a fully-classified report — the
/// disk checks failed, or the judge closure itself could not be built (a
/// true, useful `Failure`, not an infrastructure error) — and the caller
/// must not attempt a network call. `Ready` carries a `JudgeFn` closure
/// scoped to the target profile's own keys (D-14) plus the resolved API key
/// value, threaded through only so the outer async fn can scrub it from any
/// provider error text before it reaches the browser.
#[cfg(feature = "server")]
enum ProbeSetup {
    ShortCircuit(VerifyReport),
    Ready {
        report: VerifyReport,
        judge_fn: ironhermes_kanban::JudgeFn,
        secret: Option<String>,
    },
}

/// Phase 47.4 Plan 06 (D-09 / D-14): the synchronous half of `verify_profile`
/// — disk checks, then (only if they pass) building a judge closure scoped
/// to THIS profile's own `config.yaml` + `.env`, never the root profile's.
/// Runs entirely inside `spawn_blocking`; performs no network I/O itself
/// (building a judge closure resolves the provider cascade but does not call
/// it).
#[cfg(feature = "server")]
fn build_probe_setup(name: &str) -> Result<ProbeSetup, String> {
    let mut report = run_disk_checks(name);
    if !report.dir_ok || !report.config_ok {
        let summary = if !report.dir_ok {
            "profile directory does not exist".to_string()
        } else {
            "config.yaml is missing, malformed, or has no model.default set".to_string()
        };
        report.outcome = VerifyOutcome::Failure { summary };
        return Ok(ProbeSetup::ShortCircuit(report));
    }

    // Both the profile's own Config and its KanbanConfig must come from the
    // PROFILE, never the root — using the root config here would
    // reintroduce exactly the false-confidence bug D-09 exists to prevent.
    let profile_dir = crate::server::profile_api::profile_dir_for(name);
    let config_path = profile_dir.join("config.yaml");
    let profile_config = match ironhermes_core::config::Config::load_from(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            report.outcome = VerifyOutcome::Failure {
                summary: format!("load profile config.yaml: {e}"),
            };
            return Ok(ProbeSetup::ShortCircuit(report));
        }
    };
    // Phase 47.4 Plan 11 (GAP-1 corollary hole): `bdev01`'s
    // `model.roles.kanban_judge` resolves to `openrouter`, for which that
    // profile DOES have a key, so
    // `resolve_judge_model_and_endpoint_with_env_overrides` below returns
    // `Ok` and this probe would otherwise report `Success` for a profile
    // that cannot run a single real agent turn (the agent turn resolves its
    // MAIN provider, not the judge role). This gate check runs the SAME
    // shared predicate the CLI's hard pre-spawn dispatch gate uses
    // (`ironhermes_core::dispatch_gate::evaluate_profile_dispatch`,
    // Plan 10) and short-circuits to `Failure` BEFORE any judge closure is
    // built or any network call is attempted, whenever the profile's MAIN
    // provider has no resolvable key — independent of whether some OTHER
    // role happens to have one.
    if let ironhermes_core::dispatch_gate::DispatchDecision::Refuse { reason } =
        ironhermes_core::dispatch_gate::evaluate_profile_dispatch(name)
    {
        report.outcome = VerifyOutcome::Failure { summary: reason };
        return Ok(ProbeSetup::ShortCircuit(report));
    }

    let kanban_config = ironhermes_cli::kanban::commands::load_kanban_config(&profile_config);

    let env_path = profile_dir.join(".env");
    let overrides = match crate::server::profile_api::read_env_keys(&env_path) {
        Ok(m) => m,
        Err(e) => {
            report.outcome = VerifyOutcome::Failure {
                summary: format!("read profile .env: {e}"),
            };
            return Ok(ProbeSetup::ShortCircuit(report));
        }
    };

    // Resolved separately from the closure construction below purely so the
    // resolved key value is available to scrub from any later provider
    // error text — this does not perform a network call, it re-runs the
    // same cheap, local three-tier cascade the closure construction below
    // also runs.
    let secret =
        ironhermes_cli::kanban::commands::resolve_judge_model_and_endpoint_with_env_overrides_strict(
            &kanban_config,
            &profile_config,
            &overrides,
        )
        .ok()
        .and_then(|(endpoint, _model)| endpoint.api_key.clone());

    match ironhermes_cli::kanban::commands::build_runtime_judge_fn_with_env_overrides_strict(
        &kanban_config,
        &profile_config,
        &overrides,
    ) {
        Ok(judge_fn) => Ok(ProbeSetup::Ready {
            report,
            judge_fn,
            secret,
        }),
        Err(e) => {
            // A build error (including the fail-closed no-API-key bail) is a
            // true, useful Failure result, not an infrastructure error.
            report.outcome = classify_probe_result(Err(e.to_string()), false, secret.as_deref());
            Ok(ProbeSetup::ShortCircuit(report))
        }
    }
}

/// Phase 47.4 Plan 06 (D-09): a tiny fixed probe request. Title/body/worker
/// output are short static strings chosen so any verdict is acceptable —
/// the probe cares whether the provider answered at all, not what it said.
/// No operator data, no profile content, and no key material is ever sent
/// as the request payload.
#[cfg(feature = "server")]
fn probe_judge_request() -> ironhermes_kanban::JudgeRequest {
    ironhermes_kanban::JudgeRequest {
        task_id: "profile-verify-probe".to_string(),
        title: "Profile verification probe".to_string(),
        body: "Respond with any verdict — this call only checks reachability.".to_string(),
        worker_turn_output: "ok".to_string(),
        turn: 1,
    }
}

/// Phase 47.4 Plan 06 (D-09 / D-14): verify a kanban worker profile with a
/// real, time-bounded, profile-scoped judge probe. Performs the three D-11
/// disk checks PLUS an actual attempt to build the judge closure and issue a
/// minimal provider call — a configuration-presence check alone can never
/// report `Success` here. A profile whose `.env` is empty or invalid, or
/// whose keys are absent from the D-14 override map, reports `Failure`, even
/// if the server process's own environment carries a valid root key.
///
/// Phase 47.4 Plan 17 (CR-01): this guarantee is upheld by
/// `build_probe_setup` resolving the judge cascade exclusively through
/// `resolve_judge_model_and_endpoint_with_env_overrides_strict` and
/// `build_runtime_judge_fn_with_env_overrides_strict` — the process-env
/// fallback is disabled, so `overrides` (the target profile's own `.env`) is
/// the entire world. See this file's module doc, "CR-01", for the finding
/// this closed and why VERIFY is a profile-scoped claim, not an
/// operator-interactive resolution.
#[server]
pub async fn verify_profile(name: String) -> Result<VerifyReport, ServerFnError> {
    #[cfg(feature = "server")]
    {
        ironhermes_core::profile::validate_profile_name(&name)
            .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;

        let name_for_blocking = name.clone();
        let setup = tokio::task::spawn_blocking(move || build_probe_setup(&name_for_blocking))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;

        match setup {
            ProbeSetup::ShortCircuit(report) => Ok(report),
            ProbeSetup::Ready {
                mut report,
                judge_fn,
                secret,
            } => {
                let probe_req = probe_judge_request();
                let (result, timed_out) = match tokio::time::timeout(
                    std::time::Duration::from_secs(PROBE_TIMEOUT_SECONDS),
                    judge_fn(probe_req),
                )
                .await
                {
                    Ok(Ok(_output)) => (Ok(()), false),
                    Ok(Err(e)) => (Err(e.to_string()), false),
                    Err(_elapsed) => (Err("probe timed out".to_string()), true),
                };
                report.outcome = classify_probe_result(result, timed_out, secret.as_deref());
                Ok(report)
            }
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "verify_profile unavailable without `server` feature",
        ))
    }
}

/// Phase 47.4 Plan 06 Task 1 (D-09): unit tests for the pure classification
/// layer (`classify_probe_result` / `summarize_probe_error`) that need no
/// filesystem fixture or `IRONHERMES_HOME` override. The disk-fixture tests
/// for `run_disk_checks` (which DO need a tempdir + env override) are Task
/// 3's dedicated fixture-directory suite, added alongside
/// `tests/profile_verify_classification.rs` — kept out of this task's
/// commit so this file's production-code-only invariants (no process-env
/// mutation) hold at every intermediate task boundary, not just the final
/// one.
#[cfg(all(test, feature = "server"))]
mod profile_verify_classification_tests {
    use super::*;

    #[test]
    fn ok_result_classifies_as_success() {
        let outcome = classify_probe_result(Ok(()), false, None);
        assert_eq!(outcome, VerifyOutcome::Success);
    }

    #[test]
    fn err_result_classifies_as_failure_with_bounded_summary() {
        let outcome = classify_probe_result(Err("provider rejected the request".to_string()), false, None);
        match outcome {
            VerifyOutcome::Failure { summary } => {
                assert_eq!(summary, "provider rejected the request");
            }
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn timed_out_classifies_as_timeout_with_bound() {
        let outcome = classify_probe_result(Ok(()), true, None);
        assert_eq!(
            outcome,
            VerifyOutcome::Timeout {
                seconds: PROBE_TIMEOUT_SECONDS
            }
        );
    }

    #[test]
    fn timeout_precedence_wins_over_a_concurrent_error_result() {
        // Precedence case: timed_out is true AND the result is an Err —
        // timeout must win, not Failure.
        let outcome = classify_probe_result(Err("connection reset".to_string()), true, None);
        assert_eq!(
            outcome,
            VerifyOutcome::Timeout {
                seconds: PROBE_TIMEOUT_SECONDS
            }
        );
    }

    #[test]
    fn error_summary_truncates_at_200_chars_on_a_char_boundary() {
        // A multibyte string (each char is 3 bytes) longer than the 200-char
        // bound — must not panic and must truncate on a char boundary.
        let long_err: String = "日".repeat(500);
        let summary = summarize_probe_error(&long_err, None);
        assert_eq!(summary.chars().count(), 200);
    }

    #[test]
    fn error_summary_never_panics_on_multibyte_input_longer_than_bound() {
        let long_err: String = "𝔘".repeat(1000); // 4-byte chars
        let _summary = summarize_probe_error(&long_err, None);
        // Reaching this line without panicking is the assertion.
    }

    #[test]
    fn error_summary_removes_secret_substring_before_truncating() {
        let secret = "sk-VeryUniqueSecretXyz123"; // secret-scan:allow
        let err = format!("authentication failed for key {secret}: invalid");
        let summary = summarize_probe_error(&err, Some(secret));
        assert!(
            !summary.contains(secret),
            "summarize_probe_error must scrub the secret before truncating"
        );
    }

    #[test]
    fn error_summary_short_input_passes_through_minus_secret() {
        let summary = summarize_probe_error("short error", None);
        assert_eq!(summary, "short error");
    }

    #[test]
    fn error_summary_none_secret_does_not_alter_error_text() {
        let summary = summarize_probe_error("connection refused", None);
        assert_eq!(summary, "connection refused");
    }
}

/// Phase 47.4 Plan 06 Task 3 (D-09): real fixture-directory tests for
/// `run_disk_checks`, covering every permutation from the plan's
/// `<behavior>` block. Separated from `profile_verify_classification_tests`
/// above purely to keep the `IRONHERMES_HOME`-mutating fixture tests
/// grouped together — both modules run under the same
/// `--test-threads=1` requirement.
///
/// Run with (mutates process-global `IRONHERMES_HOME`, so `--test-threads=1`
/// is required):
///   `cargo nextest run -p iron_hermes_ui --features server profile_verify_disk_checks_tests --test-threads=1`
#[cfg(all(test, feature = "server"))]
mod profile_verify_disk_checks_tests {
    use super::*;
    use ironhermes_core::config::Config;
    use std::fs;

    /// RAII guard that sets an env var and restores the previous value on
    /// drop. Duplicated verbatim from `profile_api.rs`'s test modules (and
    /// originally `ironhermes-kanban/src/paths.rs:449-475`) — each
    /// `#[cfg(test)]` module is its own namespace, so this is the
    /// established "duplicate the guard" precedent, not drift.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    fn set_mode_0600(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).expect("set_permissions 0600");
    }

    fn scaffold_profile(name: &str, model_default: &str) {
        let profile_dir = crate::server::profile_api::profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut cfg = Config::default();
        cfg.model.provider = "anthropic".to_string();
        cfg.model.default = model_default.to_string();
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");

        let env_path = profile_dir.join(".env");
        fs::write(&env_path, "ANTHROPIC_API_KEY=sk-fixture-value\n").expect("write .env");
        set_mode_0600(&env_path);
    }

    #[test]
    fn fully_scaffolded_profile_reports_all_disk_checks_true() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("fully-scaffolded", "claude-3-opus");

        let report = run_disk_checks("fully-scaffolded");
        assert!(report.dir_ok);
        assert!(report.config_ok);
        assert_eq!(report.model_default, Some("claude-3-opus".to_string()));
        assert!(report.env_ok);
        assert_eq!(report.key_count, 1);
        assert_eq!(report.first_key, Some("ANTHROPIC_API_KEY".to_string()));
    }

    #[test]
    fn missing_profile_dir_yields_dir_ok_false_and_no_claimed_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // Deliberately never created.

        let report = run_disk_checks("never-created");
        assert!(!report.dir_ok);
        assert!(!report.config_ok);
        assert!(!report.env_ok);
    }

    #[test]
    fn missing_config_yaml_yields_config_ok_false_and_no_model_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = crate::server::profile_api::profile_dir_for("no-config");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        // Deliberately no config.yaml written.

        let report = run_disk_checks("no-config");
        assert!(report.dir_ok);
        assert!(!report.config_ok);
        assert_eq!(report.model_default, None);
    }

    #[test]
    fn config_yaml_with_empty_model_default_yields_config_ok_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = crate::server::profile_api::profile_dir_for("empty-model-default");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let mut cfg = Config::default();
        cfg.model.default = String::new();
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");

        let report = run_disk_checks("empty-model-default");
        assert!(
            !report.config_ok,
            "presence of config.yaml alone must not be enough — an empty model.default \
             still crashes a worker at judge-build time"
        );
    }

    #[cfg(unix)]
    #[test]
    fn env_file_at_wrong_mode_yields_env_ok_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = crate::server::profile_api::profile_dir_for("wrong-mode");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let env_path = profile_dir.join(".env");
        fs::write(&env_path, "ANTHROPIC_API_KEY=sk-abc\n").expect("write .env");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&env_path).expect("metadata").permissions();
        perms.set_mode(0o644); // wrong mode, deliberately not 0600
        fs::set_permissions(&env_path, perms).expect("set_permissions 0644");

        let report = run_disk_checks("wrong-mode");
        assert!(
            !report.env_ok,
            "a .env at a mode other than 0600 must report env_ok: false"
        );
    }

    #[test]
    fn first_key_is_a_variable_name_never_part_of_a_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = crate::server::profile_api::profile_dir_for("name-not-value");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let env_path = profile_dir.join(".env");
        let secret_value = "sk-Z9f3kQ7xN2mP8wL4vR6tY1uJ5hG0bC3eA"; // secret-scan:allow
        fs::write(&env_path, format!("ANTHROPIC_API_KEY={secret_value}\n")).expect("write .env");
        set_mode_0600(&env_path);

        let report = run_disk_checks("name-not-value");
        let first_key = report.first_key.expect("first_key present");
        assert_eq!(first_key, "ANTHROPIC_API_KEY");
        assert!(!first_key.contains(secret_value));
    }

    // -------------------------------------------------------------------
    // build_probe_setup — the GAP-1 corollary gate (Phase 47.4 Plan 11,
    // Task 3): a main-provider key miss must short-circuit to Failure even
    // when a DIFFERENT role (kanban_judge) resolves fine.
    // -------------------------------------------------------------------

    #[test]
    fn verify_short_circuits_to_failure_when_main_provider_key_missing_despite_judge_role() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let name = "bdev01-verify-shape";
        let profile_dir = crate::server::profile_api::profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut cfg = Config::default();
        // The exact bdev01 shape: MAIN provider is moonshot (no key below),
        // but model.roles.kanban_judge points at openrouter, for which a
        // key IS present — the judge cascade alone would resolve `Ok`.
        cfg.model.provider = "moonshot".to_string();
        cfg.model.default = "moonshot-v1".to_string();
        cfg.providers.insert(
            "moonshot".to_string(),
            ironhermes_core::config::ProviderConfig {
                api_key_env: Some("MOONSHOT_API_KEY".to_string()),
                ..Default::default()
            },
        );
        cfg.model.roles.insert(
            "kanban_judge".to_string(),
            ironhermes_core::config::ModelRoleConfig {
                provider: "openrouter".to_string(),
                model: None,
            },
        );
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");

        let env_path = profile_dir.join(".env");
        // High-entropy fixture value so a partial-match leak check is
        // meaningful — no MOONSHOT_API_KEY at all.
        let openrouter_secret = "sk-OrHighEntropyFixtureValueQ7xN2mP8wL4vR6tY1u"; // secret-scan:allow
        fs::write(&env_path, format!("OPENROUTER_API_KEY={openrouter_secret}\n"))
            .expect("write .env");
        set_mode_0600(&env_path);

        let setup = build_probe_setup(name).expect("build_probe_setup should not error");
        match setup {
            ProbeSetup::ShortCircuit(report) => match report.outcome {
                VerifyOutcome::Failure { summary } => {
                    assert!(
                        summary.contains("moonshot"),
                        "failure summary must name the missing provider, got: {summary}"
                    );
                    assert!(
                        !summary.contains(openrouter_secret),
                        "failure summary must never contain a key value"
                    );
                }
                other => panic!("expected Failure, got {other:?}"),
            },
            ProbeSetup::Ready { .. } => panic!(
                "expected ShortCircuit — the main provider has no key despite the judge role \
                 resolving via openrouter, so no judge closure (and no network call) should \
                 ever be built"
            ),
        }
    }

    // -------------------------------------------------------------------
    // build_probe_setup — CR-01 (Phase 47.4 Plan 17): a judge-tier key
    // present ONLY in the server process environment (standing in for the
    // ROOT `.env` `main.rs` loads at startup) must never make the probe
    // report `Ready`/`Success` — that is exactly the answer the spawned
    // worker's scrubbed environment (`.env_clear()` + profile `.env`) can
    // never reproduce, per `gate_ignores_process_env_because_the_worker_is_
    // scrubbed` (`ironhermes-kanban/tests/dispatch_gate_loop.rs`).
    // -------------------------------------------------------------------

    #[test]
    fn verify_reports_failure_when_judge_role_key_exists_only_in_the_server_process_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let name = "cr01-process-env-only";
        let profile_dir = crate::server::profile_api::profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut cfg = Config::default();
        // MAIN provider is anthropic — the profile's own `.env` carries a
        // key for it, so the D-11 dispatch gate ALLOWS this profile (the
        // MAIN provider resolves fine). The judge role points at a
        // DIFFERENT provider (openrouter) whose key exists only in the
        // TEST process environment below, standing in for the root `.env`
        // the server loads at startup (`main.rs:44-48`).
        cfg.model.provider = "anthropic".to_string();
        cfg.model.default = "claude-3-opus".to_string();
        cfg.model.roles.insert(
            "kanban_judge".to_string(),
            ironhermes_core::config::ModelRoleConfig {
                provider: "openrouter".to_string(),
                model: None,
            },
        );
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");

        let env_path = profile_dir.join(".env");
        // High-entropy fixture value, distinct from the process-env value
        // below — a partial-match leak check between the two is meaningful.
        let anthropic_secret = "sk-AnthropicHighEntropyFixtureValueM3nQ8xR1uJ5h"; // secret-scan:allow
        fs::write(&env_path, format!("ANTHROPIC_API_KEY={anthropic_secret}\n"))
            .expect("write .env");
        set_mode_0600(&env_path);

        // The hostile condition, made explicit rather than assumed away —
        // the value a root `.env` loaded into the server process would
        // supply, which the spawned worker's scrubbed environment never
        // sees.
        let openrouter_process_secret = "sk-OpenrouterProcessEnvOnlyFixtureP9wY2mK6vT4b"; // secret-scan:allow
        let _key_guard = ScopedEnv::set("OPENROUTER_API_KEY", openrouter_process_secret);

        let setup = build_probe_setup(name).expect("build_probe_setup should not error");
        match setup {
            ProbeSetup::ShortCircuit(report) => match report.outcome {
                VerifyOutcome::Failure { summary } => {
                    assert!(
                        !summary.contains(anthropic_secret),
                        "failure summary must never contain the profile's own key value"
                    );
                    assert!(
                        !summary.contains(openrouter_process_secret),
                        "failure summary must never contain the process-env-only key value"
                    );
                }
                other => panic!("expected Failure, got {other:?}"),
            },
            ProbeSetup::Ready { .. } => panic!(
                "CR-01 regression: build_probe_setup returned Ready — the judge-role key was \
                 resolved from the server PROCESS environment (simulating root's `.env`), which \
                 the spawned worker's scrubbed environment could never reproduce. Before the \
                 CR-01 fix this branch is what actually executes: build_probe_setup's two call \
                 sites used the non-strict resolvers (`resolve_judge_model_and_endpoint_with_env_\
                 overrides` / `build_runtime_judge_fn_with_env_overrides`), which fall back to \
                 `std::env::var` and so pick up OPENROUTER_API_KEY from this test's own process \
                 environment even though the profile's `.env` never carries it."
            ),
        }
    }

    #[test]
    fn verify_stays_ready_when_the_judge_role_key_is_in_the_profiles_own_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let name = "cr01-profile-owns-both-keys";
        let profile_dir = crate::server::profile_api::profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        // Same config shape as the CR-01 regression test above, but the
        // profile's own `.env` carries BOTH keys this time — the D-09
        // non-regression guard: the fix must not convert this legitimate
        // Success path into a Failure. `uatgood` (47.4-UAT.md test 2 case
        // 1, passed under the already-strict dispatch gate) is this test's
        // live analogue.
        let mut cfg = Config::default();
        cfg.model.provider = "anthropic".to_string();
        cfg.model.default = "claude-3-opus".to_string();
        cfg.model.roles.insert(
            "kanban_judge".to_string(),
            ironhermes_core::config::ModelRoleConfig {
                provider: "openrouter".to_string(),
                model: None,
            },
        );
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");

        let env_path = profile_dir.join(".env");
        fs::write(
            &env_path,
            "ANTHROPIC_API_KEY=sk-fixture-anthropic-both-keys\n\
             OPENROUTER_API_KEY=sk-fixture-openrouter-both-keys\n",
        )
        .expect("write .env");
        set_mode_0600(&env_path);

        // No network call is made — this test asserts only which
        // `ProbeSetup` variant was returned.
        let setup = build_probe_setup(name).expect("build_probe_setup should not error");
        match setup {
            ProbeSetup::Ready { .. } => {}
            ProbeSetup::ShortCircuit(report) => panic!(
                "over-refusal regression: expected Ready — both the main provider (anthropic) \
                 and the judge role (openrouter) resolve from the profile's OWN `.env`, with \
                 nothing required from the process environment. Got ShortCircuit with outcome: \
                 {:?}",
                report.outcome
            ),
        }
    }
}
