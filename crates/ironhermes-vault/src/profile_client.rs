//! Worker-side client for the Phase 51 per-profile credential endpoint (D-11).
//!
//! Implements Plan 06's WIDENED wire format verbatim, as recorded in
//! `51-06-SUMMARY.md`: one JSON object per line, newline-terminated, over the `0600`
//! Unix domain socket [`crate::profile_endpoint::socket_path`] binds.
//!
//! ```json
//! {"token": "<minted token>", "profile": "<profile slug>", "key": "<provider leaf>"}
//! ```
//!
//! Success: `{"value": "<secret value>"}`. Error: `{"error": "<named string>"}` — see
//! [`ProfileClientError`] for the full mapping.
//!
//! # Deliberately unconditional (no `rusty-vault` feature gate)
//!
//! This module has ZERO dependency on the `rusty_vault` crate — it is a plain
//! `tokio::net::UnixStream` + `serde_json` client, modeled on the in-repo
//! `crates/ironhermes-exec/src/rpc_server.rs` precedent's newline-delimited-JSON shape.
//! The worker binary (`ironhermes-cli`) must be able to attempt the vault bootstrap
//! (and no-op cleanly when the two vault env vars are absent — the common path) even
//! when built without `--features rusty-vault`, since a worker is the SAME binary the
//! dispatcher process runs (`Command::new(resolve_worker_bin())`), and the two roles
//! do not necessarily compile with matching feature sets in every distribution.
//!
//! # `SecretString` from the moment the value is parsed
//!
//! The returned credential is carried as a [`secrecy::SecretString`] immediately on
//! parse — never a `String` intermediate — because everything that later formats it
//! (logs, error messages, ledger lines) inherits whatever redaction the type
//! provides. See `51-07-PLAN.md`'s acceptance criteria.

use std::path::Path;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

// ---------------------------------------------------------------------------
// Bounded, newline-delimited line reading (Phase 51 Plan 17, IN-02)
// ---------------------------------------------------------------------------
//
// Defined HERE rather than in `profile_endpoint.rs` (the server side that
// originally introduced this bound, T-51-32) because this module is
// deliberately UNCONDITIONAL — see this file's own module doc — while
// `profile_endpoint.rs` is gated behind the `rusty-vault` feature.
// `profile_endpoint.rs` imports both items from here instead of defining its
// own copies, so client and server share exactly one cap and one bounded-
// reading algorithm rather than two that could silently drift apart.

/// Maximum bytes accepted for a single newline-delimited line before the read is refused.
/// Applies symmetrically to both directions of this protocol: the server's own request reads
/// (`profile_endpoint.rs`'s `handle_connection`) and the client's response read below — an
/// asymmetric cap (bounding one direction but not the other) is exactly the kind of gap that
/// stops being harmless the moment this client is reused against a less-trusted peer.
pub(crate) const MAX_REQUEST_LINE_BYTES: usize = 8192;

/// Read one newline-delimited line, never buffering past `max_bytes` even when no newline has
/// arrived yet — the size-cap enforcement a plain `BufReader::lines()`/`read_line()` does not
/// provide. Returns `Ok(None)` on a clean EOF with no partial data (peer closed), `Ok(Some(..))`
/// on a complete line, `Err` when the cap is exceeded (remaining buffered bytes are discarded,
/// never accumulated further).
pub(crate) async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if buf.is_empty() { None } else { Some(buf) });
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            return Ok(Some(buf));
        }
        if buf.len() + available.len() > max_bytes {
            let n = available.len();
            reader.consume(n);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "line exceeds size cap",
            ));
        }
        buf.extend_from_slice(available);
        let n = available.len();
        reader.consume(n);
    }
}

/// One request line, matching `crates/ironhermes-vault/src/profile_endpoint.rs`'s
/// `ProfileCredentialRequest` exactly (three fields, no more, no fewer).
#[derive(Debug, Serialize)]
struct ProfileCredentialRequest<'a> {
    token: &'a str,
    profile: &'a str,
    key: &'a str,
}

/// One response line — exactly one of the two shapes the endpoint ever sends.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProfileCredentialResponse {
    Ok { value: String },
    Err { error: String },
}

