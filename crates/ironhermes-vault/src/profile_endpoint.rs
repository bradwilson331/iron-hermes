//! The single, worker-facing per-profile credential endpoint (Phase 51 D-11/D-12/D-13/D-14) —
//! a `0600` Unix-domain-socket listener with exactly one verb (read), hosted wherever the
//! kanban dispatcher runs.
//!
//! # WIDENED PROTOCOL — D-11/D-12/D-13's original locked shape was REVERSED by explicit user
//! decision at this plan's Task 1 checkpoint (2026-09-10)
//!
//! `51-CONTEXT.md`'s D-12 originally locked a request shape with **no field capable of naming
//! a profile** — cross-profile addressing was meant to be unrepresentable by the wire format
//! itself. At Task 1's `checkpoint:decision` gate the user explicitly selected the **`widen`**
//! option instead of the plan's own recommended `proceed` option: *"Build the endpoint with a
//! profile/path field in the request... this is a deliberate, user-approved REVERSAL of locked
//! decisions D-11/D-12/D-13."*
//!
//! **What changed:** [`ProfileCredentialRequest`] carries an explicit `profile` field. A
//! caller CAN now name a profile in the request.
//!
//! **What did NOT change** (explicit user constraints, all honored below):
//! - Transport stays the `0600` Unix domain socket (D-13) — no TCP, no change.
//! - Still read-only — one verb, no write/delete/list/init/unseal/seal-status path exists in
//!   this schema (enforced structurally: this module's request type has no field or variant
//!   that could express any of those, and `#[serde(deny_unknown_fields)]` refuses anything that
//!   doesn't parse into the exact `{token, profile, key}` shape before any vault call is made —
//!   see [`ProfileCredentialRequest`]).
//! - Bootstrap-only token minting, no re-mint (D-11/D-15 as corrected in `51-03-SUMMARY.md`) —
//!   this module mints nothing; it only ever verifies and reads.
//! - **Because shape no longer prevents cross-profile addressing, validation is now
//!   load-bearing, built explicitly, not implied:** the server derives the AUTHORITATIVE
//!   profile from the presented token's own `profile-{slug}` policy (via `auth/token/lookup-self`,
//!   which every token is granted access to under RustyVault's own `DEFAULT_POLICY` —
//!   [`lookup_token_policies`]), exactly the way the Plan 04 `ProfileGuard` derives its own
//!   prefix. If the request's `profile` field does not match the token's own derived slug, the
//!   request is refused with the distinct, named `"profile_mismatch"` error — never silently
//!   coerced to the token's real profile, and never confused with the generic `"denied"` error
//!   a native-ACL/layer-3-guard refusal produces. See [`derive_slug_from_policies`] and
//!   [`handle_line`].
//! - **Both D-08 enforcement layers proven in `51-04-SUMMARY.md` remain unweakened and are
//!   exercised on every read this endpoint serves**, backing the explicit mismatch check as a
//!   second, independent line of defense: [`spawn_profile_credential_endpoint`] registers the
//!   Plan 04 [`crate::ProfileGuard`] (an `AuthHandler::post_auth` implementation — never
//!   `Handler::pre_route`, which `51-04-SUMMARY.md` proved leaks a cross-profile read,
//!   `Ok(Some("sk-test-beta"))`, when registered at construction time) on the endpoint's own
//!   `Core`, and every read is issued through
//!   [`crate::profile_store::ProfileSecretStore::read_profile_secret_with_token`] under the
//!   CALLER's presented token — never the root token — so the native ACL (layer 2) evaluates
//!   the real, caller-presented credential on every single request, not merely on the
//!   explicit-mismatch fast path above.
//!
//! `51-CONTEXT.md`'s D-11/D-12/D-13 entries — and this plan's own original `must_haves` truth
//! `T-51-29` / prohibition citing "no field capable of naming another profile" — are now STALE
//! for the specific "no profile field" claim; see `51-06-SUMMARY.md`'s "Deviations" section for
//! the full reconciliation against every must_have this reversal can no longer satisfy
//! literally. Plan 07's worker client MUST be built against this widened `{token, profile,
//! key}` request shape, not the original two-field one.
//!
//! # In-repo precedent this module is modeled on
//!
//! `crates/ironhermes-exec/src/rpc_server.rs` — bare `tokio::net::UnixListener`,
//! `stream.into_split()`, `BufReader::lines()`-style newline-delimited JSON, one response line
//! per request, flush. That precedent has NO `chmod`/`set_permissions` call anywhere (it gets
//! away with it because its socket lives in a single-use `tempfile::TempDir`); this module
//! closes that gap explicitly — `bind_socket` calls `set_permissions` immediately after `bind`,
//! before any worker can connect.
//!
//! # Do NOT serve RustyVault's own HTTP module (D-12)
//!
//! At the pinned rev the `sys/` backend declares `init`, `unseal` and `seal-status` as
//! unauthenticated paths. This module's request type has no way to express any of those three
//! operations — `#[serde(deny_unknown_fields)]` on [`ProfileCredentialRequest`] refuses any
//! line that does not parse into the exact `{token, profile, key}` shape, so a request shaped
//! like any other vault operation is refused by `serde_json::from_slice` itself, before the
//! vault is ever consulted. Init and unseal remain in-process only, never over any socket.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusty_vault::core::Core;
use rusty_vault::logical::Request;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;

