//! Proves `ironhermes_core::profile_credentials::decide_spawn_credential` — the single,
//! shared credential decision every spawn surface (kanban worker AND UI bot dispatch)
//! asks before creating a child process for a profile (Phase 51 Plan 10, D-07/D-11/D-14/D-15).
//!
//! `bot_credential_is_readable_over_the_socket` is the tracer assertion: it proves the
//! credential the decision mints actually WORKS by reading it back through the real
//! `ironhermes_vault::read_profile_credential` client — never merely that a token-shaped
//! string came back. It was written FIRST and watched fail (RED) before the vault arm of
//! `decide_spawn_credential` was implemented; see `51-10-SUMMARY.md` for the recorded
//! RED/GREEN observations.
//!
//! `decision_compiles_and_refuses_when_the_feature_is_absent` is compiled and run ONLY on
//! the DEFAULT feature set (no `rusty-vault`) — the counterpart to the other seven tests,
//! which require a real vault and so live under `#[cfg(feature = "rusty-vault")]`.
//!
//! # KNOWN PRODUCTION HAZARD discovered while building this file's socket-read tests
//! (Phase 51 Plan 10) — see `rusty_vault_tests`'s "Salt-stabilization gate" doc for the
//! full mechanism and evidence.
//!
//! A freshly `ironhermes vault init`-ed vault has no persisted token salt.
//! `rusty_vault::TokenStore::new()` performs an unsynchronized check-then-write on that
//! salt on EVERY `Core` boot, so several Cores opened in quick succession after `init`
//! can each independently generate their own salt before one write durably wins. A
//! long-lived host (the gateway's `DispatcherContext`, or this plan's bot-spawn host)
//! that happens to open WHILE that race is unresolved caches the losing salt for its
//! entire process lifetime and never re-reads — **every vault-backed dispatch then
//! fails with `token_invalid` until the process is restarted.** This is a real,
//! narrow, first-boot ordering hazard for an operator who runs `vault init` and starts
//! the gateway close together; it is out of scope for this plan to fix (the defect is
//! in `rusty_vault`/`RustyVaultStore::open`, not in any file this plan touches) but it
//! must be tracked, not silently masked by a lucky-timed test.

use ironhermes_core::config::Config;

const PROFILE: &str = "spawn-credential-decision";
const PROVIDER: &str = "spawncreddecisionprovider";

/// Writes `profiles_root/PROFILE/config.yaml` (+ optional `.env`), mirroring
/// `dispatch_gate_loop.rs`'s own `write_profile` helper exactly.
fn write_profile(root: &std::path::Path, config: &Config, env: Option<&str>) {
    let dir = root.join(PROFILE);
    std::fs::create_dir_all(&dir).expect("mkdir profile dir");
    config
        .save_to(&dir.join("config.yaml"))
        .expect("save_to config.yaml");
    if let Some(contents) = env {
        std::fs::write(dir.join(".env"), contents).expect("write .env");
    }
}

