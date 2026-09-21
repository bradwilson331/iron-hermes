//! Proves [`ironhermes_vault::profile_endpoint`] against a real `rusty_vault` `Core` and a real
//! `0600` Unix domain socket (Phase 51 D-11/D-12/D-13/D-14, `51-06-PLAN.md`).
//!
//! # WIDENED PROTOCOL (user checkpoint reversal, 2026-09-10)
//!
//! `51-CONTEXT.md`'s original D-12 locked a request shape with NO field capable of naming a
//! profile. At this plan's Task 1 checkpoint the user selected `widen` instead: the request now
//! carries an explicit `profile` field, checked against — never trusted over — the presented
//! token's own derived slug. See `crates/ironhermes-vault/src/profile_endpoint.rs`'s module doc
//! and `51-06-SUMMARY.md` for the full record. Every test below exercises the WIDENED shape;
//! `request_cannot_address_another_profile` from the original plan text is replaced by
//! [`request_naming_a_different_profile_than_the_token_is_refused`], proving the new explicit
//! mismatch check the user's reversal required.
//!
//! Fixture pattern mirrors `tests/profile_token_mint.rs`'s own `new_core`/`open_fixture`
//! helpers exactly: a raw `Core` built directly (not via `RustyVaultStore`, whose root token is
//! `pub(crate)`-only and unreachable from an external integration test), initialized, unsealed,
//! with `alpha`/`beta` profile policies registered and a secret written at each. The SAME
//! `Arc<Core>` is then handed directly to `spawn_profile_credential_endpoint_at` (the low-level,
//! `#[doc(hidden)]` test seam), so there is only ever ONE `Core` instance per test — no
//! concurrent-Core-same-data-dir question to worry about.

#![cfg(feature = "rusty-vault")]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ironhermes_vault::{
    MintedProfileToken, ProfileGuardAudit, ProfileTokenAudit, ensure_profile_policy,
    mint_profile_token, profile_token_ttl_for_bootstrap,
};
use ironhermes_vault::profile_endpoint::spawn_profile_credential_endpoint_at;
use rusty_vault::RustyVault;
use rusty_vault::core::{Core, SealConfig};
use rusty_vault::errors::RvError;
use rusty_vault::logical::Request;
use rusty_vault::storage;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

// ---------------------------------------------------------------------------
// Fixture (mirrors profile_token_mint.rs's new_core/open_fixture exactly)
// ---------------------------------------------------------------------------

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

async fn write_secret_raw(core: &Arc<Core>, root_token: &str, path: &str, value: &str) {
    let mut req = Request::new_write_request(
        path,
        Some(
            serde_json::json!({ "value": value })
                .as_object()
                .expect("json object literal is always a map")
                .clone(),
        ),
    );
    req.client_token = root_token.to_string();
    core.handle_request(&mut req)
        .await
        .expect("write secret directly through the real Core");
}

/// Raw, uncached read directly through the `Core` — used only by
/// `endpoint_core_has_the_guard_registered`, which must bypass the endpoint's own explicit
/// mismatch check (the wire protocol never lets a mismatched `profile` reach the read at all)
/// to independently prove the Plan 04 guard is ALSO registered on this endpoint's `Core`.
#[allow(clippy::result_large_err)] // RvError (rusty_vault dep) is >=272 bytes; this helper must surface the raw variant
async fn read_secret_raw(
    core: &Arc<Core>,
    token: &str,
    path: &str,
) -> Result<Option<String>, RvError> {
    let mut req = Request::new_read_request(path);
    req.client_token = token.to_string();
    let resp = core.handle_request(&mut req).await?;
    Ok(resp
        .and_then(|r| r.data)
        .and_then(|d| d.get("value").and_then(Value::as_str).map(str::to_string)))
}

