//! Phase 43 Plan 05 — CLI integration tests for `hermes auth` subcommand.
//!
//! Tests verify:
//! - Clap argument parsing (all valid forms accepted; mutually exclusive flags rejected)
//! - Flow dispatch: `--browser` routes to PKCE; `--device-code` routes to device flow
//! - Status redaction invariant (A-1): token values must never appear in output
//! - End-to-end device-code flow (SC#3): binary prints verification URL + user code
//!
//! Tests 1-5 are the Plan 01 Wave-0 RED scaffolds with `#[ignore]` removed and
//! assertions corrected for the real runtime behavior of cmd_login/status/list.
//! Tests 6-8 are new dispatch-proof and end-to-end tests added by Plan 05.

use assert_cmd::Command;
use predicates::prelude::*;

// ─────────────────────────────────────────────────────────────────────────────
// Helper: write a minimal config.yaml into a TempDir
// ─────────────────────────────────────────────────────────────────────────────

fn write_config(dir: &tempfile::TempDir, yaml: &str) {
    std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// SC: hermes auth login --provider <name> parses correctly
// ─────────────────────────────────────────────────────────────────────────────

/// `hermes auth login --provider <name>` must be accepted by the CLI argument parser
/// and route to cmd_login (not rejected as "unrecognized subcommand").
///
/// Without a configured provider the command fails at runtime with "provider not
/// configured" — this error proves clap routed correctly to the handler.
#[test]
fn test_auth_login_provider_flag_parses() {
    // Empty IRONHERMES_HOME — no config.yaml, so no providers are configured.
    let dir = tempfile::TempDir::new().unwrap();

    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "login", "--provider", "xai"])
        .assert()
        .failure()
        // Clap parsed correctly → handler was called → "provider not configured" error.
        // A clap parse failure would say "unrecognized subcommand" or similar.
        .stderr(predicate::str::contains("provider not configured: xai"));
}

// ─────────────────────────────────────────────────────────────────────────────
// SC: --device-code flag is accepted and conflicts with --browser
// ─────────────────────────────────────────────────────────────────────────────