fn dotenv_config() -> Config {
    let mut config = Config::default();
    config.model.provider = PROVIDER.to_string();
    config.providers.insert(
        PROVIDER.to_string(),
        ironhermes_core::config::ProviderConfig {
            api_key_env: Some("SPAWN_CRED_DECISION_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config
}

/// A profile whose own `.env` resolves a key for its configured provider yields the
/// dotenv decision — no mint attempted.
#[tokio::test]
async fn dotenv_backed_profile_decides_dotenv() {
    let tmp = tempfile::TempDir::new().unwrap();
    let profiles_root = tmp.path().join("profiles");
    std::fs::create_dir_all(&profiles_root).unwrap();
    write_profile(
        &profiles_root,
        &dotenv_config(),
        Some("SPAWN_CRED_DECISION_API_KEY=sk-dotenv-value\n"),
    );

    let config = Config::default();
    let decision = ironhermes_core::profile_credentials::decide_spawn_credential(
        &config,
        &profiles_root,
        PROFILE,
        None,
        None,
    )
    .await;

    match decision {
        ironhermes_core::profile_credentials::SpawnCredentialDecision::Dotenv => {}
        other => panic!("expected Dotenv, got {other:?}"),
    }
}

/// A profile the gate refuses yields the refuse decision whose reason is byte-identical
/// to the reason `evaluate_profile_dispatch_at` produced for the same inputs — not a
/// re-worded or re-prefixed variant.
#[tokio::test]
async fn gate_refusal_is_carried_through_verbatim() {
    let tmp = tempfile::TempDir::new().unwrap();
    let profiles_root = tmp.path().join("profiles");
    std::fs::create_dir_all(&profiles_root).unwrap();
    // No profile directory at all — the gate refuses at Step 2 with a reason naming the
    // missing directory.
    let expected =
        ironhermes_core::dispatch_gate::evaluate_profile_dispatch_at(&profiles_root, PROFILE)
            .await;
    let expected_reason = match expected {
        ironhermes_core::dispatch_gate::DispatchDecision::Refuse { reason } => reason,
        other => panic!("test precondition failed: expected the gate itself to refuse, got {other:?}"),
    };

    let config = Config::default();
    let decision = ironhermes_core::profile_credentials::decide_spawn_credential(
        &config,
        &profiles_root,
        PROFILE,
        None,
        None,
    )
    .await;

    match decision {
        ironhermes_core::profile_credentials::SpawnCredentialDecision::Refuse {
            source,
            reason,
        } => {
            assert_eq!(
                reason, expected_reason,
                "the decision's refuse reason must be byte-identical to the gate's own reason"
            );
            assert_eq!(
                source,
                ironhermes_core::profile_credentials::RefusalSource::Gate,
                "a refusal produced directly by the gate must carry RefusalSource::Gate"
            );
        }
        other => panic!("expected Refuse, got {other:?}"),
    }
}

#[cfg(feature = "rusty-vault")]
mod rusty_vault_tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use ironhermes_core::config::{Config, ProviderConfig};
    use ironhermes_core::profile_credentials::{
        ProfileTokenAudit, SpawnCredentialDecision, decide_spawn_credential, host_profile_credentials,
    };
    use secrecy::{ExposeSecret, SecretString};
    use tempfile::TempDir;

    use super::{PROFILE, PROVIDER, write_profile};

    /// A recording [`ProfileTokenAudit`] sink — captures every field a mint call gives it,
    /// so `bot_mint_is_ledgered_with_the_accessor_never_the_token` can assert the token's
    /// exposed secret appears in NONE of them (asserting only that an accessor was
    /// recorded would pass against an implementation that also recorded the token).
    struct RecordingAudit {
        calls: Arc<Mutex<Vec<(String, String, Duration)>>>,
    }

    impl ProfileTokenAudit for RecordingAudit {
        fn record_mint(&self, slug: &str, accessor: &str, ttl: Duration) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((slug.to_string(), accessor.to_string(), ttl));
            Ok(())
        }
    }

    fn vault_backed_config(vault_dir: &std::path::Path) -> Config {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        config.vault.rusty_vault.data_dir = vault_dir.to_path_buf();
        config.vault.rusty_vault.unseal_mode = "keyfile".to_string();
        config.model.provider = PROVIDER.to_string();
        config.providers.insert(
            PROVIDER.to_string(),
            ProviderConfig {
                api_key_env: Some("SPAWN_CRED_DECISION_VAULT_API_KEY".to_string()),
                ..Default::default()
            },
        );
        config
    }

    async fn init_vault_with_secret(data_dir: &std::path::Path, secret_value: &str) {
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: data_dir.to_path_buf(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
        let store = ironhermes_vault::RustyVaultStore::open(&rv_config).expect("vault open");
        let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
        profile_store
            .put_profile_secret(
                PROFILE,
                PROVIDER,
                SecretString::from(secret_value.to_string()),
            )
            .await
            .expect("write profile secret");

        // Drop this seeding open EXPLICITLY before the host/mint opens below —
        // mirrors `ironhermes-cli/tests/worker_vault_bootstrap.rs`'s
        // `set_up_vault_and_mint` exactly: never hold more than one live open
        // against the same on-disk data_dir from this process at once.
        //
        // SAFE for tests that never call `read_profile_credential` (ledger-only,
        // refusal-reason, or mint-failure assertions) — Plan 10's finding is that
        // `RustyVaultStore::init` running in THIS process only breaks token
        // validation when a host ALSO opens (in this process) before the specific
        // mint whose token is later read back over the socket. Minting itself, and
        // everything that only inspects the MINT'S OWN return value (ledger, accessor,
        // refusal text), is unaffected. `bot_credential_is_readable_over_the_socket`
        // below — the one test that performs a real socket read-back — does NOT use
        // this helper for exactly that reason; see `init_and_seed_vault_via_subprocess`.
        drop(profile_store);
        drop(store);
    }

    /// Resolve the `ironhermes` binary this workspace's `cargo build` produced, for
    /// spawning real `vault init`/`vault migrate-profile` subprocesses.
    /// `IRONHERMES_TEST_BIN` overrides (mirroring the `IRONHERMES_WORKER_BIN` convention
    /// elsewhere in this repo); otherwise derived from this TEST binary's own path
    /// (`target/<profile>/deps/<test>` -> `target/<profile>/ironhermes`) — `ironhermes-core`
    /// has no `CARGO_BIN_EXE_ironhermes` env var available (that is only set for a
    /// package's OWN integration tests, via a dev-dependency on the binary crate, which
    /// this crate does not have).
    fn resolve_ironhermes_bin() -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("IRONHERMES_TEST_BIN") {
            let p = std::path::PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        let exe = std::env::current_exe().ok()?;
        let profile_dir = exe.parent()?.parent()?; // target/<profile>/deps/<test> -> target/<profile>
        let candidate = profile_dir.join("ironhermes");
        candidate.is_file().then_some(candidate)
    }

    /// Init AND seed a real vault via two genuinely SEPARATE `ironhermes` subprocesses —
    /// `vault init` then `vault migrate-profile` — never `RustyVaultStore::init` in this
    /// test's own process (Phase 51 Plan 10 gap-closure).
    ///
    /// # Why this exists
    ///
    /// A process that calls `RustyVaultStore::init()` itself, and later opens a
    /// long-lived host (before the specific mint whose token is read back), cannot
    /// validate that token over the socket — confirmed by a controlled 3-cell
    /// experiment (5/5 runs each): same-process-init + host-before-mint fails; either
    /// same-process-init + mint-before-host, OR separate-process-init + host-before-mint,
    /// passes. Production is ALWAYS in the safe branch: `RustyVaultStore::init` has
    /// exactly one call site in the whole workspace outside `#[cfg(test)]` code —
    /// `vault_cmd.rs`'s `cmd_init()`, the one-time `ironhermes vault init` CLI
    /// subcommand, a short-lived process that exits immediately. No hosting process
    /// (gateway, dispatcher) ever calls `init()`. This helper reproduces THAT shape —
    /// the only shape that matches how a real credential ever reaches a real worker or
    /// bot — so this test proves what it claims to prove instead of tripping a
    /// same-process-init artifact.
    ///
    /// Returns the resolved `data_dir`.
    async fn init_and_seed_vault_via_subprocess(
        home: &std::path::Path,
        slug: &str,
        provider: &str,
        api_key_env: &str,
        secret_value: &str,
    ) -> std::path::PathBuf {
        let Some(bin) = resolve_ironhermes_bin() else {
            panic!(
                "could not resolve the `ironhermes` binary for a real `vault init`/\
                 `vault migrate-profile` subprocess — set IRONHERMES_TEST_BIN to an \
                 explicit path, or build the workspace binary first \
                 (`cargo build -p ironhermes-cli --features rusty-vault --bin ironhermes`)"
            );
        };

        std::fs::create_dir_all(home).expect("mkdir home");
        let vault_dir = home.join("vault");
        // Pin `data_dir` EXPLICITLY on every on-disk config this helper writes, rather
        // than leaving it empty and relying on `IRONHERMES_HOME`/`IRONHERMES_ROOT_HOME`
        // fallback resolution (`resolve_vault_config`) to land on `home.join("vault")`.
        // The subprocess calls below DO set `IRONHERMES_HOME`, but THIS test process
        // never does — an empty `data_dir` on the on-disk profile config.yaml means the
        // gate (which reads that file directly) falls back to the REAL operator home
        // (`~/.ironhermes`) instead of this fixture, silently testing the wrong vault.
        let mut root_config = Config::default();
        root_config.vault.enabled = true;
        root_config.vault.backend = "rusty-vault".to_string();
        root_config.vault.rusty_vault.data_dir = vault_dir.clone();
        root_config.vault.rusty_vault.unseal_mode = "keyfile".to_string();
        root_config
            .save_to(&home.join("config.yaml"))
            .expect("save root config.yaml");

        let profile_dir = home.join("profiles").join(slug);
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let mut profile_config = Config::default();
        profile_config.vault.enabled = true;
        profile_config.vault.backend = "rusty-vault".to_string();
        profile_config.vault.rusty_vault.data_dir = vault_dir.clone();
        profile_config.vault.rusty_vault.unseal_mode = "keyfile".to_string();
        profile_config.model.provider = provider.to_string();
        profile_config.providers.insert(
            provider.to_string(),
            ProviderConfig {
                api_key_env: Some(api_key_env.to_string()),
                ..Default::default()
            },
        );
        profile_config
            .save_to(&profile_dir.join("config.yaml"))
            .expect("save profile config.yaml");
        std::fs::write(
            profile_dir.join(".env"),
            format!("{api_key_env}={secret_value}\n"),
        )
        .expect("write .env");

        // Real, separate, short-lived OS process — exits before this test process ever
        // opens the vault.
        let init_out = std::process::Command::new(&bin)
            .args(["vault", "init"])
            .env("IRONHERMES_HOME", home)
            .output()
            .expect("spawn `ironhermes vault init`");
        assert!(
            init_out.status.success(),
            "`ironhermes vault init` failed (the resolved binary may need `cargo build \
             --features rusty-vault`): stdout={} stderr={}",
            String::from_utf8_lossy(&init_out.stdout),
            String::from_utf8_lossy(&init_out.stderr)
        );

        // A second, independent, short-lived process performs the actual mint-target
        // write — `migrate-profile` removes the key from `.env` after writing it to the
        // vault, so the profile ends up genuinely `.env`-less for the gate.
        let migrate_out = std::process::Command::new(&bin)
            .args(["vault", "migrate-profile", slug])
            .env("IRONHERMES_HOME", home)
            .output()
            .expect("spawn `ironhermes vault migrate-profile`");
        assert!(
            migrate_out.status.success(),
            "`ironhermes vault migrate-profile {slug}` failed: stdout={} stderr={}",
            String::from_utf8_lossy(&migrate_out.stdout),
            String::from_utf8_lossy(&migrate_out.stderr)
        );

        vault_dir
    }

    // =========================================================================
    // Salt-stabilization gate (Phase 51 Plan 10 finding — see module doc)
    // =========================================================================
    //
    // A freshly `vault init`-ed vault has NO persisted token salt. `rusty_vault`'s
    // `TokenStore::new()` (called fresh on every `Core` boot) does an unsynchronized
    // check-then-write: read the salt; if absent, generate a random UUID and persist
    // it. Immediately after `init`, several Cores can each independently "win" this
    // race for a brief window before one write durably sticks — confirmed via a
    // 3-cell black-box matrix (Core E opened after a mint validates a token Core B
    // opened before the mint cannot; the pass/fail transition is sharp and permanent
    // once it occurs, but the NUMBER of opens needed to reach it varies between
    // independently-built fixtures — 3 on one, 4 on another — ruling out a fixed
    // count and confirming a race, not a threshold).
    //
    // A long-lived host that happens to open WHILE this race is unresolved caches
    // the losing salt for its entire process lifetime and never re-reads — it can
    // never self-correct without a restart. This is a REAL, if narrow, production
    // hazard: an operator who runs `ironhermes vault init` and starts the gateway
    // before the race resolves gets a gateway whose EVERY vault-backed dispatch
    // fails with `TokenInvalid` until restarted. It is out of scope for this plan to
    // fix (it lives in `rusty_vault`/`RustyVaultStore::open`, not in this module's
    // files), but it must not be silently masked by a lucky-timed test either.
    //
    // The fix here is NOT a fixed count of warm-up opens — the varying transition
    // point (3 vs. 4) proves a fixed count is still betting on a race. Instead,
    // `establish_stable_host` proves stabilization POSITIVELY against the EXACT host
    // object a test is about to use: mint a throwaway token in a genuinely separate
    // process, then try validating it through THAT SAME host (opened in-process, the
    // one the caller keeps). A synthetic "mint here, validate over there in a THIRD,
    // throwaway Core" round trip was tried first and did not reliably predict whether
    // the real host, opened moments later in the same long-running test process,
    // would itself validate correctly — so this gate discards a losing host outright
    // and opens a fresh one, rather than trusting a proxy Core's result.

    /// Resolve this test binary's OWN path — used to re-exec `salt_stabilization_mint_worker`
    /// as a genuinely separate OS process for the mint half of each attempt (in-process
    /// opens do NOT stabilize the salt, per the finding above).
    fn resolve_self_test_bin() -> std::path::PathBuf {
        std::env::current_exe().expect("resolve this test binary's own path for re-exec")
    }

    /// Bounded (20 attempts), deterministic proof that a freshly-opened
    /// [`ironhermes_core::profile_credentials::ProfileCredentialHost`] against `config`
    /// can validate a real token: mint a throwaway token in a genuinely separate
    /// process, open a candidate host in-process, and try reading the throwaway token
    /// through THAT candidate. A `TokenInvalid` result means this candidate's salt
    /// doesn't match — shut it down and try a fresh one. Any other result (including
    /// `SecretNotFound`, since the probe's dummy leaf never exists) proves the token
    /// itself was recognized, so this candidate is safe to keep and use for real.
    /// Panics with a clear, named message — never leaves a bare `TokenInvalid` for
    /// whoever hits this next — if no candidate stabilizes within the budget.
    async fn establish_stable_host(
        config: &Config,
    ) -> Arc<ironhermes_core::profile_credentials::ProfileCredentialHost> {
        const MAX_ATTEMPTS: u32 = 20;
        let exe = resolve_self_test_bin();
        let vault_dir = &config.vault.rusty_vault.data_dir;

        for attempt in 1..=MAX_ATTEMPTS {
            let handoff_dir = TempDir::new().expect("tempdir for probe handoff");
            let handoff = handoff_dir.path().join("handoff.token");

            let mint_out = std::process::Command::new(&exe)
                .args([
                    "--test-threads=1",
                    "--nocapture",
                    "--exact",
                    "rusty_vault_tests::salt_stabilization_mint_worker",
                ])
                .env("STABILIZATION_PROBE_DATA_DIR", vault_dir)
                .env("STABILIZATION_PROBE_HANDOFF", &handoff)
                .output()
                .expect("spawn mint probe subprocess");
            assert!(
                mint_out.status.success(),
                "salt-stabilization mint probe failed (attempt {attempt}): stdout={} stderr={}",
                String::from_utf8_lossy(&mint_out.stdout),
                String::from_utf8_lossy(&mint_out.stderr)
            );
            let token_str = std::fs::read_to_string(&handoff).expect("read probe handoff");
            let token = SecretString::from(token_str);

            let candidate = host_profile_credentials(config).expect("candidate host must come up");
            let result = ironhermes_vault::read_profile_credential(
                candidate.socket_path(),
                &token,
                STABILIZATION_PROBE_SLUG,
                "probe-leaf",
            )
            .await;

            if !matches!(result, Err(ironhermes_vault::ProfileClientError::TokenInvalid)) {
                eprintln!(
                    "host stabilized against {vault_dir:?} after {attempt} attempt(s): {result:?}"
                );
                return candidate;
            }

            eprintln!("attempt {attempt}: candidate host rejected the probe token, retrying");
            if let Ok(candidate) = Arc::try_unwrap(candidate) {
                candidate.shutdown().await;
            }
        }

        panic!(
            "no candidate host stabilized against {vault_dir:?} after {MAX_ATTEMPTS} attempts — \
             see this module's \"Salt-stabilization gate\" doc for the mechanism (Phase 51 Plan 10)"
        );
    }

    const STABILIZATION_PROBE_SLUG: &str = "salt-stabilization-probe";

    /// Hidden worker, re-exec'd by [`establish_stable_host`] as a genuinely separate
    /// process — never runs under a normal test invocation (no-ops immediately when
    /// `STABILIZATION_PROBE_DATA_DIR` is unset). Mints a throwaway token against
    /// `STABILIZATION_PROBE_DATA_DIR` and writes it to `STABILIZATION_PROBE_HANDOFF` for
    /// the parent process to validate through its own candidate host.
    #[tokio::test(flavor = "multi_thread")]
    async fn salt_stabilization_mint_worker() {
        let Ok(data_dir) = std::env::var("STABILIZATION_PROBE_DATA_DIR") else {
            return; // no-op under a normal `cargo test` run
        };
        let handoff = std::path::PathBuf::from(std::env::var("STABILIZATION_PROBE_HANDOFF").unwrap());
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: std::path::PathBuf::from(data_dir),
            unseal_mode: "keyfile".to_string(),
        };

        struct NoopAudit;
        impl ProfileTokenAudit for NoopAudit {
            fn record_mint(&self, _: &str, _: &str, _: Duration) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let minted = ironhermes_vault::mint_profile_token_via_config(
            &rv_config,
            STABILIZATION_PROBE_SLUG,
            ironhermes_vault::profile_token_ttl_for_bootstrap(),
            &NoopAudit,
        )
        .await
        .expect("mint probe token");
        std::fs::write(&handoff, minted.token().expose_secret()).expect("write probe handoff");
    }

    /// PERMANENT REGRESSION PIN — NOT an ordering rule. This demonstrates the salt race
    /// (see the "Salt-stabilization gate" doc above), not "mint before host is correct
    /// and host before mint is wrong." Both orderings are equally exposed to the race in
    /// principle; this specific ordering (mint fully completes and closes before the host
    /// opens) happens to dodge it because `vault init`'s own ephemeral unseal cycle
    /// already gives the salt one full write-then-readback pass before this test's own
    /// mint runs. Do not generalize "mint first" as a fix — use
    /// `establish_stable_host` for anything that needs a real guarantee.
    #[tokio::test(flavor = "multi_thread")]
    async fn diagnostic_mint_before_host_ordering_experiment() {
        const SECRET_VALUE: &str = "sk-diagnostic-ordering-value";

        let tmp = TempDir::new().unwrap();
        let profiles_root = tmp.path().join("profiles");
        std::fs::create_dir_all(&profiles_root).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault_with_secret(&vault_dir, SECRET_VALUE).await;

        let config = vault_backed_config(&vault_dir);
        write_profile(&profiles_root, &config, None);

        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: vault_dir.clone(),
            unseal_mode: "keyfile".to_string(),
        };

        // MINT FIRST — same primitive mint_worker_credential calls internally.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingAudit {
            calls: Arc::clone(&calls),
        };
        let minted = ironhermes_vault::mint_profile_token_via_config(
            &rv_config,
            PROFILE,
            ironhermes_vault::profile_token_ttl_for_bootstrap(),
            &sink,
        )
        .await
        .expect("mint profile token");

        // THEN open the host — after the mint's own Core has already closed.
        let host = host_profile_credentials(&config).expect(
            "a genuinely reachable, unsealed vault must yield a hosted credential endpoint",
        );

        let token = secrecy::SecretString::from(
            secrecy::ExposeSecret::expose_secret(minted.token()).to_string(),
        );

        let read_back =
            ironhermes_vault::read_profile_credential(host.socket_path(), &token, PROFILE, PROVIDER)
                .await
                .expect("reading the minted credential back over the real socket must succeed");

        assert_eq!(read_back.expose_secret(), SECRET_VALUE);

        if let Ok(host) = Arc::try_unwrap(host) {
            host.shutdown().await;
        }
    }

    /// THE TRACER ASSERTION. Was `#[ignore]`d as a DOCUMENTED, HONEST REPRO of an
    /// upstream `rusty_vault` defect; un-ignored by Phase 51 Plan 11 (CR-06 fix). When
    /// it passes, it proves: a real vault, a real hosted endpoint, and a `.env`-less
    /// profile with a seeded `secret/profiles/{slug}/{provider}` — the decision yields
    /// the vault arm, and the returned token + socket path actually WORK: read back
    /// through the real client, the value equals the seeded secret. A decision that
    /// mints a token the endpoint's own `profile_mismatch` or policy check would refuse
    /// passes every weaker test and fails this one.
    ///
    /// **Why it used to fail, and why the failure class is now unreachable (Phase 51
    /// Plan 11):** `rusty_vault`'s `TokenStore::new()` performs an unsynchronized
    /// check-then-write on a process-scoped token salt on every `Core` boot, so two
    /// `Core`s that disagree on that salt disagree on every token lookup between them.
    /// `decide_spawn_credential`'s internal `mint_worker_credential` call used to open a
    /// SECOND, independent `Core` (via `mint_profile_token_via_config`) to mint a token
    /// that the test's `host` — a DIFFERENT `Core` — then had to validate; that is
    /// exactly the two-`Core`-disagreement shape. The fix (`mint_profile_token_for_host`,
    /// `crates/ironhermes-vault/src/lib.rs`) mints through the SAME `Core` the endpoint
    /// hosts with, using `pub(crate)` accessors on `ProfileCredentialEndpointHandle` — so
    /// mint and validation now share one `Core`, one `TokenStore`, one salt by
    /// construction, and there is no second `Core` left on this path for the salt to
    /// disagree with. See `51-11-SUMMARY.md` for the fix record; `51-10-SUMMARY.md` for
    /// the original investigation arc this superseded.
    #[tokio::test(flavor = "multi_thread")]
    async fn bot_credential_is_readable_over_the_socket() {
        const SECRET_VALUE: &str = "sk-tracer-seeded-value";
        const API_KEY_ENV: &str = "SPAWN_CRED_DECISION_VAULT_API_KEY";

        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        // Seeded via two genuinely separate `ironhermes` subprocesses — never
        // `RustyVaultStore::init` in THIS process — so this test proves what it claims
        // (a real read-back) without tripping the same-process-init artifact. See
        // `init_and_seed_vault_via_subprocess`'s doc for the full finding.
        let vault_dir =
            init_and_seed_vault_via_subprocess(&home, PROFILE, PROVIDER, API_KEY_ENV, SECRET_VALUE)
                .await;
        let profiles_root = home.join("profiles");

        let config = vault_backed_config(&vault_dir);

        // Deterministic proof this exact host object can validate a real token (Phase
        // 51 Plan 10 finding — see the "Salt-stabilization gate" doc above) — never a
        // fixed count of warm-up opens.
        let host = establish_stable_host(&config).await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingAudit {
            calls: Arc::clone(&calls),
        };

        let decision =
            decide_spawn_credential(&config, &profiles_root, PROFILE, Some(&host), Some(&sink))
                .await;

        let minted = match decision {
            SpawnCredentialDecision::Vault(minted) => minted,
            other => panic!(
                "expected the vault arm for a genuinely reachable, seeded, .env-less \
                 profile, got {other:?}"
            ),
        };

        let read_back = ironhermes_vault::read_profile_credential(
            &minted.socket_path,
            &minted.token,
            PROFILE,
            PROVIDER,
        )
        .await
        .expect("reading the minted credential back over the real socket must succeed");

        assert_eq!(
            read_back.expose_secret(),
            SECRET_VALUE,
            "the credential read back over the socket must equal the seeded secret — a \
             token-shaped string that the endpoint would refuse is not a working credential"
        );

        if let Ok(host) = Arc::try_unwrap(host) {
            host.shutdown().await;
        }
    }

    /// Driving the same vault-arm path through a recording sink produces exactly one
    /// record, carrying the slug and the accessor; the token's exposed secret appears in
    /// no field of that record.
    #[tokio::test(flavor = "multi_thread")]
    async fn bot_mint_is_ledgered_with_the_accessor_never_the_token() {
        let tmp = TempDir::new().unwrap();
        let profiles_root = tmp.path().join("profiles");
        std::fs::create_dir_all(&profiles_root).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault_with_secret(&vault_dir, "sk-ledger-seeded-value").await;

        let config = vault_backed_config(&vault_dir);
        write_profile(&profiles_root, &config, None);

        let host = host_profile_credentials(&config).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingAudit {
            calls: Arc::clone(&calls),
        };

        let decision =
            decide_spawn_credential(&config, &profiles_root, PROFILE, Some(&host), Some(&sink))
                .await;

        let minted = match decision {
            SpawnCredentialDecision::Vault(minted) => minted,
            other => panic!("expected the vault arm, got {other:?}"),
        };
        let token_secret = minted.token.expose_secret().to_string();

        {
            let recorded = calls.lock().unwrap();
            assert_eq!(recorded.len(), 1, "exactly one mint must be ledgered");
            let (slug, accessor, _ttl) = &recorded[0];
            assert_eq!(slug, PROFILE);
            assert_eq!(accessor, &minted.accessor);
            assert_ne!(
                accessor, &token_secret,
                "the accessor must never equal the token's own secret bytes"
            );
            assert!(
                !token_secret.is_empty() && accessor != &token_secret && slug != &token_secret,
                "the token's exposed secret must appear in no recorded field"
            );
        }

        if let Ok(host) = Arc::try_unwrap(host) {
            host.shutdown().await;
        }
    }

    /// The same seeded setup with no hosted endpoint supplied refuses, and the reason
    /// names the absent endpoint rather than producing a generic denial or falling
    /// through to the dotenv arm.
    #[tokio::test(flavor = "multi_thread")]
    async fn allow_from_vault_without_a_host_refuses() {
        let tmp = TempDir::new().unwrap();
        let profiles_root = tmp.path().join("profiles");
        std::fs::create_dir_all(&profiles_root).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault_with_secret(&vault_dir, "sk-no-host-seeded-value").await;

        let config = vault_backed_config(&vault_dir);
        write_profile(&profiles_root, &config, None);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingAudit {
            calls: Arc::clone(&calls),
        };

        let decision =
            decide_spawn_credential(&config, &profiles_root, PROFILE, None, Some(&sink)).await;

        match decision {
            SpawnCredentialDecision::Refuse { source, reason } => {
                assert!(
                    reason.to_lowercase().contains("host")
                        || reason.to_lowercase().contains("endpoint"),
                    "reason must name the absent hosted endpoint, got: {reason}"
                );
                assert_eq!(
                    source,
                    ironhermes_core::profile_credentials::RefusalSource::MintUnavailable,
                    "a missing hosted endpoint is a mint-unavailable refusal, not a gate refusal"
                );
            }
            other => panic!("expected Refuse (no host), got {other:?}"),
        }
        assert!(
            calls.lock().unwrap().is_empty(),
            "no mint may be attempted without a hosted endpoint"
        );
    }

    /// Same, with no audit sink supplied — refuses rather than minting un-audited.
    #[tokio::test(flavor = "multi_thread")]
    async fn allow_from_vault_without_a_sink_refuses() {
        let tmp = TempDir::new().unwrap();
        let profiles_root = tmp.path().join("profiles");
        std::fs::create_dir_all(&profiles_root).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault_with_secret(&vault_dir, "sk-no-sink-seeded-value").await;

        let config = vault_backed_config(&vault_dir);
        write_profile(&profiles_root, &config, None);

        let host = host_profile_credentials(&config).unwrap();

        let decision =
            decide_spawn_credential(&config, &profiles_root, PROFILE, Some(&host), None).await;

        match decision {
            SpawnCredentialDecision::Refuse { source, reason } => {
                assert!(
                    reason.to_lowercase().contains("sink") || reason.to_lowercase().contains("audit"),
                    "reason must name the absent audit sink, got: {reason}"
                );
                assert_eq!(
                    source,
                    ironhermes_core::profile_credentials::RefusalSource::MintUnavailable,
                    "a missing audit sink is a mint-unavailable refusal, not a gate refusal"
                );
            }
            other => panic!("expected Refuse (no sink), got {other:?}"),
        }

        if let Ok(host) = Arc::try_unwrap(host) {
            host.shutdown().await;
        }
    }

    /// A vault the gate proved reachable but whose mint fails (Phase 51 Plan 11: the mint's
    /// own `config` has `vault.enabled = false`, tripping `mint_worker_credential`'s guard
    /// check — the only remaining config-driven failure point now that the mint goes
    /// through the host's own `Core` rather than re-opening a store from config) refuses,
    /// and the reason carries the underlying mint error.
    #[tokio::test(flavor = "multi_thread")]
    async fn mint_failure_refuses() {
        let tmp = TempDir::new().unwrap();
        let profiles_root = tmp.path().join("profiles");
        std::fs::create_dir_all(&profiles_root).unwrap();
        let vault_dir = tmp.path().join("vault");
        init_vault_with_secret(&vault_dir, "sk-mint-failure-seeded-value").await;

        // The PROFILE's own config.yaml (read by the gate) points at the real, reachable
        // vault, so the gate genuinely resolves AllowFromVault.
        let profile_config = vault_backed_config(&vault_dir);
        write_profile(&profiles_root, &profile_config, None);

        // The host is built from the SAME real vault, so it is genuinely hosted.
        let host = host_profile_credentials(&profile_config).unwrap();

        // Phase 51 Plan 11 (CR-06 fix): `mint_worker_credential` no longer opens a second
        // store from a freshly-resolved `Config` — it mints through `host`'s own
        // already-open `Core`. Repointing the mint's `Config` at an uninitialized data dir
        // (the pre-Plan-11 failure injection) is therefore a no-op now: no new store is
        // ever opened on this path. The mint's own guard check
        // (`config.vault.enabled && config.vault.backend == "rusty-vault"`) is the only
        // remaining config-driven failure point, so flip THAT instead: the `config` PASSED
        // TO THE MINT has vault disabled, even though the gate (which independently reads
        // the PROFILE's own config.yaml under `profiles_root`, untouched here) and the host
        // are both genuinely healthy.
        let mut mint_config = profile_config.clone();
        mint_config.vault.enabled = false;

        let calls = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingAudit {
            calls: Arc::clone(&calls),
        };

        let decision = decide_spawn_credential(
            &mint_config,
            &profiles_root,
            PROFILE,
            Some(&host),
            Some(&sink),
        )
        .await;

        match decision {
            SpawnCredentialDecision::Refuse { source, reason } => {
                assert!(
                    !reason.is_empty(),
                    "a mint failure's refuse reason must carry the underlying error, not be empty"
                );
                assert_eq!(
                    source,
                    ironhermes_core::profile_credentials::RefusalSource::MintUnavailable,
                    "a mint failure is a mint-unavailable refusal, not a gate refusal"
                );
            }
            other => panic!("expected Refuse (mint failure), got {other:?}"),
        }
        assert!(
            calls.lock().unwrap().is_empty(),
            "a failed mint must not ledger anything"
        );

        if let Ok(host) = Arc::try_unwrap(host) {
            host.shutdown().await;
        }
    }

    /// WR-03: proves the coupling a substring match on `reason` used to depend on can no
    /// longer silently break. A gate refusal (no profile directory at all — the mint is
    /// never reached) and a mint-unavailable refusal (the gate genuinely resolves
    /// `AllowFromVault` against a real, reachable, seeded vault, but no hosted endpoint is
    /// supplied) must carry DISTINCT `RefusalSource` values — the discriminant a caller
    /// routes on, not the shape of either crate's `format!` output. Nothing here inspects
    /// `reason`'s wording; a cosmetic reword of either literal changes no assertion in this
    /// test, which is exactly the property the substring match this replaces did not have.
    #[tokio::test(flavor = "multi_thread")]
    async fn gate_refusal_and_mint_unavailable_refusal_carry_distinct_sources() {
        // Gate refusal: no profile directory at all — `evaluate_profile_dispatch_at`
        // refuses at Step 2, and the mint is never attempted.
        let tmp_gate = TempDir::new().unwrap();
        let profiles_root_gate = tmp_gate.path().join("profiles");
        std::fs::create_dir_all(&profiles_root_gate).unwrap();
        let gate_config = Config::default();
        let gate_decision =
            decide_spawn_credential(&gate_config, &profiles_root_gate, PROFILE, None, None).await;
        let gate_source = match gate_decision {
            SpawnCredentialDecision::Refuse { source, .. } => source,
            other => panic!("expected Refuse (gate), got {other:?}"),
        };
        assert_eq!(gate_source, ironhermes_core::profile_credentials::RefusalSource::Gate);

        // Mint-unavailable refusal: the gate genuinely resolves `AllowFromVault` against a
        // real, reachable, seeded vault (mirrors `allow_from_vault_without_a_host_refuses`),
        // but no hosted endpoint is supplied — `decide_spawn_credential` refuses before the
        // mint is ever attempted.
        let tmp_mint = TempDir::new().unwrap();
        let profiles_root_mint = tmp_mint.path().join("profiles");
        std::fs::create_dir_all(&profiles_root_mint).unwrap();
        let vault_dir = tmp_mint.path().join("vault");
        init_vault_with_secret(&vault_dir, "sk-routing-distinct-sources-seeded-value").await;
        let mint_config = vault_backed_config(&vault_dir);
        write_profile(&profiles_root_mint, &mint_config, None);
        let mint_decision =
            decide_spawn_credential(&mint_config, &profiles_root_mint, PROFILE, None, None).await;
        let mint_source = match mint_decision {
            SpawnCredentialDecision::Refuse { source, .. } => source,
            other => panic!("expected Refuse (mint unavailable), got {other:?}"),
        };
        assert_eq!(
            mint_source,
            ironhermes_core::profile_credentials::RefusalSource::MintUnavailable
        );

        assert_ne!(
            gate_source, mint_source,
            "a gate refusal and a mint-unavailable refusal must carry distinguishable sources"
        );
    }
}