/// Write a policy directly through the real `Core`, authorized as root — used only by
/// `endpoint_core_has_the_guard_registered` to install a deliberately PERMISSIVE `profile-alpha`
/// policy (overwriting `open_fixture`'s normal restrictive one), mirroring
/// `profile_layer_independence.rs`'s own "Run B" fixture.
async fn write_policy_raw(core: &Arc<Core>, root_token: &str, name: &str, hcl: &str) {
    let mut req = Request::new_write_request(
        format!("sys/policy/{name}"),
        Some(
            serde_json::json!({ "policy": hcl })
                .as_object()
                .expect("json object literal is always a map")
                .clone(),
        ),
    );
    req.client_token = root_token.to_string();
    core.handle_request(&mut req)
        .await
        .expect("write policy directly through the real Core");
}

/// Create a token directly through `auth/token/create`, authorized as root, WITHOUT going
/// through `mint_profile_token` (which would re-register the restrictive policy this test
/// deliberately overwrote). Returns the raw client token string.
async fn create_token_raw(core: &Arc<Core>, root_token: &str, policies: &[&str]) -> String {
    let mut req = Request::new_write_request(
        "auth/token/create",
        Some(
            serde_json::json!({ "policies": policies, "ttl": "60s", "renewable": false })
                .as_object()
                .expect("json object literal is always a map")
                .clone(),
        ),
    );
    req.client_token = root_token.to_string();
    let resp = core
        .handle_request(&mut req)
        .await
        .expect("create token directly through the real Core");
    resp.and_then(|r| r.auth)
        .expect("auth/token/create must return an auth block")
        .client_token
}

#[derive(Default)]
struct NoopTokenAudit;
impl ProfileTokenAudit for NoopTokenAudit {
    fn record_mint(&self, _slug: &str, _accessor: &str, _ttl: Duration) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Recording guard-denial sink — used by `endpoint_core_has_the_guard_registered` to prove the
/// registered guard's sink actually fires, mirroring `profile_layer_independence.rs`'s own
/// `RecordingGuardAudit` pattern.
#[derive(Default)]
struct RecordingGuardAudit {
    denials: std::sync::Mutex<Vec<(String, String, String)>>,
}
impl ProfileGuardAudit for RecordingGuardAudit {
    fn record_denial(&self, slug: &str, path: &str, policy_name: &str) -> anyhow::Result<()> {
        self.denials
            .lock()
            .unwrap()
            .push((slug.to_string(), path.to_string(), policy_name.to_string()));
        Ok(())
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    core: Arc<Core>,
    root_token: String,
}

async fn open_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());

    let seal_config = SealConfig {
        secret_shares: 1,
        secret_threshold: 1,
    };
    let init_result = core.init(&seal_config).await.expect("core.init()");
    let unsealed = core
        .unseal(&init_result.secret_shares[0])
        .await
        .expect("core.unseal()");
    assert!(unsealed, "single-share unseal must return Ok(true)");
    let root_token = init_result.root_token.clone();

    ensure_profile_policy(&core, &root_token, "alpha")
        .await
        .expect("ensure alpha policy");
    ensure_profile_policy(&core, &root_token, "beta")
        .await
        .expect("ensure beta policy");
    write_secret_raw(&core, &root_token, "secret/profiles/alpha/openrouter", "sk-test-alpha").await;
    write_secret_raw(&core, &root_token, "secret/profiles/beta/openrouter", "sk-test-beta").await;

    Fixture {
        _tmp: tmp,
        core,
        root_token,
    }
}

async fn mint(core: &Arc<Core>, root_token: &str, slug: &str) -> SecretString {
    let sink = NoopTokenAudit;
    let minted: MintedProfileToken = mint_profile_token(
        core,
        &SecretString::from(root_token.to_string()),
        slug,
        profile_token_ttl_for_bootstrap(),
        &sink,
    )
    .await
    .expect("mint profile token");
    minted.token().clone()
}

