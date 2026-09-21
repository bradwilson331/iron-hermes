//! Phase 46.8 Plan 02's original Wave-0 spike, re-proven against `rusty_vault` **0.3.1**
//! (git rev `0922e6d5e6fbe6fd2d909863eca6d583623f4ad7`, Phase 51 Plan 01 — D-01/D-02/D-03).
//!
//! D-15: this test embeds NO real secret — only a throwaway placeholder key/value pair
//! (`secret/providers/spike-key` -> a dummy string). The `tempfile::TempDir` data dir is
//! auto-removed on drop; nothing here is persisted outside the test process.
//!
//! Feature-gated: only compiles/runs with `--features rusty-vault` (default build never
//! touches this file or the `rusty_vault`/openssl dependency tree — D-10).
//!
//! # Observed `Core` construction sequence (Assumption A1 — SETTLED, not inferred)
//!
//! At the pinned rev, `Core` has **no** `Core::new(...)`-then-manual-field-literal shape and
//! **no** `Core::config(...)` call at all (`fn config(` does not exist anywhere in the 0.3.1
//! source). The real, confirmed sequence:
//! 1. Build the `file` physical backend via `storage::new_backend("file", &conf)`.
//! 2. `Core::new(backend)` — this constructor builds its OWN `AESGCMBarrier` from the same
//!    backend internally (it absorbs exactly what the 0.2.1 adapter used to build by hand:
//!    `barrier_aes_gcm::AESGCMBarrier::new(backend.clone())`), plus a fresh `Router` and
//!    `MountsRouter`. No separate barrier construction is needed or possible from outside.
//! 3. `.wrap()` — **mandatory, not optional**. It moves the `Core` into an `Arc` and wires
//!    `Core::self_ptr` (a `Weak<Core>` back-reference) via an `unsafe` raw-pointer round trip.
//!    `Core::post_unseal()` (invoked from inside every `unseal()` call) does
//!    `self.self_ptr.upgrade().unwrap()` unconditionally to hand the mounts router a strong
//!    `Arc<Core>` — calling `init`/`unseal` on a bare, un-wrapped `Core` panics there. There is
//!    no external lock: `Core`'s fields are `ArcSwap`/`ArcSwapOption` throughout, so the type
//!    this crate holds is a plain `Arc<Core>`, not `Arc<RwLock<Core>>`.
//! 4. `inited`, `init`, and `unseal` are now `async fn` (0.2.1 had them sync); `sealed()` stays
//!    a plain sync `fn`. No explicit `mount()` call for `"secret/"` is needed or possible —
//!    it is still a DEFAULT `kv` mount, seeded by `post_unseal()`'s `mounts_router.load_or_default`
//!    the first time `unseal()` succeeds (confirmed again below, unchanged from 0.2.1).
//!
//! **`Core::new(backend).wrap()` alone is NOT sufficient — observed by running it, not
//! inferred.** It leaves `Core::module_manager` empty, so the very first `init()` fails with
//! `ErrCoreLogicalBackendNoExist` (no `"kv"` logical-backend factory registered for the default
//! `"secret/"` mount to use). The step `Core::config(...)` used to perform is
//! `ModuleManager::set_default_modules` (registers Kv + System) plus registering `AuthModule`
//! (needed for `TokenStore`, which every `client_token` check goes through) — and upstream's own
//! top-level constructor, `rusty_vault::RustyVault::new(backend, config)` (`src/lib.rs:92-147`),
//! already does this plus the crate's other built-in feature modules, in the crate's own
//! canonical order. This spike (and the production adapter) therefore call
//! `RustyVault::new(backend, None)` and extract its `Arc<Core>` via `rv.core.load_full()` rather
//! than hand-reimplementing the wiring.
//!
//! # Observed `Send` outcome for `Core::handle_request`'s future (Assumption A2 — SETTLED)
//!
//! `handle_request_future_send_outcome` below applies `fn assert_send<T: Send>(_: T) {}` to
//! the literal future `Core::handle_request` returns. **It compiles — the future IS `Send`.**
//! `self.handlers.load()` returns an `arc_swap::Guard`, held across the internal `.await`
//! points exactly where 0.2.1 held a `std::sync::RwLockReadGuard` (which made 0.2.1's future
//! non-`Send`) — but `Guard<T, DefaultStrategy>` is itself `Send`, so this specific blocker is
//! gone at 0.3.1.
//!
//! **CHOSEN STRATEGY — retained `spawn_blocking` + `Handle::block_on` bridge, not a direct
//! `.await`.** Even though the future is provably `Send` now, `crates/ironhermes-vault/src/
//! rusty_vault_store.rs`'s `run_request` keeps the exact same blocking-pool bridge it used
//! against 0.2.1. Rationale (recorded here because Plans 02, 03, 04, 06 and 09 build `Core`
//! fixtures against whichever answer this file records): the bridge is a *correct superset*
//! regardless of the `Send` answer, this plan is explicitly scoped to prove EXISTING behavior
//! unchanged rather than introduce a new async shape, and `tokio::task::block_in_place` — the
//! one alternative that would let `run_request` skip `spawn_blocking` — is prohibited outright
//! (it panics inside `iron_hermes_ui`'s per-connection `LocalSet`). A later phase may
//! reconsider simplifying `run_request` to a direct `.await` now that `Send` is proven, but
//! that is a deliberate follow-on change, not a byproduct of this migration.
#![cfg(feature = "rusty-vault")]