/// `hermes auth login --provider <name> --device-code` must be accepted.
///
/// Also verifies that `--device-code` and `--browser` together are rejected by clap
/// (conflicts_with constraint on AuthCommands::Login).
#[test]
fn test_auth_login_device_code_flag_parses() {
    let dir = tempfile::TempDir::new().unwrap();

    // --device-code alone: clap accepts it; handler routes to device flow; fails
    // at runtime because "xai" is not configured. Proves parsing succeeded.
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "login", "--provider", "xai", "--device-code"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("provider not configured: xai"));

    // --device-code and --browser together must be rejected by clap.
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args([
            "auth",
            "login",
            "--provider",
            "xai",
            "--device-code",
            "--browser",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

// ─────────────────────────────────────────────────────────────────────────────
// SC: hermes auth status [--provider <name>] parses correctly
// ─────────────────────────────────────────────────────────────────────────────

/// `hermes auth status` (all providers) and `hermes auth status --provider <name>`
/// (single provider) must both succeed even when no tokens are present.
///
/// With no configured providers, `status` prints "No providers configured" → exits 0.
/// With `--provider xai` and no token, it prints "not authenticated" → exits 0.
#[test]
fn test_auth_status_parses() {
    let dir = tempfile::TempDir::new().unwrap();

    // Without --provider: no providers configured → success with informational message.
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No providers configured"));

    // With --provider: namespace not in config → "not authenticated" → success.
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "status", "--provider", "xai"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not authenticated"));
}

// ─────────────────────────────────────────────────────────────────────────────
// SC: hermes auth list parses correctly
// ─────────────────────────────────────────────────────────────────────────────

/// `hermes auth list` must parse correctly and exit 0.
///
/// With no configured providers, `list` prints "No providers configured" and exits 0.
#[test]
fn test_auth_list_parses() {
    let dir = tempfile::TempDir::new().unwrap();

    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No providers configured"));
}

// ─────────────────────────────────────────────────────────────────────────────
// SC: hermes auth status never prints a raw token value (A-1 / D-08 redaction)
// ─────────────────────────────────────────────────────────────────────────────

/// `hermes auth status` output must never contain a raw token value.
///
/// The status command shows presence + expiry but MUST NOT print the `access_token`,
/// `refresh_token`, or a `Bearer` value (A-1, D-08 redaction invariant from PITFALLS §A-1).
///
/// Uses `IRONHERMES_HOME` to point the binary at a pre-seeded `auth.json` with a
/// known token value, then asserts the token value is absent from all output.
#[test]
fn test_auth_status_never_prints_token_value() {
    use std::io::Write as _;
    use tempfile::TempDir;

    // Seed a temp IRONHERMES_HOME with a known token value.
    let dir = TempDir::new().unwrap();
    let auth_json_path = dir.path().join("auth.json");
    let mut f = std::fs::File::create(&auth_json_path).unwrap();
    writeln!(
        f,
        r#"{{"tokens":{{"xai":{{"access_token":"SUPER_SECRET_ACCESS_TOKEN_DO_NOT_PRINT","token_type":"Bearer","scopes":["read"]}}}}}}"#
    )
    .unwrap();

    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "status"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !combined.contains("SUPER_SECRET_ACCESS_TOKEN_DO_NOT_PRINT"),
        "auth status must not print the raw access_token (A-1); output: {combined}"
    );
    assert!(
        !combined.contains("Bearer"),
        "auth status must not print a Bearer token value (A-1); output: {combined}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch routing: --browser routes to PKCE (not device code)
// ─────────────────────────────────────────────────────────────────────────────

/// `--browser` flag must select the PKCE flow, not the device-code flow.
///
/// Proof method: provide a config that has `device_authorization_url` but NOT
/// `authorization_url`. PKCE requires `authorization_url` → fails fast with
/// "authorization_url" in the error. If device code were selected instead,
/// the binary would try to reach `device_authorization_url` (different failure).
///
/// This proves SC#1 routing: `--browser` reaches `run_pkce_login` (D-04).
#[test]
fn test_browser_flag_routes_to_pkce_dispatch() {
    let dir = tempfile::TempDir::new().unwrap();
    // Deliberately omit authorization_url — only the device URL is present.
    write_config(
        &dir,
        "auth:\n  providers:\n    test:\n      \
         client_id: test-client\n      \
         device_authorization_url: http://127.0.0.1:19999/device/code\n      \
         token_url: http://127.0.0.1:19999/token\n      \
         scopes: []\n",
    );

    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "login", "--provider", "test", "--browser"])
        .assert()
        .failure()
        // PKCE was selected → "missing authorization_url" error (not a device-flow error).
        .stderr(predicate::str::contains("authorization_url"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch routing: --device-code routes to device flow (not PKCE)
// ─────────────────────────────────────────────────────────────────────────────

/// `--device-code` flag must select the device-code flow, not PKCE.
///
/// Proof method: provide a config that has `authorization_url` but NOT
/// `device_authorization_url`. Device flow requires `device_authorization_url`
/// → fails fast with "device_authorization_url" in the error. If PKCE were selected
/// instead, the binary would bind a loopback listener and wait → different behaviour.
///
/// This proves D-04 routing: `--device-code` reaches `run_device_code_login`.
#[test]
fn test_device_code_flag_routes_to_device_dispatch() {
    let dir = tempfile::TempDir::new().unwrap();
    // Deliberately omit device_authorization_url — only the browser URL is present.
    write_config(
        &dir,
        "auth:\n  providers:\n    test:\n      \
         client_id: test-client\n      \
         authorization_url: http://127.0.0.1:19999/authorize\n      \
         token_url: http://127.0.0.1:19999/token\n      \
         scopes: []\n",
    );

    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "login", "--provider", "test", "--device-code"])
        .assert()
        .failure()
        // Device flow was selected → "missing device_authorization_url" error.
        .stderr(predicate::str::contains("device_authorization_url"));
}

// ─────────────────────────────────────────────────────────────────────────────
// End-to-end: device-code flow via CLI (SC#3 — D-03)
// ─────────────────────────────────────────────────────────────────────────────

/// SC#3 end-to-end: `hermes auth login --provider test --device-code` must print
/// the verification URL and user code to stdout, then complete successfully when
/// the token endpoint returns a token.
///
/// Uses wiremock to stub both the device-authorization and token endpoints.
/// The mock returns `interval: 0` (no poll wait) so the test completes instantly.
///
/// Exercises all four criteria through the CLI entry point (D-03):
/// - SC#3: verification URI + user code printed to stdout before polling
/// - A-1: access_token value must NOT appear in status output after login
/// - dispatch: `--device-code` flag selects device flow end-to-end
#[tokio::test]
async fn test_device_code_login_sc3_end_to_end() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = tempfile::TempDir::new().unwrap();

    // Start a local mock OAuth server.
    let mock_server = MockServer::start().await;

    // Mock device-authorization endpoint → returns device_code, user_code, verification_uri.
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "cli-test-device-code",
            "user_code": "CLI-TEST-9876",
            "verification_uri": "https://example.com/activate",
            "verification_uri_complete": "https://example.com/activate?user_code=CLI-TEST-9876",
            "expires_in": 300,
            "interval": 0,
        })))
        .mount(&mock_server)
        .await;

    // Mock token endpoint → immediate success on first poll.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "cli-e2e-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
        })))
        .mount(&mock_server)
        .await;

    // Write config.yaml with the mock server endpoints.
    let config_yaml = format!(
        "auth:\n  providers:\n    test:\n      \
         display_name: Test Provider\n      \
         client_id: cli-test-client\n      \
         device_authorization_url: {}/device/code\n      \
         token_url: {}/token\n      \
         scopes: [read]\n",
        mock_server.uri(),
        mock_server.uri()
    );
    write_config(&dir, &config_yaml);

    // Run the CLI binary with --device-code.
    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "login", "--provider", "test", "--device-code"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "device-code login must exit 0; stdout={stdout}\nstderr={stderr}"
    );

    // SC#3: verification URL must be printed to stdout before polling begins.
    assert!(
        stdout.contains("https://example.com/activate"),
        "stdout must contain the verification URI (SC#3); got: {stdout}"
    );

    // SC#3: user code must be printed to stdout.
    assert!(
        stdout.contains("CLI-TEST-9876"),
        "stdout must contain the user code (SC#3); got: {stdout}"
    );

    // A-1: the access_token value must NOT appear in stdout.
    assert!(
        !stdout.contains("cli-e2e-access-token"),
        "stdout must not contain the access_token value (A-1); got: {stdout}"
    );

    // Verify the token is now stored — run auth status and check it shows authenticated.
    let status_output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["auth", "status"])
        .output()
        .unwrap();
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(
        status_output.status.success(),
        "auth status must exit 0 after login; stderr={}",
        String::from_utf8_lossy(&status_output.stderr)
    );
    assert!(
        status_stdout.contains("authenticated"),
        "auth status must show 'authenticated' after login; got: {status_stdout}"
    );
    // A-1: token value must not appear in status output either.
    assert!(
        !status_stdout.contains("cli-e2e-access-token"),
        "auth status must not print the access_token value (A-1); got: {status_stdout}"
    );
}