/// Send one JSON request line over the socket and read back exactly one response line.
async fn send_request(socket_path: &Path, body: serde_json::Value) -> serde_json::Value {
    let stream = UnixStream::connect(socket_path)
        .await
        .expect("connect to endpoint socket");
    let (reader, mut writer) = stream.into_split();
    let line = format!("{}\n", body);
    writer
        .write_all(line.as_bytes())
        .await
        .expect("write request line");
    writer.flush().await.expect("flush request");
    let mut buf_reader = BufReader::new(reader);
    let mut resp_line = String::new();
    buf_reader
        .read_line(&mut resp_line)
        .await
        .expect("read response line");
    serde_json::from_str(resp_line.trim_end()).expect("response must be valid JSON")
}

// ---------------------------------------------------------------------------
// Task 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn socket_is_0600_immediately_after_bind() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());

    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let mode = std::fs::metadata(&path)
        .expect("read socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "socket must be owner-only read/write immediately after bind");

    handle.shutdown().await;
}

#[tokio::test]
async fn endpoint_returns_the_profiles_own_secret() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    let response = send_request(
        &path,
        serde_json::json!({
            "token": token.expose_secret(),
            "profile": "alpha",
            "key": "openrouter",
        }),
    )
    .await;

    assert_eq!(
        response,
        serde_json::json!({ "value": "sk-test-alpha" }),
        "response must be exactly the profile's own secret value"
    );

    // Minimality, asserted on its own (plan-check W-4): deserializing into a single-field
    // struct with deny_unknown_fields must succeed — a response carrying ANY additional field
    // (echoed path, policy name, token) would fail this deserialize.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct OnlyValue {
        #[allow(dead_code)]
        value: String,
    }
    let only_value: Result<OnlyValue, _> = serde_json::from_value(response.clone());
    assert!(
        only_value.is_ok(),
        "success response must deserialize into a single-field {{value}} struct: {response:?}"
    );
    assert_eq!(
        response.as_object().expect("object").len(),
        1,
        "success response must have exactly one key"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn request_naming_a_different_profile_than_the_token_is_refused() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let alpha_token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    let response = send_request(
        &path,
        serde_json::json!({
            "token": alpha_token.expose_secret(),
            "profile": "beta",
            "key": "openrouter",
        }),
    )
    .await;

    assert_eq!(
        response,
        serde_json::json!({ "error": "profile_mismatch" }),
        "a token for alpha naming profile beta must be refused with the distinct \
         profile_mismatch error, not a generic denial"
    );
    let raw = response.to_string();
    assert!(
        !raw.contains("sk-test-beta"),
        "beta's secret value must never appear in the response, got: {raw}"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn request_key_traversal_is_refused() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let alpha_token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    for bad_key in ["../beta/openrouter", "..", "%2e%2e", "a/b"] {
        let response = send_request(
            &path,
            serde_json::json!({
                "token": alpha_token.expose_secret(),
                "profile": "alpha",
                "key": bad_key,
            }),
        )
        .await;
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("invalid_key"),
            "traversal-shaped key {bad_key:?} must be refused as invalid_key, got: {response:?}"
        );
    }

    handle.shutdown().await;
}