use crate::error::VaultError;
use crate::profile_client::{MAX_REQUEST_LINE_BYTES, read_bounded_line};
use crate::profile_guard::{ProfileGuard, ProfileGuardAudit, register_profile_guard, unregister_profile_guard};
use crate::profile_paths::profile_secret_prefix;
use crate::profile_store::ProfileSecretStore;
use crate::rusty_vault_store::{RustyVaultStore, run_request};

// ---------------------------------------------------------------------------
// Bounds (T-51-32 DoS mitigation)
// ---------------------------------------------------------------------------
//
// Phase 51 Plan 17 (IN-02): `MAX_REQUEST_LINE_BYTES` and `read_bounded_line`
// moved to `profile_client.rs` — the one module in this crate that is
// deliberately UNCONDITIONAL (no `rusty-vault` feature gate, see its module
// doc), so the worker-side client's response read can derive the identical
// cap and reuse the identical bounded-reading algorithm this server module
// uses for requests, without this feature-gated module becoming a build
// dependency of that unconditional one.

/// A connected-but-silent client is dropped after this long, so it cannot hold a worker slot in
/// the accept loop's per-connection task pool indefinitely.
const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Phase 51 Plan 17 (IN-05): bounds in-flight connections on the credential socket. The accept
/// loop acquires a permit from a semaphore of this size BEFORE spawning each connection's task
/// — beyond this many concurrent connections, `accept_loop` itself blocks rather than spawning
/// an unbounded task per acceptor. Same-uid-only via the `0600` socket (T-51-94), so the blast
/// radius of exceeding legitimate use is local resource exhaustion, not a remote DoS.
///
/// Not derived from any existing named constant — none exists for "how many kanban workers
/// might bootstrap concurrently" in this codebase. Chosen generously above any plausible
/// single-dispatch-tick concurrent-spawn count for a single-operator install (each worker's
/// bootstrap is one short-lived, single-request connection, per `profile_client.rs`'s own doc),
/// while still bounding a runaway or malfunctioning same-uid process from opening unbounded
/// connections.
const MAX_IN_FLIGHT_CONNECTIONS: usize = 64;

/// macOS `sockaddr_un.sun_path` is 104 bytes — tighter than Linux's 108. Checked explicitly
/// (never silently truncated) so an overlong `TMPDIR` fails with a clear message instead of a
/// cryptic OS bind error.
const SUN_PATH_MAX_BYTES: usize = 104;

/// The RustyVault policy-name convention this module derives the caller's AUTHORITATIVE profile
/// from — mirrors `profile_guard.rs`'s own constant.
const PROFILE_POLICY_PREFIX: &str = "profile-";

// ---------------------------------------------------------------------------
// Wire format (WIDENED — see module doc)
// ---------------------------------------------------------------------------

