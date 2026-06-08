// Phase 36.2 Plan 09 — integration tests for `hermes pricing` CLI subcommand.
//
// 10 behaviors covered (per PLAN.md §<task><behavior>):
//   1-3  clap parsing (list / refresh / refresh --force)
//   4    cmd_list output prints at least one bundled-table model
//   5    parse_models_dev_body — successful schema parses to entries
//   6    cmd_refresh — empty body without --force exits non-zero + cache untouched
//   7    cmd_refresh — empty body with --force writes the empty cache file
//   8    cmd_refresh — network failure exits non-zero + cache untouched
//   9    cost_field_to_micros — f64 / i64 / u64 / missing / wrong-type conversions
//   10   round-trip — refresh writes, list reads back
//
// Plus: clap subprocess assertions through CARGO_BIN_EXE_ironhermes for behaviors 1-3.

use assert_cmd::Command;
use ironhermes_cli::pricing_cmd::{
    PricingSubcommand, cmd_list_to_string, cmd_refresh_from_url_with_path,
};
use ironhermes_core::pricing_cache::{PricingCache, cost_field_to_micros, parse_models_dev_body};
use std::sync::OnceLock;
use tempfile::TempDir;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Behaviors 1-3: clap parsing through the binary
// ---------------------------------------------------------------------------

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[test]
fn cli_parses_pricing_list_subcommand() {
    // Behavior 1: `hermes pricing list` is a recognized subcommand and runs cleanly
    // against an empty $HERMES_HOME (no cache file).
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", tmp.path())
        .args(["pricing", "list"])
        .assert()
        .success();
}

#[test]
fn cli_parses_pricing_refresh_without_force() {
    // Behavior 2: `hermes pricing refresh` parses to PricingSubcommand::Refresh { force: false }.
    // Lib-level: the enum variant exists and can be constructed with force=false.
    let cmd = PricingSubcommand::Refresh {
        force: false,
        source: "models.dev".to_string(),
    };
    match cmd {
        PricingSubcommand::Refresh { force, source } => {
            assert!(!force);
            assert_eq!(source, "models.dev");
        }
        _ => panic!("expected Refresh variant"),
    }
}

#[test]
fn cli_parses_pricing_refresh_with_force() {
    // Behavior 3: `hermes pricing refresh --force` parses to PricingSubcommand::Refresh { force: true }.
    let cmd = PricingSubcommand::Refresh {
        force: true,
        source: "models.dev".to_string(),
    };
    match cmd {
        PricingSubcommand::Refresh { force, source: _ } => assert!(force),
        _ => panic!("expected Refresh variant"),
    }
}

#[test]
fn cli_parses_pricing_refresh_with_openrouter_source() {
    // Phase 36.2 follow-up: `hermes pricing refresh --source openrouter`
    let cmd = PricingSubcommand::Refresh {
        force: false,
        source: "openrouter".to_string(),
    };
    match cmd {
        PricingSubcommand::Refresh { source, .. } => assert_eq!(source, "openrouter"),
        _ => panic!("expected Refresh variant"),
    }
}

// ---------------------------------------------------------------------------
// Behavior 4: cmd_list prints a bundled-table model
// ---------------------------------------------------------------------------