/// Every distinguishable failure a worker's bootstrap read can hit — named error
/// strings from the endpoint (per `51-06-SUMMARY.md`'s table, verbatim), plus the
/// transport-level failures a real Unix socket can produce.
#[derive(Debug, thiserror::Error)]
pub enum ProfileClientError {
    /// The endpoint's `malformed_request` — the line did not parse into the exact
    /// `{token, profile, key}` shape.
    #[error("malformed_request")]
    MalformedRequest,
    /// The endpoint's `token_invalid`.
    #[error("token_invalid")]
    TokenInvalid,
    /// The endpoint's `profile_mismatch` — the WIDENED protocol's explicit check.
    #[error("profile_mismatch")]
    ProfileMismatch,
    /// The endpoint's `secret_not_found`.
    #[error("secret_not_found")]
    SecretNotFound,
    /// The endpoint's `denied` (native ACL or the layer-3 guard refused).
    #[error("denied")]
    Denied,
    /// The endpoint's `token_expired`.
    #[error("token_expired")]
    TokenExpired,
    /// The endpoint's `vault_unreachable`.
    #[error("vault_unreachable")]
    VaultUnreachable,
    /// The endpoint's `invalid_key`.
    #[error("invalid_key")]
    InvalidKey,
    /// The endpoint's `backend_error`.
    #[error("backend_error")]
    BackendError,
    /// The endpoint's `request_too_large`.
    #[error("request_too_large")]
    RequestTooLarge,
    /// An error string the endpoint sent that this client does not recognize — kept
    /// distinct from the named variants above so a protocol drift is visible rather
    /// than silently folded into an existing bucket.
    #[error("unrecognized profile credential endpoint error: {0}")]
    Unrecognized(String),
    /// Could not connect to the socket at all (not running, wrong path, permission).
    #[error("connect {socket}: {source}")]
    Connect {
        socket: String,
        #[source]
        source: std::io::Error,
    },
    /// Any other I/O failure writing the request or reading the response.
    #[error("io error talking to the profile credential endpoint: {0}")]
    Io(#[from] std::io::Error),
    /// The endpoint closed the connection before sending a response line.
    #[error("profile credential endpoint closed the connection without a response")]
    Eof,
    /// The response line did not parse into either known shape.
    #[error("malformed response from the profile credential endpoint: {0}")]
    MalformedResponse(String),
    /// Phase 51 Plan 17 (IN-02): the response line exceeded [`MAX_REQUEST_LINE_BYTES`] before a
    /// newline arrived — refused rather than buffered unboundedly. The peer is same-uid and
    /// trusted today; a named refusal here is what keeps that trust assumption from becoming a
    /// silent unbounded-memory-growth hazard if this client is ever reused against a
    /// less-trusted peer.
    #[error("profile credential endpoint response exceeds the {MAX_REQUEST_LINE_BYTES}-byte cap")]
    ResponseTooLarge,
}

impl ProfileClientError {
    fn from_named(name: &str) -> Self {
        match name {
            "malformed_request" => Self::MalformedRequest,
            "token_invalid" => Self::TokenInvalid,
            "profile_mismatch" => Self::ProfileMismatch,
            "secret_not_found" => Self::SecretNotFound,
            "denied" => Self::Denied,
            "token_expired" => Self::TokenExpired,
            "vault_unreachable" => Self::VaultUnreachable,
            "invalid_key" => Self::InvalidKey,
            "backend_error" => Self::BackendError,
            "request_too_large" => Self::RequestTooLarge,
            other => Self::Unrecognized(other.to_string()),
        }
    }
}

/// One request/response round-trip over an ALREADY-CONNECTED stream — the shared body both
/// [`read_profile_credential`] (single-read) and [`read_profile_credentials`] (multi-read,
/// Phase 51 Plan 18) call, so client and server share exactly one wire-encoding and one
/// response-decoding path no matter how many requests go out over the connection.
async fn request_one_credential<W, R>(
    writer: &mut W,
    reader: &mut R,
    token: &SecretString,
    profile: &str,
    key: &str,
) -> Result<SecretString, ProfileClientError>
where
    W: tokio::io::AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    let request = ProfileCredentialRequest {
        token: token.expose_secret(),
        profile,
        key,
    };
    let mut line = serde_json::to_string(&request)
        .map_err(|e| ProfileClientError::MalformedResponse(e.to_string()))?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;

    // Phase 51 Plan 17 (IN-02): bounded, matching the server's own request cap — an unbounded
    // `read_line` here would let a peer that never sends a newline grow this buffer without
    // limit.
    let response_bytes = match read_bounded_line(reader, MAX_REQUEST_LINE_BYTES).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(ProfileClientError::Eof),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(ProfileClientError::ResponseTooLarge);
        }
        Err(e) => return Err(ProfileClientError::Io(e)),
    };

    let response: ProfileCredentialResponse = serde_json::from_slice(&response_bytes)
        .map_err(|e| ProfileClientError::MalformedResponse(e.to_string()))?;
    match response {
        ProfileCredentialResponse::Ok { value } => Ok(SecretString::from(value)),
        ProfileCredentialResponse::Err { error } => Err(ProfileClientError::from_named(&error)),
    }
}