/// Compiled and RUN specifically WITHOUT the `rusty-vault` feature. With the feature off,
/// `decide_spawn_credential` is still callable; the vault arm is structurally
/// unreachable (the gate's own vault branch is feature-gated to the same condition), so a
/// config claiming `vault.enabled` still never produces a spawn without a credential.
#[cfg(not(feature = "rusty-vault"))]
#[tokio::test]
async fn decision_compiles_and_refuses_when_the_feature_is_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let profiles_root = tmp.path().join("profiles");
    std::fs::create_dir_all(&profiles_root).unwrap();

    let mut config = Config::default();
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    config.model.provider = PROVIDER.to_string();
    config.providers.insert(
        PROVIDER.to_string(),
        ironhermes_core::config::ProviderConfig {
            api_key_env: Some("SPAWN_CRED_DECISION_FEATURE_OFF_KEY".to_string()),
            ..Default::default()
        },
    );
    // Deliberately no .env — with the feature off, the vault can never be reached
    // regardless, so this must refuse rather than falling through to a spawn.
    write_profile(&profiles_root, &config, None);

    let decision = ironhermes_core::profile_credentials::decide_spawn_credential(
        &config,
        &profiles_root,
        PROFILE,
        None,
        None,
    )
    .await;

    match decision {
        ironhermes_core::profile_credentials::SpawnCredentialDecision::Refuse {
            source,
            reason,
        } => {
            assert!(
                !reason.is_empty(),
                "the feature-absent refuse reason must be non-empty"
            );
            // The gate itself refuses here (`evaluate_vault_branch`'s feature-off arm
            // returns a vault-unreachable `Refuse`, never `AllowFromVault`) — the mint is
            // never reached, so this is a gate refusal, not a mint-unavailable one.
            assert_eq!(
                source,
                ironhermes_core::profile_credentials::RefusalSource::Gate,
                "with the feature absent, the gate itself refuses before any mint is attempted"
            );
        }
        other => panic!(
            "with the rusty-vault feature not compiled in, a config claiming vault.enabled \
             must never produce a spawnable decision, got {other:?}"
        ),
    }
}