/// The one request shape this endpoint accepts. `#[serde(deny_unknown_fields)]` is the
/// mechanism that keeps this a single-verb, read-only endpoint (D-12): any line carrying an
/// extra field (an `"op"`, a `"value"` to write, anything shaped like `sys/init` /
/// `sys/unseal` / `sys/seal-status`) fails to deserialize into this exact shape and is refused
/// before the vault is ever consulted — see [`handle_line`].
///
/// `profile` is the WIDENED field (see module doc) — the caller MAY name a profile, but it is
/// checked against the presented token's own derived slug, never trusted to address the read.
/// The actual read always uses the TOKEN-DERIVED slug, never this field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileCredentialRequest {
    /// The caller's minted, profile-scoped token (Plan 03/07).
    token: String,
    /// The profile the caller believes it is addressing — checked, never trusted.
    profile: String,
    /// The provider/secret leaf name within that profile's own subtree.
    key: String,
}

/// Named, greppable error strings. Never echoes request content (the prohibition in this
/// plan's `must_haves`) — every string here is a static, pre-written literal.
mod error_names {
    pub const MALFORMED_REQUEST: &str = "malformed_request";
    pub const TOKEN_INVALID: &str = "token_invalid";
    pub const PROFILE_MISMATCH: &str = "profile_mismatch";
    pub const SECRET_NOT_FOUND: &str = "secret_not_found";
    pub const DENIED: &str = "denied";
    pub const TOKEN_EXPIRED: &str = "token_expired";
    pub const VAULT_UNREACHABLE: &str = "vault_unreachable";
    pub const INVALID_KEY: &str = "invalid_key";
    pub const BACKEND_ERROR: &str = "backend_error";
    pub const REQUEST_TOO_LARGE: &str = "request_too_large";
}

/// Success response: exactly one field. `endpoint_returns_the_profiles_own_secret` (the test
/// suite) asserts this minimality on its own — deserializing into a `#[serde(deny_unknown_fields)]`
/// single-field struct, or asserting the parsed JSON object has exactly one key — so this
/// response NEVER gains a second field (echoed path, policy name, or token) without that test
/// catching it.
fn ok_response(value: &str) -> String {
    serde_json::json!({ "value": value }).to_string()
}

/// Error response: exactly one field, a static named string — never echoes request content.
fn err_response(error: &str) -> String {
    serde_json::json!({ "error": error }).to_string()
}

// ---------------------------------------------------------------------------
// Socket path + bind (D-13)
// ---------------------------------------------------------------------------

/// A short path under the system temp directory — NEVER under the IronHermes home (this crate
/// cannot even reference `get_hermes_home`, the zero-cycle D-01 direction, so this is enforced
/// structurally, not merely by convention). Length is checked explicitly against the tighter
/// macOS `sun_path` bound; an overflow fails with a clear error naming the limit rather than a
/// cryptic OS bind failure.
pub fn socket_path() -> Result<PathBuf, VaultError> {
    let dir = std::env::temp_dir();
    let name = format!("ihvc-{}.sock", std::process::id());
    let path = dir.join(name);
    let len = path.to_string_lossy().len();
    if len >= SUN_PATH_MAX_BYTES {
        return Err(VaultError::Backend(format!(
            "profile credential endpoint: socket path is {len} bytes, at or beyond the \
             {SUN_PATH_MAX_BYTES}-byte sun_path bound (macOS) — {}: set a shorter TMPDIR",
            path.display()
        )));
    }
    Ok(path)
}

/// Bind the socket and immediately `chmod` it to owner-only read/write — the two calls stay
/// adjacent with nothing that can connect in between, closing the gap the in-repo
/// `rpc_server.rs` precedent leaves open (see module doc). A leftover socket file from a killed
/// process (T-51-32b) does not prevent binding: it is removed first, and the replacement is
/// still `0600`.
fn bind_socket(path: &Path) -> Result<UnixListener, VaultError> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| {
            VaultError::Backend(format!(
                "profile credential endpoint: could not remove stale socket file {}: {e}",
                path.display()
            ))
        })?;
    }
    let listener = UnixListener::bind(path).map_err(|e| {
        VaultError::Backend(format!(
            "profile credential endpoint: bind {} failed: {e}",
            path.display()
        ))
    })?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        VaultError::Backend(format!(
            "profile credential endpoint: chmod {} failed: {e}",
            path.display()
        ))
    })?;
    Ok(listener)
}

