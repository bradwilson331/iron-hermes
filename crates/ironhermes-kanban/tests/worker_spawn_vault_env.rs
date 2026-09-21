//! Phase 51 Plan 07 (D-11) — `build_kanban_worker_env`'s vault-variable emission
//! contract: `vault_vars_are_emitted_not_passed_through`.
//!
//! Two halves of one claim:
//! 1. When a [`WorkerVaultBootstrap`] is supplied, both variables appear in the
//!    built env map with the expected values.
//! 2. When NO bootstrap is supplied (`None`), and an AMBIENT value for either
//!    variable name happens to be set in the parent process's own environment,
//!    NEITHER reaches the built map — proving the two variables are explicitly
//!    EMITTED by this function (like every other kanban var), never a
//!    pass-through of an ambient value the way [`SAFE_SYSTEM_VARS`] works. A
//!    credential variable accidentally landing in the pass-through allowlist
//!    would let an attacker-controlled ambient value reach the worker (T-51-36).

use ironhermes_kanban::types::{Task, TaskRun};
use ironhermes_kanban::worker_spawn::{
    IRONHERMES_KANBAN_VAULT_SOCKET_ENV, IRONHERMES_KANBAN_VAULT_TOKEN_ENV, SAFE_SYSTEM_VARS,
    WorkerVaultBootstrap, build_kanban_worker_env, resolve_worker_bin,
};
use secrecy::SecretString;

// Guards concurrent mutation of this process's own environment across the two
// tests below (this crate's documented env-race trap — see
// `worker_bin_resolution.rs`'s identical precedent).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Minimal `Task` fixture — mirrors `worker_bin_resolution.rs`'s own fixture
/// shape exactly, so both files stay structurally identical if `Task` gains a
/// field.
fn make_task() -> Task {
    Task {
        id: "test-task-vault".to_string(),
        title: "Test task".to_string(),
        body: None,
        assignee: "dev".to_string(),
        status: "ready".to_string(),
        priority: 0,
        tenant: None,
        workspace: None,
        skills: None,
        idempotency_key: None,
        claim_lock: None,
        claim_expires: None,
        current_run_id: None,
        consecutive_failures: 0,
        max_retries: None,
        max_runtime_seconds: None,
        scheduled_at: None,
        workflow_template_id: None,
        current_step_key: None,
        created_by: None,
        created_at: 0.0,
        started_at: None,
        ended_at: None,
        goal_mode: false,
        goal_max_turns: 0,
        goal_turns_used: 0,
        goal_toolset: None,
        output_path: None,
    }
}

fn make_run() -> TaskRun {
    TaskRun {
        id: "run-vault-1".to_string(),
        task_id: "test-task-vault".to_string(),
        claim_lock: "lock-vault".to_string(),
        claim_pid: None,
        started_at: 0.0,
        ended_at: None,
        outcome: None,
        summary: None,
        metadata: None,
        error: None,
        log_path: None,
    }
}

fn env_map_get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Half 1: `Some(bootstrap)` → both variables appear, with the expected values.
#[test]
fn vault_vars_are_emitted_when_bootstrap_is_supplied() {
    let _guard = ENV_LOCK.lock().unwrap();
    let task = make_task();
    let run = make_run();

    let bootstrap = WorkerVaultBootstrap::new(
        SecretString::from("minted-token-xyz".to_string()),
        "/tmp/ihvc-test.sock",
    );

    let env = build_kanban_worker_env(&task, &run, "/tmp/ws", "default", Some(&bootstrap));

    assert_eq!(
        env_map_get(&env, IRONHERMES_KANBAN_VAULT_TOKEN_ENV),
        Some("minted-token-xyz"),
        "the token variable must be emitted with the bootstrap's token value"
    );
    assert_eq!(
        env_map_get(&env, IRONHERMES_KANBAN_VAULT_SOCKET_ENV),
        Some("/tmp/ihvc-test.sock"),
        "the socket variable must be emitted with the bootstrap's socket path"
    );
}

/// Half 2: `None` + an ambient parent-process value for both variable names →
/// neither reaches the built map. Proves emission, not pass-through.
#[test]
fn ambient_vault_vars_do_not_leak_through_when_no_bootstrap_is_supplied() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: guarded by ENV_LOCK above; restored to absent before returning.
    unsafe {
        std::env::set_var(IRONHERMES_KANBAN_VAULT_TOKEN_ENV, "attacker-controlled-ambient-token");
        std::env::set_var(IRONHERMES_KANBAN_VAULT_SOCKET_ENV, "/tmp/attacker-controlled.sock");
    }

    let task = make_task();
    let run = make_run();
    let env = build_kanban_worker_env(&task, &run, "/tmp/ws", "default", None);

    unsafe {
        std::env::remove_var(IRONHERMES_KANBAN_VAULT_TOKEN_ENV);
        std::env::remove_var(IRONHERMES_KANBAN_VAULT_SOCKET_ENV);
    }

    assert_eq!(
        env_map_get(&env, IRONHERMES_KANBAN_VAULT_TOKEN_ENV),
        None,
        "an ambient parent-process value for the token variable must NOT reach \
         the child when no bootstrap is supplied — this variable is emitted, \
         never passed through"
    );
    assert_eq!(
        env_map_get(&env, IRONHERMES_KANBAN_VAULT_SOCKET_ENV),
        None,
        "an ambient parent-process value for the socket variable must NOT reach \
         the child when no bootstrap is supplied"
    );
}