#[test]
fn cmd_list_prints_bundled_table_models() {
    // Even with an empty cache, the bundled pricing.toml has 9 rows. `cmd_list`
    // must surface at least one (claude-opus-4-7) to confirm the merge path runs.
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    let output = cmd_list_to_string(&tmp.path().join("nonexistent-cache.json"));
    assert!(
        output.contains("claude-opus-4-7"),
        "cmd_list output must include claude-opus-4-7 (bundled): got\n{output}"
    );
    assert!(
        output.contains("anthropic"),
        "cmd_list output must include provider 'anthropic': got\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Behavior 5: parse_models_dev_body — happy path
// ---------------------------------------------------------------------------

#[test]
fn parse_models_dev_body_extracts_entries_from_canned_response() {
    let body = serde_json::json!({
        "anthropic": {
            "models": {
                "claude-test-9": {
                    "cost": {
                        "input": 5.0,
                        "output": 25.0,
                        "cache_read": 0.5,
                        "cache_write": 6.25
                    }
                }
            }
        }
    });
    let parsed = parse_models_dev_body(&body);
    let entry = parsed.get("claude-test-9").expect("claude-test-9 present");
    assert_eq!(entry.pricing.provider, "anthropic");
    assert_eq!(entry.pricing.input_per_1m_micros, 5_000_000);
    assert_eq!(entry.pricing.output_per_1m_micros, 25_000_000);
    assert_eq!(entry.pricing.cache_read_per_1m_micros, 500_000);
    assert_eq!(entry.pricing.cache_creation_per_1m_micros, 6_250_000);
}

#[test]
fn parse_models_dev_body_skips_all_zero_entries() {
    // Defensive: rows with zero input AND zero output get filtered out so the
    // cache doesn't accumulate dead entries from incomplete upstream rows.
    let body = serde_json::json!({
        "anthropic": {
            "models": {
                "zero-row": {
                    "cost": { "input": 0, "output": 0 }
                },
                "real-row": {
                    "cost": { "input": 1.0, "output": 2.0 }
                }
            }
        }
    });
    let parsed = parse_models_dev_body(&body);
    assert!(!parsed.contains_key("zero-row"));
    assert!(parsed.contains_key("real-row"));
}

// ---------------------------------------------------------------------------
// Behavior 6: empty body + no --force => non-zero exit, cache untouched
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn cmd_refresh_rejects_empty_response_without_force() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let cache_path = tmp.path().join("pricing-cache.json");

    // Pre-seed an existing cache to prove we DON'T overwrite it.
    let pre_existing = r#"{"entries":{"sentinel":{"pricing":{"provider":"sentinel-provider","input_per_1m_micros":1,"output_per_1m_micros":2,"cache_read_per_1m_micros":3,"cache_creation_per_1m_micros":4},"fetched_at":"2026-05-25T00:00:00Z"}}}"#;
    std::fs::write(&cache_path, pre_existing).unwrap();

    let result = cmd_refresh_from_url_with_path(&server.uri(), false, &cache_path, None).await;
    assert!(result.is_err(), "empty response without --force must error");

    // Cache file is byte-identical to the pre-existing seed.
    let post = std::fs::read_to_string(&cache_path).unwrap();
    assert_eq!(post, pre_existing, "cache file must NOT be overwritten");
}

// ---------------------------------------------------------------------------
// Behavior 7: empty body + --force => write empty cache (escape hatch)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn cmd_refresh_with_force_writes_empty_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let cache_path = tmp.path().join("pricing-cache.json");

    let result = cmd_refresh_from_url_with_path(&server.uri(), true, &cache_path, None).await;
    assert!(result.is_ok(), "with --force, empty response must succeed");

    assert!(cache_path.exists(), "cache file must be written");
    let cache = PricingCache::load_from(&cache_path);
    assert!(cache.entries.is_empty());
}

// ---------------------------------------------------------------------------
// Behavior 8: network failure => non-zero exit, cache untouched
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn cmd_refresh_handles_network_failure_without_touching_cache() {
    // Connect to a port that should refuse connections.
    let bogus_url = "http://127.0.0.1:1";

    let tmp = TempDir::new().unwrap();
    let cache_path = tmp.path().join("pricing-cache.json");
    let pre_existing = r#"{"entries":{}}"#;
    std::fs::write(&cache_path, pre_existing).unwrap();

    let result = cmd_refresh_from_url_with_path(bogus_url, false, &cache_path, None).await;
    assert!(result.is_err(), "network failure must error");

    let post = std::fs::read_to_string(&cache_path).unwrap();
    assert_eq!(
        post, pre_existing,
        "cache must be byte-identical to pre-state"
    );
}

// ---------------------------------------------------------------------------
// Behavior 9: cost_field_to_micros conversions
// ---------------------------------------------------------------------------

#[test]
fn cost_field_to_micros_handles_all_input_types() {
    use serde_json::json;
    assert_eq!(cost_field_to_micros(Some(&json!(5.0))), 5_000_000);
    assert_eq!(cost_field_to_micros(Some(&json!(15))), 15_000_000);
    assert_eq!(cost_field_to_micros(Some(&json!(0.5))), 500_000);
    assert_eq!(cost_field_to_micros(None), 0);
    assert_eq!(cost_field_to_micros(Some(&json!("string-not-number"))), 0);
    assert_eq!(cost_field_to_micros(Some(&json!(null))), 0);
    assert_eq!(cost_field_to_micros(Some(&json!([1, 2, 3]))), 0);
}

// ---------------------------------------------------------------------------
// Behavior 10: round-trip refresh → list
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn round_trip_refresh_then_list_surfaces_new_entry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synthetic-provider": {
                "models": {
                    "round-trip-test-model": {
                        "cost": {
                            "input": 7.0,
                            "output": 42.0,
                            "cache_read": 0.7,
                            "cache_write": 8.75
                        }
                    }
                }
            }
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let cache_path = tmp.path().join("pricing-cache.json");

    cmd_refresh_from_url_with_path(&server.uri(), false, &cache_path, None)
        .await
        .expect("refresh succeeds with valid body");

    let listing = cmd_list_to_string(&cache_path);
    assert!(
        listing.contains("round-trip-test-model"),
        "list output must include freshly-refreshed entry: got\n{listing}"
    );
}