#[tokio::test]
async fn endpoint_core_has_the_guard_registered() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink.clone(),
        path.clone(),
    )
    .expect("host endpoint");

    // Drive a cross-profile attempt directly through the endpoint's OWN Core (bypassing the
    // wire protocol's explicit mismatch check, which would refuse this before the guard is ever
    // reached) — proving the Plan 04 guard is independently registered and active on THIS
    // endpoint's Core, not merely that the wire-level mismatch check catches the common case.
    //
    // Mirrors `profile_layer_independence.rs`'s own "Run B" design: under alpha's NORMAL,
    // restrictive policy the native ACL (layer 2) would already deny this read before the
    // guard's post_auth is ever invoked (`TokenStore::pre_route`'s auth_handlers loop returns
    // on the FIRST non-ErrHandlerDefault result) — that would prove layer 2 works, not that
    // THIS endpoint's Core carries the guard. So this test overwrites alpha's policy with a
    // DELIBERATELY PERMISSIVE one the ACL would allow under, and mints a token against it
    // directly (bypassing `mint_profile_token`, which would re-register the restrictive policy)
    // — isolating the guard as the ONLY layer that can be denying this read.
    write_policy_raw(
        &fixture.core,
        &fixture.root_token,
        "profile-alpha",
        "path \"secret/profiles/*\" {\n  capabilities = [\"read\", \"list\"]\n}\n",
    )
    .await;
    let permissive_token = create_token_raw(&fixture.core, &fixture.root_token, &["profile-alpha"]).await;

    let result = read_secret_raw(
        &fixture.core,
        &permissive_token,
        "secret/profiles/beta/openrouter",
    )
    .await;

    assert!(
        result.is_err(),
        "a cross-profile read through the endpoint's Core must be denied, got: {result:?}"
    );
    assert!(
        !sink.denials.lock().unwrap().is_empty(),
        "the registered guard's sink must have recorded the denial"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn endpoint_shutdown_removes_the_socket_file() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    assert!(path.exists(), "socket file must exist while hosted");
    handle.shutdown().await;
    assert!(
        !path.exists(),
        "socket file must be removed after shutdown"
    );
}

#[tokio::test]
async fn endpoint_task_stops_accepting_after_shutdown() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");
    handle.shutdown().await;

    let connect_result = UnixStream::connect(&path).await;
    assert!(
        connect_result.is_err(),
        "connecting after shutdown must fail rather than hang or be served"
    );
}

// ---------------------------------------------------------------------------
// Task 3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn endpoint_exposes_only_the_read_verb() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let alpha_token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    let raw_token = alpha_token.expose_secret().to_string();

    let shaped_requests = vec![
        // write-shaped
        serde_json::json!({"token": raw_token, "profile": "alpha", "key": "openrouter", "value": "evil"}),
        // delete-shaped
        serde_json::json!({"token": raw_token, "profile": "alpha", "key": "openrouter", "op": "delete"}),
        // list-shaped (missing required "key")
        serde_json::json!({"token": raw_token, "profile": "alpha", "op": "list"}),
        // init-shaped
        serde_json::json!({"op": "init", "secret_shares": 1, "secret_threshold": 1}),
        // unseal-shaped
        serde_json::json!({"op": "unseal", "key": "deadbeef"}),
        // seal-status-shaped
        serde_json::json!({"op": "seal-status"}),
    ];

    for shaped in shaped_requests {
        let response = send_request(&path, shaped.clone()).await;
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("malformed_request"),
            "request shaped like {shaped:?} must be refused before reaching the vault, got: {response:?}"
        );
    }

    handle.shutdown().await;
}

#[tokio::test]
async fn oversized_request_is_refused_without_buffering_it() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let stream = UnixStream::connect(&path).await.expect("connect");
    let (reader, mut writer) = stream.into_split();
    // 16 KiB of padding plus the trailing newline, well past the 8 KiB cap, sent as ONE
    // write_all call so it lands in the kernel socket buffer atomically — a second, separate
    // write_all after the server has already detected the overflow and closed its side would
    // race a BrokenPipe, which is itself valid evidence of refusal but not what this test means
    // to assert (the size cap, not connection teardown timing).
    let mut oversized = vec![b'a'; 16 * 1024];
    oversized.push(b'\n');
    // Best-effort: tolerate a BrokenPipe here too — a fast server response can legitimately
    // race ahead of this write completing.
    let _ = writer.write_all(&oversized).await;
    let _ = writer.flush().await;

    let mut buf_reader = BufReader::new(reader);
    let mut resp_line = String::new();
    let read_outcome = tokio::time::timeout(
        Duration::from_secs(5),
        buf_reader.read_line(&mut resp_line),
    )
    .await
    .expect("must not hang waiting for the oversized-request response");
    match read_outcome {
        Ok(0) => {
            // Connection closed with no response body — also valid evidence the oversized
            // request was refused rather than buffered and served.
        }
        Ok(_) => {
            let response: serde_json::Value = serde_json::from_str(resp_line.trim_end())
                .expect("response must be valid JSON");
            assert_eq!(
                response,
                serde_json::json!({ "error": "request_too_large" }),
                "an oversized request line must be refused with request_too_large"
            );
        }
        Err(e) => panic!("reading the oversized-request response errored unexpectedly: {e}"),
    }

    handle.shutdown().await;
}