use std::{collections::HashMap, path::Path, sync::Arc};

use rusty_vault::{
    RustyVault,
    core::{Core, SealConfig},
    logical::{Operation, Request},
    storage,
};
use serde_json::{Value, json};

/// Construct a fresh, sealed `Arc<Core>` wired to a `file`-backed physical storage rooted at
/// `data_dir`. Mirrors `rusty_vault_store.rs::new_core` exactly (see that file's module doc and
/// this file's module doc §3a for the full rationale — bare `Core::new(backend).wrap()` leaves
/// `module_manager` empty and the first `init()` fails) — kept as a local reimplementation here
/// rather than importing the crate internal, matching this file's role as an independent proof
/// against the raw upstream API.
fn new_core(data_dir: &Path) -> Arc<Core> {
    let mut conf: HashMap<String, Value> = HashMap::new();
    conf.insert(
        "path".to_string(),
        Value::String(data_dir.to_string_lossy().into_owned()),
    );
    let backend = storage::new_backend("file", &conf).expect("construct file physical backend");
    let rv = RustyVault::new(backend, None).expect("construct rusty_vault core");
    rv.core.load_full()
}

/// Build a `secret/<rest>` read or write `Request` against the mounted `kv` backend. Unchanged
/// from the 0.2.1 spike — `Request`/`Operation`'s shape and the "every request needs
/// `client_token`" requirement (`TokenStore::pre_route` rejects an empty token) did not move.
fn write_request(path: &str, value: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Write;
    req.client_token = root_token.to_string();
    req.body = Some(
        json!({ "value": value })
            .as_object()
            .expect("json object")
            .clone(),
    );
    req
}

fn read_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Read;
    req.client_token = root_token.to_string();
    req
}

fn delete_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::Delete;
    req.client_token = root_token.to_string();
    req
}

fn list_request(path: &str, root_token: &str) -> Request {
    let mut req = Request::new(path);
    req.operation = Operation::List;
    req.client_token = root_token.to_string();
    req
}