/// `None` also produces a map byte-identical to today's shape (no vault keys at
/// all), the same claim `worker_without_vault_vars_uses_dotenv_exactly_as_today`
/// makes at the CLI-integration level (`ironhermes-cli/tests/worker_vault_bootstrap.rs`)
/// — this is the producer-side half of that same guarantee.
#[test]
fn none_bootstrap_emits_neither_vault_key_at_all() {
    let _guard = ENV_LOCK.lock().unwrap();
    let task = make_task();
    let run = make_run();
    let env = build_kanban_worker_env(&task, &run, "/tmp/ws", "default", None);

    assert!(
        env_map_get(&env, IRONHERMES_KANBAN_VAULT_TOKEN_ENV).is_none(),
        "None must not emit the token variable at all"
    );
    assert!(
        env_map_get(&env, IRONHERMES_KANBAN_VAULT_SOCKET_ENV).is_none(),
        "None must not emit the socket variable at all"
    );
}

// ---------------------------------------------------------------------------
// WR-06 (Phase 51 Plan 16): IRONHERMES_WORKER_BIN / IRONHERMES_ROOT_HOME move
// from ambient pass-through (SAFE_SYSTEM_VARS) to explicit, computed emission
// — the same category shift the two vault variables above already went
// through. Unlike the vault token/socket (crypto material with zero relation
// to any ambient variable name), `resolve_worker_bin()` and
// `ironhermes_core::get_root_hermes_home()` are THEMSELVES ambient-aware —
// each already reads the identically-named env var as its primary source, so
// when a caller's ambient value is legitimately present, the emitted value
// necessarily still equals it (a compromised dispatcher is not a new
// privilege boundary — see the plan's own WR-06 text). What genuinely
// changes, and what these tests pin, is: (1) the value no longer rides the
// generic pass-through list — a caller can no longer inject it merely by
// matching a name in that literal — and (2) the child ALWAYS receives an
// explicit, single, authoritative value, even when the ambient environment
// has nothing under that name at all, which the old list-based forward could
// never do (an absent ambient value forwarded nothing).
// ---------------------------------------------------------------------------

/// Guards the two tests below, which mutate this process's own
/// IRONHERMES_WORKER_BIN / IRONHERMES_ROOT_HOME env vars — separate from
/// ENV_LOCK above (which only guards the vault var names) so the two guard
/// pairs never cross-block unrelated tests in this file unnecessarily.
static STEERING_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Half 1 of the WR-06 property: `IRONHERMES_WORKER_BIN` is no longer a
/// member of the ambient pass-through allowlist, and — with NO ambient value
/// present at all — `build_kanban_worker_env` still emits it explicitly with
/// `resolve_worker_bin()`'s own fallback value. Before this fix, an absent
/// ambient value meant the old list-based loop pushed nothing at all; the
/// key was simply missing from the child's env.
#[test]
fn worker_bin_no_longer_rides_the_ambient_allowlist() {
    let _guard = STEERING_ENV_LOCK.lock().unwrap();
    // SAFETY: guarded by STEERING_ENV_LOCK; restored to absent before returning.
    unsafe {
        std::env::remove_var("IRONHERMES_WORKER_BIN");
    }

    assert!(
        !SAFE_SYSTEM_VARS.contains(&"IRONHERMES_WORKER_BIN"),
        "IRONHERMES_WORKER_BIN must not ride the ambient pass-through allowlist — \
         it selects which binary is exec'd, the same category as a credential"
    );

    let task = make_task();
    let run = make_run();
    let env = build_kanban_worker_env(&task, &run, "/tmp/ws", "default", None);

    assert_eq!(
        env_map_get(&env, "IRONHERMES_WORKER_BIN"),
        Some(resolve_worker_bin().as_str()),
        "with no ambient value present, build_kanban_worker_env must still \
         explicitly emit IRONHERMES_WORKER_BIN using resolve_worker_bin()'s \
         own fallback — the old pass-through-only mechanism would have \
         omitted the key entirely here"
    );

    // Now plant a hostile ambient value: the emitted value must still track
    // resolve_worker_bin()'s own (ambient-aware) resolution exactly, proving
    // there is exactly one source of truth for this value — never a second,
    // uncoordinated raw copy riding alongside it.
    unsafe {
        std::env::set_var("IRONHERMES_WORKER_BIN", "attacker-controlled-worker-bin");
    }
    let env_with_ambient = build_kanban_worker_env(&task, &run, "/tmp/ws", "default", None);
    let expected_with_ambient = resolve_worker_bin();
    let occurrences: Vec<&str> = env_with_ambient
        .iter()
        .filter(|(k, _)| k == "IRONHERMES_WORKER_BIN")
        .map(|(_, v)| v.as_str())
        .collect();
    unsafe {
        std::env::remove_var("IRONHERMES_WORKER_BIN");
    }
    assert_eq!(
        occurrences,
        vec![expected_with_ambient.as_str()],
        "IRONHERMES_WORKER_BIN must appear exactly once, sourced only from \
         resolve_worker_bin() — never both forwarded raw from the allowlist \
         AND emitted explicitly"
    );
}