#[tokio::test]
async fn idle_connection_times_out() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let stream = UnixStream::connect(&path).await.expect("connect");
    let (reader, _writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut resp_line = String::new();
    // The endpoint's idle read timeout is 5s; give it a generous margin so this assertion
    // cannot be a timing flake, without holding the accept loop open indefinitely.
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        buf_reader.read_line(&mut resp_line),
    )
    .await
    .expect("the connection must be dropped well within 10s of the 5s idle timeout");

    // The server closes its write half on idle timeout with no response — read_line on a
    // closed connection returns Ok(0) (EOF).
    assert_eq!(
        result.expect("read must not error, just observe EOF"),
        0,
        "an idle client must observe EOF (connection dropped), not a response"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn concurrent_workers_are_served() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let alpha_token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    let beta_token = mint(&fixture.core, &fixture.root_token, "beta").await;
    let counter = Arc::new(AtomicUsize::new(0));

    let path_a = path.clone();
    let alpha_raw = alpha_token.expose_secret().to_string();
    let counter_a = Arc::clone(&counter);
    let task_a = tokio::spawn(async move {
        let response = send_request(
            &path_a,
            serde_json::json!({"token": alpha_raw, "profile": "alpha", "key": "openrouter"}),
        )
        .await;
        counter_a.fetch_add(1, Ordering::SeqCst);
        response
    });

    let path_b = path.clone();
    let beta_raw = beta_token.expose_secret().to_string();
    let counter_b = Arc::clone(&counter);
    let task_b = tokio::spawn(async move {
        let response = send_request(
            &path_b,
            serde_json::json!({"token": beta_raw, "profile": "beta", "key": "openrouter"}),
        )
        .await;
        counter_b.fetch_add(1, Ordering::SeqCst);
        response
    });

    let (resp_a, resp_b) = tokio::join!(task_a, task_b);
    assert_eq!(resp_a.expect("task a"), serde_json::json!({"value": "sk-test-alpha"}));
    assert_eq!(resp_b.expect("task b"), serde_json::json!({"value": "sk-test-beta"}));
    assert_eq!(counter.load(Ordering::SeqCst), 2, "both concurrent reads must succeed");

    handle.shutdown().await;
}

#[tokio::test]
async fn stale_socket_file_is_replaced_on_bind() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");

    // Simulate a leftover socket file from a previous, killed process: bind a first listener,
    // then leak it WITHOUT unlinking the file (a plain drop of a std UnixListener does not
    // remove the socket file).
    {
        let orphan = std::os::unix::net::UnixListener::bind(&path).expect("bind orphan listener");
        std::mem::forget(orphan);
    }
    assert!(path.exists(), "precondition: a stale socket file exists at the path");

    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint despite the stale socket file");

    let mode = std::fs::metadata(&path)
        .expect("read socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the REPLACEMENT socket must still be 0600");

    // Confirm the replacement is actually live and serving, not just present on disk.
    let alpha_token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    let response = send_request(
        &path,
        serde_json::json!({
            "token": alpha_token.expose_secret(),
            "profile": "alpha",
            "key": "openrouter",
        }),
    )
    .await;
    assert_eq!(response, serde_json::json!({"value": "sk-test-alpha"}));

    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Phase 51 Plan 11 (WR-05 / IN-06)
// ---------------------------------------------------------------------------