#[tokio::test]
async fn init_unseal_mount_put_get_delete_list_round_trip_and_reload_persists() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");

    // --- construct -> init -> unseal (single-operator posture, D-05/RESEARCH Pitfall 6:
    // shares=1, threshold=1 so unseal() returns Ok(true) on the first and only key) ---
    let core = new_core(tmp.path());

    let seal_config = SealConfig {
        secret_shares: 1,
        secret_threshold: 1,
    };
    let init_result = core.init(&seal_config).await.expect("core.init()");
    assert_eq!(
        init_result.secret_shares.len(),
        1,
        "shares=1/threshold=1 must yield exactly one key share (no Shamir splitting)"
    );

    let unsealed = core
        .unseal(&init_result.secret_shares[0])
        .await
        .expect("core.unseal()");
    assert!(
        unsealed,
        "single-share unseal must return Ok(true) on the first call"
    );

    // --- NO manual mount() call --- "secret/" -> kv is a DEFAULT mount, seeded by
    // `post_unseal()`'s `mounts_router.load_or_default` on the first successful `unseal()`,
    // unchanged in shape from 0.2.1 (confirmed again here at 0.3.1: a put against
    // "secret/providers/..." below succeeds with zero mount setup code).

    const SPIKE_PATH: &str = "secret/providers/spike-key";
    const SPIKE_VALUE: &str = "spike-placeholder-value";
    let root_token = init_result.root_token.clone();

    // --- put ---
    {
        let mut req = write_request(SPIKE_PATH, SPIKE_VALUE, &root_token);
        let resp = core
            .handle_request(&mut req)
            .await
            .expect("handle_request(write)");
        assert!(resp.is_none(), "kv write returns no response body");
    }

    // --- get ---
    {
        let mut req = read_request(SPIKE_PATH, &root_token);
        let resp = core
            .handle_request(&mut req)
            .await
            .expect("handle_request(read)")
            .expect("kv read returns Some(Response) for an existing key");
        let data = resp.data.expect("response.data present");
        let round_tripped = data
            .get("value")
            .and_then(Value::as_str)
            .expect("value field present and a string");
        assert_eq!(
            round_tripped, SPIKE_VALUE,
            "round-tripped value must equal what was written"
        );
    }

    // --- list (before delete: the spike key must be present) ---
    {
        let mut req = list_request("secret/providers/", &root_token);
        let resp = core
            .handle_request(&mut req)
            .await
            .expect("handle_request(list)")
            .expect("kv list returns Some(Response) when at least one key exists");
        let data = resp.data.expect("response.data present");
        let keys = data
            .get("keys")
            .and_then(Value::as_array)
            .expect("keys field present and an array");
        assert!(
            keys.iter().any(|k| k.as_str() == Some("spike-key")),
            "list must include the just-written key, got {keys:?}"
        );
    }

    // --- delete ---
    {
        let mut req = delete_request(SPIKE_PATH, &root_token);
        core.handle_request(&mut req)
            .await
            .expect("handle_request(delete)");
    }

    // --- get after delete: must be absent, not an error ---
    {
        let mut req = read_request(SPIKE_PATH, &root_token);
        let resp = core
            .handle_request(&mut req)
            .await
            .expect("handle_request(read) after delete");
        assert!(
            resp.is_none(),
            "a deleted key must read back as no Response, not an error"
        );
    }

    // --- drop this Core, reconstruct a FRESH Core from the SAME data_dir, unseal again, and
    // prove the `file`-backend + AES-GCM barrier persisted the mount table across the reload
    // (write a fresh value first since the spike key above was deleted) ---
    let mut req = write_request(SPIKE_PATH, SPIKE_VALUE, &root_token);
    core.handle_request(&mut req)
        .await
        .expect("handle_request(write) before reload");
    drop(core);

    let core2 = new_core(tmp.path());
    // Barrier is already initialized (persisted to disk by the first Core) — do NOT call
    // init() again, it would return ErrBarrierAlreadyInit.
    let unsealed = core2
        .unseal(&init_result.secret_shares[0])
        .await
        .expect("core2.unseal()");
    assert!(unsealed, "reload must unseal with the same key share");

    // No re-mount call here: `post_unseal()` re-registers every persisted `MountEntry`
    // (including "secret/" -> kv) automatically on unseal.
    {
        // The root token itself is stored (via TokenStore's own barrier-backed storage) in the
        // same file-backed data_dir, so it too survives the reload — reuse the SAME
        // root_token captured from the original init() above.
        let mut req = read_request(SPIKE_PATH, &root_token);
        let resp = core2
            .handle_request(&mut req)
            .await
            .expect("handle_request(read) on reloaded core")
            .expect("kv read returns Some(Response) after reload");
        let data = resp.data.expect("response.data present after reload");
        let round_tripped = data
            .get("value")
            .and_then(Value::as_str)
            .expect("value field present and a string after reload");
        assert_eq!(
            round_tripped, SPIKE_VALUE,
            "value must survive a fresh Core reload from the same data_dir (file-backend persistence)"
        );
    }
}