// ---------------------------------------------------------------------------
// Token -> authoritative profile derivation (mirrors profile_guard.rs's own logic)
// ---------------------------------------------------------------------------

/// Look up the presented token's OWN resolved policy set via `auth/token/lookup-self` — a path
/// every token is granted under RustyVault's own `DEFAULT_POLICY` (verified in
/// `profile_token.rs`'s module doc: `auth/token/lookup-self` is one of the six capabilities
/// `DEFAULT_POLICY` grants unconditionally). An unrecognized or expired-and-swept token fails
/// `TokenStore::check_token` before this even reaches `handle_lookup_self`, surfacing as
/// [`VaultError::Denied`] via [`run_request`]'s existing mapping — this function never needs
/// its own denial-detection logic.
async fn lookup_token_policies(core: &Arc<Core>, token: &str) -> Result<Vec<String>, VaultError> {
    let core = Arc::clone(core);
    let token_owned = token.to_string();
    let data = run_request(core, move || {
        let mut req = Request::new_read_request("auth/token/lookup-self");
        req.client_token = token_owned;
        req
    })
    .await?;
    let data = data.ok_or(VaultError::Denied)?;
    let policies = data
        .get("policies")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            VaultError::Backend("auth/token/lookup-self response missing \"policies\" field".to_string())
        })?;
    Ok(policies
        .iter()
        .filter_map(JsonValue::as_str)
        .map(str::to_string)
        .collect())
}

/// Derive the AUTHORITATIVE profile slug from a resolved policy set — exactly the shape
/// `ProfileGuard::post_auth` requires: exactly one `profile-`-prefixed policy, whose slug also
/// passes [`profile_secret_prefix`]'s own validation. Zero, two-or-more, or a malformed slug
/// all return `None` (folded into the `"token_invalid"` error by the caller) — this endpoint's
/// own copy of the same derivation `profile_guard.rs` performs, so the mismatch check below is
/// meaningful even before the request ever reaches the registered guard.
fn derive_slug_from_policies(policies: &[String]) -> Option<String> {
    let mut matches = policies.iter().filter(|p| p.starts_with(PROFILE_POLICY_PREFIX));
    let policy_name = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let slug = &policy_name[PROFILE_POLICY_PREFIX.len()..];
    profile_secret_prefix(slug).ok()?;
    Some(slug.to_string())
}

// ---------------------------------------------------------------------------
// Per-request handling
// ---------------------------------------------------------------------------

/// Handle one parsed request line end to end: derive the caller's authoritative profile from
/// its token, refuse a named-profile mismatch (the WIDENED protocol's load-bearing check — see
/// module doc), then read through
/// [`ProfileSecretStore::read_profile_secret_with_token`] under the CALLER's own token — never
/// the root token, so the native ACL (D-08 layer 2) and the registered [`ProfileGuard`] (layer
/// 3) both evaluate the real presented credential on every read, not only on this fast-path
/// check.
async fn handle_line(core: &Arc<Core>, store: &Arc<ProfileSecretStore>, line: &[u8]) -> String {
    let request: ProfileCredentialRequest = match serde_json::from_slice(line) {
        Ok(r) => r,
        Err(_) => return err_response(error_names::MALFORMED_REQUEST),
    };

    let policies = match lookup_token_policies(core, &request.token).await {
        Ok(p) => p,
        // WR-05 (Phase 51 Plan 11): three explicit, exhaustive-by-wildcard arms — the
        // denial/expiry pair (an actually-bad token), the sealed/uninitialized pair (the
        // vault itself is unreachable), and everything else (an opaque backend/storage
        // fault, notably `VaultError::Backend` — including the missing-`"policies"`-field
        // error `lookup_token_policies` itself constructs) to its own distinct name. No arm
        // here may route a backend-class error to the invalid-token name — that conflation
        // is exactly what hid a real storage fault behind "bad credential" for an operator
        // debugging a failed read.
        Err(VaultError::Denied) | Err(VaultError::TokenExpired) => {
            return err_response(error_names::TOKEN_INVALID);
        }
        Err(VaultError::Sealed) | Err(VaultError::NotInitialized) => {
            return err_response(error_names::VAULT_UNREACHABLE);
        }
        Err(_) => return err_response(error_names::BACKEND_ERROR),
    };

    let Some(slug) = derive_slug_from_policies(&policies) else {
        return err_response(error_names::TOKEN_INVALID);
    };

    // WIDENED protocol's explicit check (user decision, 2026-09-10): the request's named
    // profile must match the token's own derived slug — a distinct, named error, never the
    // generic "denied" a downstream ACL/guard refusal produces.
    if slug != request.profile {
        return err_response(error_names::PROFILE_MISMATCH);
    }

    let token = SecretString::from(request.token.clone());
    match store.read_profile_secret_with_token(&token, &slug, &request.key).await {
        Ok(Some(value)) => ok_response(value.expose_secret()),
        Ok(None) => err_response(error_names::SECRET_NOT_FOUND),
        Err(VaultError::Denied) => err_response(error_names::DENIED),
        Err(VaultError::TokenExpired) => err_response(error_names::TOKEN_EXPIRED),
        Err(VaultError::Sealed) | Err(VaultError::NotInitialized) => {
            err_response(error_names::VAULT_UNREACHABLE)
        }
        Err(VaultError::InvalidKey(_)) => err_response(error_names::INVALID_KEY),
        Err(_) => err_response(error_names::BACKEND_ERROR),
    }
}