/// WR-05: a storage/backend-class fault during the token-policy lookup must surface as
/// `backend_error`, never `token_invalid` — an operator debugging a failed credential read
/// needs to know whether the TOKEN or the STORAGE is at fault. Constructed through the real
/// `Core`, not a stub: a real token is minted (which needs storage WRITE access), then the
/// vault's data dir is stripped of execute permission so the SAME token's next lookup can no
/// longer traverse into it. `auth/token/lookup-self`'s own `TokenStore::check_token` ->
/// `lookup` -> `Backend::get` is a bare `File::open` with no locking (unlike a WRITE, which
/// retries forever on any error via its `lock()` step — not viable for fault injection, see
/// `51-11-SUMMARY.md`), so this fails immediately with a permission error that
/// `map_rv_error`'s catch-all turns into `VaultError::Backend`.
#[tokio::test]
async fn backend_class_lookup_fault_surfaces_as_backend_error_not_invalid_token() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    // Mint FIRST — minting needs storage write access (ensure_profile_policy + the token
    // create write), which must happen before the data dir is locked down below.
    let token = mint(&fixture.core, &fixture.root_token, "alpha").await;

    // Break storage READS: strip execute permission from the vault's data dir so no path
    // underneath it can be traversed, regardless of depth.
    std::fs::set_permissions(fixture._tmp.path(), std::fs::Permissions::from_mode(0o600))
        .expect("lock down vault data dir");

    let response = send_request(
        &path,
        serde_json::json!({
            "token": token.expose_secret(),
            "profile": "alpha",
            "key": "openrouter",
        }),
    )
    .await;

    // Restore permissions before the TempDir cleans itself up on drop.
    std::fs::set_permissions(fixture._tmp.path(), std::fs::Permissions::from_mode(0o700))
        .expect("restore vault data dir permissions");

    assert_eq!(
        response,
        serde_json::json!({ "error": "backend_error" }),
        "a storage-level read fault must surface as backend_error, not token_invalid (WR-05)"
    );

    handle.shutdown().await;
}

/// IN-06: dropping a [`ironhermes_vault::ProfileCredentialEndpointHandle`] whose `Core` has
/// been sealed must not panic (and, since a panic during a non-unwinding drop would abort the
/// process, must not abort it either). `register_profile_guard`'s own doc note says the
/// sibling `add_auth_handler` path panics when the core is not unsealed
/// (`AuthModule::set_auth_handlers`'s `.unwrap()` on an absent `token_store`); `Core::seal()`
/// tears the auth module back down to that exact "not unsealed" state, so
/// `unregister_profile_guard`'s `delete_auth_handler` call hits the identical panic unless
/// `Drop`/`shutdown` guard it.
#[tokio::test]
async fn dropping_a_handle_whose_core_is_sealed_does_not_abort() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());

    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    fixture.core.seal().await.expect("seal the core");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(handle)));
    assert!(
        result.is_ok(),
        "dropping a handle whose Core is sealed must not panic (and must not abort the process)"
    );
}

// ---------------------------------------------------------------------------
// Task 3 (Phase 51 Plan 17, IN-05): bounded in-flight connections.
//
// `64` mirrors `profile_endpoint.rs`'s private `MAX_IN_FLIGHT_CONNECTIONS`
// constant directly (this file cannot import a module-private const) — the
// same "duplicate the production magic number as a documented literal"
// precedent `idle_connection_times_out` above already establishes for the
// 5-second `IDLE_READ_TIMEOUT`.
// ---------------------------------------------------------------------------

const MAX_IN_FLIGHT_CONNECTIONS_MIRROR: usize = 64;