/// Assumption A2 — see the module doc's "Observed `Send` outcome" section for the full
/// analysis and the CHOSEN bridging strategy this result feeds into.
#[tokio::test]
async fn handle_request_future_send_outcome() {
    fn assert_send<T: Send>(_: T) {}

    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());
    let seal_config = SealConfig {
        secret_shares: 1,
        secret_threshold: 1,
    };
    let init_result = core.init(&seal_config).await.expect("core.init()");
    core.unseal(&init_result.secret_shares[0])
        .await
        .expect("core.unseal()");

    let mut req = read_request("secret/providers/send-check", &init_result.root_token);
    let fut = core.handle_request(&mut req);
    // Reaching this line means the future compiles as `Send` — see the module doc for what
    // this does and does not change about `run_request`'s implementation.
    assert_send(fut);
}

/// Assumption A2's sync-to-async bridge, the "no runtime at all" branch: proves
/// `RustyVaultStore::init` (a plain sync `pub fn`) works correctly when called from a thread
/// that has never entered any tokio runtime — `Handle::try_current()` must return `Err` here
/// (this is an ordinary `#[test]` fn, not `#[tokio::test]`), exercising
/// `block_on_admin_call`'s private-runtime fallback rather than its `spawn_blocking` branch.
#[test]
fn init_from_outside_any_runtime() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let cfg = ironhermes_vault::RustyVaultConfig {
        data_dir: tmp.path().join("vault"),
        unseal_mode: "keyfile".to_string(),
    };

    ironhermes_vault::RustyVaultStore::init(&cfg).expect(
        "RustyVaultStore::init must succeed from a plain non-async #[test] fn with no tokio \
         runtime on this thread",
    );
    let store = ironhermes_vault::RustyVaultStore::open(&cfg)
        .expect("RustyVaultStore::open must also succeed with no runtime on this thread");
    assert!(
        !store.is_sealed().expect("is_sealed"),
        "keyfile mode auto-unseals"
    );
}

/// T-51-SC / T-51-02 — the supply-chain anchor for this migration: the dependency must stay
/// pinned to the exact audited commit `rev`, never a mutable tag, a branch, or a different
/// commit. Fails if anyone re-points `crates/ironhermes-vault/Cargo.toml` or if the workspace
/// `Cargo.lock` ever resolves to a different commit.
#[test]
fn rusty_vault_pin_is_the_audited_rev() {
    const AUDITED_REV: &str = "0922e6d5e6fbe6fd2d909863eca6d583623f4ad7";

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    let cargo_toml_path = manifest_dir.join("Cargo.toml");
    let cargo_toml = std::fs::read_to_string(&cargo_toml_path)
        .unwrap_or_else(|e| panic!("read {cargo_toml_path:?}: {e}"));
    assert!(
        cargo_toml.contains(AUDITED_REV),
        "crates/ironhermes-vault/Cargo.toml must pin rusty_vault's git dependency to the \
         audited rev {AUDITED_REV}, not a tag, branch, or different commit"
    );

    let workspace_lock_path = manifest_dir.join("..").join("..").join("Cargo.lock");
    let cargo_lock = std::fs::read_to_string(&workspace_lock_path)
        .unwrap_or_else(|e| panic!("read {workspace_lock_path:?}: {e}"));
    assert!(
        cargo_lock.contains(AUDITED_REV),
        "the workspace Cargo.lock must resolve rusty_vault to the audited rev {AUDITED_REV}"
    );
}