// ---------------------------------------------------------------------------
// Bounded, newline-delimited connection handling (T-51-32)
// ---------------------------------------------------------------------------

/// Serve one connection: bounded, timed-out reads; one response line per request; concurrent
/// with every other connection (this function runs inside its own `tokio::spawn`'d task per
/// [`accept_loop`], never serialized against sibling connections — T-51-32).
async fn handle_connection(stream: UnixStream, core: Arc<Core>, store: Arc<ProfileSecretStore>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        let outcome = tokio::time::timeout(
            IDLE_READ_TIMEOUT,
            read_bounded_line(&mut reader, MAX_REQUEST_LINE_BYTES),
        )
        .await;
        let line = match outcome {
            Ok(Ok(Some(bytes))) => bytes,
            Ok(Ok(None)) => break, // clean EOF — client closed
            Ok(Err(_)) => {
                let resp = err_response(error_names::REQUEST_TOO_LARGE);
                let _ = writer.write_all(resp.as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
                let _ = writer.flush().await;
                break;
            }
            Err(_) => break, // idle read timeout — drop silently, no response owed
        };

        let response = handle_line(&core, &store, &line).await;
        if writer.write_all(response.as_bytes()).await.is_err() {
            break;
        }
        if writer.write_all(b"\n").await.is_err() {
            break;
        }
        if writer.flush().await.is_err() {
            break;
        }
    }
}