/// Half one of the concurrency-bound assertion: legitimate concurrent bootstraps up to (and
/// including) the bound must ALL succeed — the bound must never become a functional limit on
/// the one operation this endpoint exists for.
#[tokio::test]
async fn up_to_the_concurrency_bound_all_succeed() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    let alpha_token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    let alpha_raw = alpha_token.expose_secret().to_string();

    let mut tasks = Vec::with_capacity(MAX_IN_FLIGHT_CONNECTIONS_MIRROR);
    for _ in 0..MAX_IN_FLIGHT_CONNECTIONS_MIRROR {
        let path = path.clone();
        let alpha_raw = alpha_raw.clone();
        tasks.push(tokio::spawn(async move {
            send_request(
                &path,
                serde_json::json!({"token": alpha_raw, "profile": "alpha", "key": "openrouter"}),
            )
            .await
        }));
    }

    for task in tasks {
        let response = task.await.expect("task must not panic");
        assert_eq!(
            response,
            serde_json::json!({"value": "sk-test-alpha"}),
            "every one of the {MAX_IN_FLIGHT_CONNECTIONS_MIRROR} concurrent bootstraps (exactly \
             at the bound) must succeed"
        );
    }

    handle.shutdown().await;
}

/// Half two of the concurrency-bound assertion: a bound nothing tests is a bound someone will
/// raise to infinity. Saturates every permit with idle (never-write) connections, proves a
/// connection past the bound gets no response while all permits are held, then frees one and
/// proves that same connection is served once a permit becomes available.
#[tokio::test]
async fn connection_past_the_concurrency_bound_waits_for_a_free_slot() {
    let fixture = open_fixture().await;
    let tmp_sock_dir = tempfile::tempdir().expect("create temp socket dir");
    let path = tmp_sock_dir.path().join("endpoint.sock");
    let sink: Arc<dyn ProfileGuardAudit> = Arc::new(RecordingGuardAudit::default());
    let handle = spawn_profile_credential_endpoint_at(
        Arc::clone(&fixture.core),
        SecretString::from(fixture.root_token.clone()),
        sink,
        path.clone(),
    )
    .expect("host endpoint");

    // Saturate every permit: connect but never write a byte. The accept loop still spawns
    // `handle_connection` for each (acquiring a permit first), which then blocks inside its own
    // `read_bounded_line` waiting for data that never arrives — holding the permit until this
    // stream is dropped or the server's own 5s idle timeout fires.
    let mut saturating_streams = Vec::with_capacity(MAX_IN_FLIGHT_CONNECTIONS_MIRROR);
    for _ in 0..MAX_IN_FLIGHT_CONNECTIONS_MIRROR {
        saturating_streams.push(
            UnixStream::connect(&path)
                .await
                .expect("connect a saturating (idle) connection"),
        );
    }

    let alpha_token = mint(&fixture.core, &fixture.root_token, "alpha").await;
    let alpha_raw = alpha_token.expose_secret().to_string();

    // One more connection, past the bound: every permit is held by a saturating connection
    // above, so the accept loop itself is now blocked acquiring a permit for THIS connection —
    // it must not get a response within a short window.
    let overflow_path = path.clone();
    let overflow_raw = alpha_raw.clone();
    let mut overflow_task = tokio::spawn(async move {
        send_request(
            &overflow_path,
            serde_json::json!({"token": overflow_raw, "profile": "alpha", "key": "openrouter"}),
        )
        .await
    });

    let premature = tokio::time::timeout(Duration::from_millis(500), &mut overflow_task).await;
    assert!(
        premature.is_err(),
        "a connection past the concurrency bound must not be served while every permit is \
         held — the bound is not being applied at all"
    );

    // Free exactly one permit — drop the LAST saturating connection. The server's
    // `handle_connection` observes a clean EOF on its next `fill_buf`, breaks its loop, and
    // drops its `_permit`, returning it to the semaphore.
    saturating_streams.pop();

    let response = tokio::time::timeout(Duration::from_secs(5), overflow_task)
        .await
        .expect("the queued connection must be served promptly once a permit frees")
        .expect("task must not panic");
    assert_eq!(
        response,
        serde_json::json!({"value": "sk-test-alpha"}),
        "the connection queued past the bound must succeed once a slot frees"
    );

    handle.shutdown().await;
}