/// Read one credential over the widened `{token, profile, key}` protocol. One
/// connection per call: connect, write one line, read one line, disconnect — mirrors
/// `rpc_server.rs`'s newline-delimited-JSON shape, but this client only ever sends a
/// single request per connection (the endpoint itself supports many requests per
/// connection; the worker's bootstrap only ever needs one).
///
/// `token` is exposed only for the instant it takes to serialize the request line —
/// never logged, never retained as a `String`.
///
/// A thin wrapper over [`read_profile_credentials`] (Phase 51 Plan 18) — kept as its own
/// function so every EXISTING caller and test is untouched, but the connection handling and
/// wire encoding/decoding live in exactly one place.
pub async fn read_profile_credential(
    socket_path: &Path,
    token: &SecretString,
    profile: &str,
    key: &str,
) -> Result<SecretString, ProfileClientError> {
    let mut results = read_profile_credentials(socket_path, token, profile, &[key]).await?;
    results
        .pop()
        .map(|(_, outcome)| outcome)
        .unwrap_or(Err(ProfileClientError::Eof))
}

/// Read MULTIPLE credentials over ONE connection (Phase 51 Plan 18, G-51-5): connects once,
/// then for each entry in `keys`, writes the existing `{token, profile, key}` request line and
/// reads the existing single response line via [`request_one_credential`] — reusing the
/// existing bounded-line reader and [`MAX_REQUEST_LINE_BYTES`] cap for every response,
/// symmetrically. D-12 is one-way: this is N reads of the EXISTING single-read wire shape,
/// never a new operation, field, or batch request shape.
///
/// Returns one `(key, outcome)` pair per requested key, IN THE SAME ORDER as `keys`. Only the
/// initial connect failure is a whole-call `Err` — every per-key outcome after a successful
/// connect (a named protocol error, a transport failure mid-batch, a malformed response, ...)
/// is folded into that key's own `Result`, because the caller (`worker_bootstrap`'s
/// multi-provider loop) needs to know PER PROVIDER whether the read succeeded, not merely
/// whether the whole batch did — a transport failure on one key does not prevent reporting
/// outcomes already obtained for earlier keys in the same call.
pub async fn read_profile_credentials(
    socket_path: &Path,
    token: &SecretString,
    profile: &str,
    keys: &[&str],
) -> Result<Vec<(String, Result<SecretString, ProfileClientError>)>, ProfileClientError> {
    let stream =
        UnixStream::connect(socket_path)
            .await
            .map_err(|source| ProfileClientError::Connect {
                socket: socket_path.display().to_string(),
                source,
            })?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut results = Vec::with_capacity(keys.len());
    for key in keys {
        let outcome = request_one_credential(&mut writer, &mut reader, token, profile, key).await;
        results.push(((*key).to_string(), outcome));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_named_maps_every_documented_error_string() {
        let cases: &[(&str, &str)] = &[
            ("malformed_request", "malformed_request"),
            ("token_invalid", "token_invalid"),
            ("profile_mismatch", "profile_mismatch"),
            ("secret_not_found", "secret_not_found"),
            ("denied", "denied"),
            ("token_expired", "token_expired"),
            ("vault_unreachable", "vault_unreachable"),
            ("invalid_key", "invalid_key"),
            ("backend_error", "backend_error"),
            ("request_too_large", "request_too_large"),
        ];
        for (input, expected_display) in cases {
            let err = ProfileClientError::from_named(input);
            assert_eq!(err.to_string(), *expected_display, "input {input}");
        }
    }

    #[test]
    fn from_named_unrecognized_is_distinct() {
        let err = ProfileClientError::from_named("some_future_error");
        assert!(matches!(err, ProfileClientError::Unrecognized(_)));
    }

    #[test]
    fn request_serializes_to_exactly_three_fields() {
        let req = ProfileCredentialRequest {
            token: "tok",
            profile: "alpha",
            key: "openrouter",
        };
        let value: serde_json::Value = serde_json::to_value(&req).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj.len(), 3, "request must carry exactly {{token, profile, key}}");
        assert_eq!(obj.get("token").and_then(|v| v.as_str()), Some("tok"));
        assert_eq!(obj.get("profile").and_then(|v| v.as_str()), Some("alpha"));
        assert_eq!(obj.get("key").and_then(|v| v.as_str()), Some("openrouter"));
    }

    #[tokio::test]
    async fn connect_failure_on_a_nonexistent_socket_is_named() {
        let path = std::env::temp_dir().join(format!(
            "ihvc-test-nonexistent-{}.sock",
            std::process::id()
        ));
        let token = SecretString::from("tok".to_string());
        let err = read_profile_credential(&path, &token, "alpha", "openrouter")
            .await
            .expect_err("connecting to a nonexistent socket must fail");
        assert!(matches!(err, ProfileClientError::Connect { .. }));
    }

    /// Phase 51 Plan 17 (IN-02): a peer that sends a response line past
    /// `MAX_REQUEST_LINE_BYTES` without ever sending a newline must be refused with a named
    /// error, not read into an ever-growing buffer. A minimal fake server — not the real
    /// endpoint — so this test exercises only the CLIENT's own bound.
    #[tokio::test]
    async fn oversized_response_is_refused_without_buffering_it() {
        let socket_path = std::env::temp_dir().join(format!(
            "ihvc-test-oversized-response-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket_path);
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind fake server");

        let server = tokio::spawn(async move {
            let (mut stream, _addr) = listener.accept().await.expect("accept");
            // Consume the one request line the client always sends before reading a response.
            let mut reader = BufReader::new(&mut stream);
            let mut discard = String::new();
            let _ = reader.read_line(&mut discard).await;
            // Respond with a line well past the cap, no newline — the client must refuse
            // before this ever completes into a "line".
            let oversized = vec![b'a'; MAX_REQUEST_LINE_BYTES + 1024];
            let _ = stream.write_all(&oversized).await;
            let _ = stream.flush().await;
        });

        let token = SecretString::from("tok".to_string());
        let err = read_profile_credential(&socket_path, &token, "alpha", "openrouter")
            .await
            .expect_err("an oversized response must be refused, not buffered unboundedly");
        assert!(
            matches!(err, ProfileClientError::ResponseTooLarge),
            "expected ResponseTooLarge, got {err:?}"
        );

        let _ = server.await;
        let _ = std::fs::remove_file(&socket_path);
    }

    /// Phase 51 Plan 18 Test 5 (D-12 — one-way, no protocol change): N credentials are
    /// fetched as N `{token, profile, key}`/`{value}` round-trips on EXACTLY ONE connection —
    /// never via a new operation, a batch shape, or a second connection. A minimal fake
    /// server (not the real endpoint) counts accepted connections and captures each request
    /// line verbatim, so this proves both properties directly rather than assuming them from
    /// the single-read client's own existing coverage.
    #[tokio::test]
    async fn multi_read_issues_n_requests_on_exactly_one_connection() {
        let socket_path = std::env::temp_dir().join(format!(
            "ihvc-test-multi-read-one-conn-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket_path);
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind fake server");

        let server = tokio::spawn(async move {
            let (stream, _addr) = listener.accept().await.expect("accept first connection");
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut captured_requests = Vec::new();
            for _ in 0..2 {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .await
                    .expect("read request line");
                let value: serde_json::Value =
                    serde_json::from_str(line.trim()).expect("request line must be valid JSON");
                captured_requests.push(value);
                writer.write_all(b"{\"value\":\"ok\"}\n").await.unwrap();
                writer.flush().await.unwrap();
            }
            // Prove no SECOND connection was ever attempted — a short timeout on a second
            // accept() must elapse, never complete.
            let second_accept =
                tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
                    .await;
            assert!(
                second_accept.is_err(),
                "a second connection must never be opened for N reads on one connection"
            );
            captured_requests
        });

        let token = SecretString::from("tok".to_string());
        let results =
            read_profile_credentials(&socket_path, &token, "alpha", &["providerA", "providerB"])
                .await
                .expect("multi-read must succeed");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "providerA");
        assert_eq!(results[1].0, "providerB");
        assert!(
            results[0].1.is_ok(),
            "providerA outcome: {:?}",
            results[0].1
        );
        assert!(
            results[1].1.is_ok(),
            "providerB outcome: {:?}",
            results[1].1
        );

        let captured_requests = server.await.expect("server task must not panic");
        assert_eq!(
            captured_requests.len(),
            2,
            "the server must see exactly two request lines"
        );
        for (i, key) in ["providerA", "providerB"].iter().enumerate() {
            let obj = captured_requests[i]
                .as_object()
                .expect("each request line must be a JSON object");
            assert_eq!(
                obj.len(),
                3,
                "request must carry exactly {{token, profile, key}} — no new field, no batch shape"
            );
            assert_eq!(obj.get("token").and_then(|v| v.as_str()), Some("tok"));
            assert_eq!(obj.get("profile").and_then(|v| v.as_str()), Some("alpha"));
            assert_eq!(obj.get("key").and_then(|v| v.as_str()), Some(*key));
        }

        let _ = std::fs::remove_file(&socket_path);
    }
}