/// Half 2 of the WR-06 property: `IRONHERMES_ROOT_HOME` is no longer a
/// member of the ambient pass-through allowlist, and — with NO ambient value
/// present at all — `build_kanban_worker_env` still emits it explicitly with
/// `ironhermes_core::get_root_hermes_home()`'s own fallback value. Mirrors
/// `worker_bin_no_longer_rides_the_ambient_allowlist` exactly.
#[test]
fn root_home_no_longer_rides_the_ambient_allowlist() {
    let _guard = STEERING_ENV_LOCK.lock().unwrap();
    // SAFETY: guarded by STEERING_ENV_LOCK; restored to absent before returning.
    unsafe {
        std::env::remove_var("IRONHERMES_ROOT_HOME");
    }

    assert!(
        !SAFE_SYSTEM_VARS.contains(&"IRONHERMES_ROOT_HOME"),
        "IRONHERMES_ROOT_HOME must not ride the ambient pass-through allowlist — \
         it selects which vault data dir is opened, the same category as a credential"
    );

    let task = make_task();
    let run = make_run();
    let env = build_kanban_worker_env(&task, &run, "/tmp/ws", "default", None);

    let expected = ironhermes_core::get_root_hermes_home()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        env_map_get(&env, "IRONHERMES_ROOT_HOME"),
        Some(expected.as_str()),
        "with no ambient value present, build_kanban_worker_env must still \
         explicitly emit IRONHERMES_ROOT_HOME using get_root_hermes_home()'s \
         own fallback — the old pass-through-only mechanism would have \
         omitted the key entirely here"
    );

    // Now plant a hostile ambient value: the emitted value must still track
    // get_root_hermes_home()'s own (ambient-aware) resolution exactly,
    // proving there is exactly one source of truth for this value.
    unsafe {
        std::env::set_var("IRONHERMES_ROOT_HOME", "/tmp/attacker-controlled-root");
    }
    let env_with_ambient = build_kanban_worker_env(&task, &run, "/tmp/ws", "default", None);
    let expected_with_ambient = ironhermes_core::get_root_hermes_home()
        .to_string_lossy()
        .into_owned();
    let occurrences: Vec<&str> = env_with_ambient
        .iter()
        .filter(|(k, _)| k == "IRONHERMES_ROOT_HOME")
        .map(|(_, v)| v.as_str())
        .collect();
    unsafe {
        std::env::remove_var("IRONHERMES_ROOT_HOME");
    }
    assert_eq!(
        occurrences,
        vec![expected_with_ambient.as_str()],
        "IRONHERMES_ROOT_HOME must appear exactly once, sourced only from \
         get_root_hermes_home() — never both forwarded raw from the allowlist \
         AND emitted explicitly"
    );
}

// ---------------------------------------------------------------------------
// WR-07 / T18 (Phase 51 Plan 16): tie docs/CONFIGURATION.md's "Kanban Worker
// Profile Credentials" section to this constant so the two cannot drift
// apart silently — a change to either length OR membership must turn this
// test red before it can ship.
// ---------------------------------------------------------------------------

/// Asserts BOTH the length and the exact membership set of `SAFE_SYSTEM_VARS`
/// — a length-only assertion would stay green if one entry were swapped for
/// another, and the exact membership IS the security claim
/// `docs/CONFIGURATION.md` makes to the operator. If this test fails, update
/// `docs/CONFIGURATION.md`'s "Kanban Worker Profile Credentials" section (the
/// allowlist-size sentence and the member list) to match the new constant —
/// do NOT just update the `EXPECTED` array below without also updating that
/// doc section; the whole point of this test is that the two move together.
#[test]
fn safe_system_vars_matches_the_operator_doc_membership_claim() {
    const EXPECTED: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LANG",
        "TERM",
        "RUST_LOG",
        "IRONHERMES_HOME",
    ];
    assert_eq!(
        SAFE_SYSTEM_VARS.len(),
        EXPECTED.len(),
        "SAFE_SYSTEM_VARS is now {} entries. docs/CONFIGURATION.md's \"Kanban \
         Worker Profile Credentials\" section states this exact count as the \
         worker credential boundary and MUST be updated to match — along with \
         the member list, which this test's sibling assertion also checks.",
        SAFE_SYSTEM_VARS.len()
    );
    assert_eq!(
        SAFE_SYSTEM_VARS, EXPECTED,
        "SAFE_SYSTEM_VARS membership changed. docs/CONFIGURATION.md's \"Kanban \
         Worker Profile Credentials\" section names these exact variables as \
         the worker credential boundary — the exact membership IS the security \
         claim, and the doc MUST be updated to match this constant."
    );
}