/// Accept loop: cancellable via `cancel`, one `tokio::spawn`'d task per accepted connection so
/// sibling workers are served concurrently (T-51-32 — a serialized implementation would pass
/// every single-client test and then quietly serialize dispatch).
async fn accept_loop(
    listener: UnixListener,
    core: Arc<Core>,
    store: Arc<ProfileSecretStore>,
    cancel: CancellationToken,
) {
    // Phase 51 Plan 17 (IN-05): one permit per in-flight connection, acquired BEFORE the spawn
    // below — beyond `MAX_IN_FLIGHT_CONNECTIONS`, this loop itself blocks on `acquire_owned`
    // rather than spawning an unbounded task per accepted connection. The permit moves into the
    // spawned task and is released (back to this semaphore) when that task completes.
    let connection_slots = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT_CONNECTIONS));
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        // `acquire_owned` on a `Semaphore` we never `close()` only ever returns
                        // `Err` if the semaphore itself was dropped — which cannot happen while
                        // this loop (which holds `connection_slots`) is still running.
                        let permit = Arc::clone(&connection_slots)
                            .acquire_owned()
                            .await
                            .expect("connection_slots is never closed while accept_loop runs");
                        let core = Arc::clone(&core);
                        let store = Arc::clone(&store);
                        tokio::spawn(async move {
                            let _permit = permit;
                            handle_connection(stream, core, store).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "profile credential endpoint: accept error");
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handle + lifecycle (T-51-32b)
// ---------------------------------------------------------------------------

/// A running endpoint. Dropping (or explicitly [`ProfileCredentialEndpointHandle::shutdown`]-ing)
/// this handle cancels the accept loop's task and removes the socket file, so a restarted
/// dispatcher binds fresh rather than inheriting an orphaned listener (T-51-32b — this repo has
/// a documented history of exactly that faking an intermittent bug).
pub struct ProfileCredentialEndpointHandle {
    socket_path: PathBuf,
    cancel: CancellationToken,
    core: Arc<Core>,
    guard: Arc<ProfileGuard>,
    /// The root token this endpoint was hosted with (Phase 51 Plan 11, CR-06). Retained so a
    /// mint against this handle's own `Core` (see [`Self::core`]) needs no second, independent
    /// `Core` — see [`crate::mint_profile_token_for_host`]. Never derive or hand-write
    /// `Debug`/`Display` on this struct: that would render this field (T-51-68).
    root_token: SecretString,
}

impl ProfileCredentialEndpointHandle {
    /// The bound socket path — Plan 07's worker client connects here (passed to the worker via
    /// its spawn environment, e.g. `IRONHERMES_KANBAN_VAULT_SOCKET`).
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Crate-internal access to the hosting `Core` (Phase 51 Plan 11, CR-06). CR-06 found that
    /// the earlier claim — a call site holding this handle "cannot reach" its `Core` — was false
    /// in-crate: the field was always plain-private, never actually inaccessible to code living
    /// in this crate. `pub(crate)` makes that access intentional without widening the public API
    /// past the crate boundary (D-12's minimal-surface posture).
    pub(crate) fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// Crate-internal access to the root token this endpoint was hosted with. Same rationale and
    /// boundary as [`Self::core`].
    pub(crate) fn root_token(&self) -> &SecretString {
        &self.root_token
    }

    /// Explicit, graceful shutdown: cancels the accept loop, unregisters the layer-3 guard from
    /// this endpoint's `Core`, yields once so the accept loop's `select!` observes cancellation,
    /// then removes the socket file. Idempotent-safe to call even if the accept task has
    /// already exited on its own.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        // IN-06 (Phase 51 Plan 11): `register_profile_guard`'s doc warns the sibling
        // `add_auth_handler` path panics when the core is not unsealed; skip the
        // unregister call entirely on a sealed core rather than risk that panic reaching an
        // unwind. Socket removal and cancellation stay unconditional either way — the
        // T-51-32b orphaned-listener control must not be weakened by this guard.
        if !self.core.sealed() {
            let _ = unregister_profile_guard(&self.core, &self.guard);
        }
        tokio::task::yield_now().await;
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Drop for ProfileCredentialEndpointHandle {
    /// Defensive fallback for an unexpectedly-dropped (rather than explicitly shut down) handle
    /// — cancels the task and removes the socket file synchronously. `unregister_profile_guard`
    /// is a plain sync fn, so this is Drop-safe with no async-drop workaround needed.
    fn drop(&mut self) {
        self.cancel.cancel();
        // IN-06: see the identical guard in `shutdown` above — a panic during a
        // non-unwinding `Drop` aborts the process, so this must never even attempt the call
        // on a sealed core.
        if !self.core.sealed() {
            let _ = unregister_profile_guard(&self.core, &self.guard);
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Low-level: host the endpoint on an ALREADY-open, unsealed `core` at the default
/// [`socket_path`]. Registers the Plan 04 [`ProfileGuard`] on `core` (must already be unsealed —
/// `Core::add_auth_handler` panics otherwise, matching `51-04-SUMMARY.md`'s documented
/// precondition), binds and `chmod`s the socket, and spawns the accept loop. Requires an active
/// Tokio runtime on the calling thread (`tokio::spawn` inside).
pub fn spawn_profile_credential_endpoint(
    core: Arc<Core>,
    root_token: SecretString,
    guard_sink: Arc<dyn ProfileGuardAudit>,
) -> Result<ProfileCredentialEndpointHandle, VaultError> {
    spawn_profile_credential_endpoint_at(core, root_token, guard_sink, socket_path()?)
}

/// Same as [`spawn_profile_credential_endpoint`], but at an explicit `path` — the test seam
/// every behavior test in `tests/profile_endpoint.rs` uses so parallel test processes never
/// collide on the PID-derived default path, and so the stale-socket-replacement test can bind
/// twice at a known, fixed location.
#[doc(hidden)]
pub fn spawn_profile_credential_endpoint_at(
    core: Arc<Core>,
    root_token: SecretString,
    guard_sink: Arc<dyn ProfileGuardAudit>,
    path: PathBuf,
) -> Result<ProfileCredentialEndpointHandle, VaultError> {
    // Cloned BEFORE the move into `ProfileSecretStore::from_core` below (Phase 51 Plan 11,
    // CR-06) — the handle retains its own copy so a mint against this endpoint's `Core` needs
    // no second, independent store.
    let root_token_for_handle = root_token.clone();
    let profile_store = Arc::new(ProfileSecretStore::from_core(Arc::clone(&core), root_token));
    let guard = register_profile_guard(&core, guard_sink)?;
    let listener = bind_socket(&path)?;

    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let core_for_task = Arc::clone(&core);
    tokio::spawn(async move {
        accept_loop(listener, core_for_task, profile_store, cancel_for_task).await;
    });

    Ok(ProfileCredentialEndpointHandle {
        socket_path: path,
        cancel,
        core,
        guard,
        root_token: root_token_for_handle,
    })
}

/// High-level, config-driven host (D-14): opens the vault from `rv_config`, and — only if it is
/// reachable and unsealed — hosts the endpoint. Returns `None` (never panics, never partially
/// hosts) on ANY failure: unopenable (uninitialized/opaque backend error), sealed, or a guard/
/// socket-bind failure. This is what `ironhermes-core`'s `profile_credentials` facade calls;
/// `host_yields_no_endpoint_when_vault_unavailable` proves the unreachable/sealed arms directly
/// against this function.
pub fn host_profile_credential_endpoint(
    rv_config: &crate::config::RustyVaultConfig,
    guard_sink: Arc<dyn ProfileGuardAudit>,
) -> Option<ProfileCredentialEndpointHandle> {
    let store = match RustyVaultStore::open(rv_config) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "profile credential endpoint: vault unavailable, hosting no endpoint (D-14)"
            );
            return None;
        }
    };
    match store.is_sealed() {
        Ok(false) => {}
        Ok(true) => {
            tracing::warn!("profile credential endpoint: vault sealed, hosting no endpoint (D-14)");
            return None;
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "profile credential endpoint: could not read seal state, hosting no endpoint (D-14)"
            );
            return None;
        }
    }

    let core = store.core_handle();
    let root_token = store.root_token_secret();
    match spawn_profile_credential_endpoint(core, root_token, guard_sink) {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "profile credential endpoint: failed to host, hosting no endpoint (D-14)"
            );
            None
        }
    }
}

/// Production [`ProfileGuardAudit`] sink — records a denial via `tracing::warn!` rather than
/// the trajectory ledger. `51-04-SUMMARY.md`'s "Next Phase Readiness" anticipated this plan
/// adapting `ProfileGuardAudit` (and Plan 03's `ProfileTokenAudit`) onto the same trajectory
/// writer `ironhermes-kanban`'s `DispatcherContext` does not have a handle to (it depends on
/// neither `ironhermes-vault` nor `ironhermes-trajectory` — see `profile_token.rs`'s module
/// doc). Wiring the real trajectory ledger in is deferred; see `51-06-SUMMARY.md`'s
/// "Deviations" section. The sink is still REQUIRED (not `Option`) and still exercised on every
/// denial — this defers WHERE the record goes, not WHETHER one is made.
pub struct TracingProfileGuardAudit;

impl ProfileGuardAudit for TracingProfileGuardAudit {
    fn record_denial(&self, slug: &str, path: &str, policy_name: &str) -> anyhow::Result<()> {
        tracing::warn!(
            target: "ironhermes_profile_credential_endpoint",
            slug = slug,
            path = path,
            policy_name = policy_name,
            "profile credential endpoint: cross-profile read denied by layer-3 guard"
        );
        Ok(())
    }
}
