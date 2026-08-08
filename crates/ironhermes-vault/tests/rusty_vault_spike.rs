//! Phase 46.8 Plan 02 — Wave-0 spike proving the REAL `rusty_vault` =0.2.1 embedding API
//! (RESEARCH Open Question 1) before `RustyVaultStore` is implemented in Plan 03.
//!
//! D-15: this test embeds NO real secret — only a throwaway placeholder key/value pair
//! (`secret/providers/spike-key` -> a dummy string). The `tempfile::TempDir` data dir is
//! auto-removed on drop; nothing here is persisted outside the test process.
//!
//! Feature-gated: only compiles/runs with `--features rusty-vault` (default build never
//! touches this file or the `rusty_vault`/openssl dependency tree — D-10).
#![cfg(feature = "rusty-vault")]

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, RwLock},
};

use rusty_vault::{
    core::{Core, SealConfig},
    logical::{Operation, Request},
    storage::{self, barrier_aes_gcm},
};
use serde_json::{Value, json};

/// Construct a fresh, sealed `Core` wired to a `file`-backed physical storage rooted at
/// `data_dir`, with an AES-GCM encryption barrier over it.
///
/// CORRECTED vs RESEARCH's Code-Examples sketch: `Core` has no `Core::new(...)`
/// constructor and no free-standing `Config`-driven builder for embedded/library mode.
/// The real, confirmed pattern (mirrors the crate's own `#[cfg(test)]`-only
/// `test_utils::test_rusty_vault_core_new`, which is NOT visible to downstream crates
/// since it's gated behind `#[cfg(test)]` in `rusty_vault`'s own `lib.rs` — hence this
/// local reimplementation) is: build the physical backend + barrier by hand, then
/// construct `Core { physical, barrier, ..Default::default() }`.
fn new_core(data_dir: &Path) -> Arc<RwLock<Core>> {
    let mut conf: HashMap<String, Value> = HashMap::new();
    conf.insert(
        "path".to_string(),
        Value::String(data_dir.to_string_lossy().into_owned()),
    );
    let backend = storage::new_backend("file", &conf).expect("construct file physical backend");
    let barrier = barrier_aes_gcm::AESGCMBarrier::new(Arc::clone(&backend));
    Arc::new(RwLock::new(Core {
        physical: backend,
        barrier: Arc::new(barrier),
        ..Default::default()
    }))
}

/// Build a `secret/<rest>` read or write `Request` against the mounted `kv` backend.
///
/// CORRECTED vs RESEARCH's sketch: there is no `Core::handle_request`-adjacent
/// "handle_request against a mounted path" helper to guess at — the real, confirmed
/// shape is `Request::new(<FULL path including the mount prefix, e.g.
/// "secret/providers/spike-key">)` with `.operation` set, and for writes `.body` set to
/// the raw `Map<String, Value>` payload (the `kv` module's `handle_write` reads
/// `req.body` verbatim, NOT `req.data` — `req.data` is reserved for schema-validated
/// path fields like `ttl`). `Core::handle_request` internally routes via the mount
/// table and strips the mount prefix before invoking the backend.
///
/// SECOND CORRECTION vs RESEARCH's sketch: every request (even a purely embedded,
/// no-HTTP-server call) MUST carry a valid `client_token`. After the first
/// `unseal()`, `post_unseal()` -> `AuthModule::init()` registers `TokenStore` as an
/// additional `Core.handlers` entry (`core.add_handler(token_store)`); its `pre_route`
/// hook rejects any request with an empty `client_token` with
/// `RvError::ErrRequestClientTokenMissing` (confirmed by running this spike without a
/// token first). The root token returned in `InitResult::root_token` from `init()` is
/// the correct token for this single-operator embedded posture (D-05) — pass it here.
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

// THIRD CORRECTION vs RESEARCH's sketch: `Core::handle_request` is `async fn(&self, ...)`
// but the crate's own reference call site (`http/logical.rs`: `core.read()?.handle_request(&mut
// r).await?`) holds a `std::sync::RwLockReadGuard` across that `.await` — this is the
// crate's own confirmed API shape, not a workaround we invented. No genuine executor
// yield occurs inside `handle_request` for the `file`/mock backends used here (all I/O
// is synchronous), so this is safe in practice; `clippy::await_holding_lock` cannot see
// that statically. Plan 03's `RustyVaultStore` will need the same allow (or an
// equivalent `spawn_blocking` wrapper) wherever it calls `Core::handle_request`.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn init_unseal_mount_put_get_round_trip_and_reload_persists() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");

    // --- init -> unseal (single-operator posture, D-05/RESEARCH Pitfall 6: shares=1,
    // threshold=1 so unseal() returns Ok(true) on the first and only key) ---
    let core = new_core(tmp.path());
    let init_result = {
        let mut c = core.write().expect("lock core for init");
        // CORRECTED vs RESEARCH sketch: `config()` must be called explicitly BEFORE
        // `init()` — it registers the default modules (including the "kv" logical
        // backend factory via KvModule) and sets `Core.self_ref`, both required later
        // by `mount()`/`post_unseal()`. Neither `init()` nor `unseal()` calls it
        // implicitly.
        c.config(Arc::clone(&core), None).expect("core.config()");

        let seal_config = SealConfig {
            secret_shares: 1,
            secret_threshold: 1,
        };
        let init_result = c.init(&seal_config).expect("core.init()");
        assert_eq!(
            init_result.secret_shares.len(),
            1,
            "shares=1/threshold=1 must yield exactly one key share (no Shamir splitting)"
        );

        let unsealed = c
            .unseal(&init_result.secret_shares[0])
            .expect("core.unseal()");
        assert!(
            unsealed,
            "single-share unseal must return Ok(true) on the first call"
        );

        init_result
    };

    // --- NO manual mount() call ---
    // CORRECTED vs RESEARCH sketch: RESEARCH's Code-Examples sketch assumed a mount
    // step (`MountEntry::new("secret", "secret/", "kv", ...)` then `core.mount(&me)`)
    // was required before "secret/" was usable. In the REAL crate, `DEFAULT_CORE_MOUNTS`
    // (`mount.rs`) already seeds a `path: "secret/", logical_type: "kv"` entry the very
    // first time `mounts.load_or_default()` runs inside `post_unseal()` (i.e. on the
    // first `init()`/`unseal()`). Calling `core.mount(&me)` again for "secret/" fails
    // with `ErrMountPathExist` (confirmed by running this spike) — "secret/" -> kv is
    // ALREADY mounted after unseal, with zero extra code. `MountEntry`/`Core::mount`
    // are still the right API for a CUSTOM path (Plan 03 may still want a
    // non-default path), but the default "secret/" prefix RESEARCH/PATTERNS chose for
    // `secret/providers/<name>` needs no explicit mount call at all.

    // --- write + read back a throwaway placeholder secret (D-15: no real credential) ---
    const SPIKE_PATH: &str = "secret/providers/spike-key";
    const SPIKE_VALUE: &str = "spike-placeholder-value";

    let root_token = init_result.root_token.clone();

    {
        let c = core.read().expect("lock core for write");
        let mut req = write_request(SPIKE_PATH, SPIKE_VALUE, &root_token);
        let resp = c
            .handle_request(&mut req)
            .await
            .expect("handle_request(write)");
        // CORRECTED vs RESEARCH sketch: `kv`'s `handle_write` returns `Ok(None)` (no
        // response body on write) — mirrors typical vault semantics (204 No Content).
        assert!(resp.is_none(), "kv write returns no response body");
    }

    {
        let c = core.read().expect("lock core for read");
        let mut req = read_request(SPIKE_PATH, &root_token);
        let resp = c
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

    // --- drop this Core, reconstruct a FRESH Core from the SAME data_dir, unseal
    // again, and prove the `file`-backend + AES-GCM barrier persisted the secret AND
    // the mount table across the reload (RESEARCH Open Question 2) ---
    drop(core);

    let core2 = new_core(tmp.path());
    {
        let mut c = core2.write().expect("lock core2 for config/unseal");
        // Barrier is already initialized (persisted to disk by the first Core) — do
        // NOT call init() again, it would return ErrBarrierAlreadyInit. config() is
        // still required (fresh Core -> fresh, empty ModuleManager/self_ref).
        c.config(Arc::clone(&core2), None).expect("core2.config()");
        let unsealed = c
            .unseal(&init_result.secret_shares[0])
            .expect("core2.unseal()");
        assert!(unsealed, "reload must unseal with the same key share");
    }

    // No re-mount call here: `post_unseal()` -> `setup_mounts()` re-registers every
    // persisted `MountEntry` (including "secret/" -> kv) automatically on unseal.
    {
        let c = core2.read().expect("lock core2 for read");
        // The root token itself is stored (via TokenStore's own barrier-backed
        // storage) in the same file-backed data_dir, so it too survives the reload —
        // reuse the SAME root_token captured from the original init() above.
        let mut req = read_request(SPIKE_PATH, &root_token);
        let resp = c
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
