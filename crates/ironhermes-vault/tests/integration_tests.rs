//! Wave-0 integration test scaffold for `ironhermes-vault` (Phase 46.8 Plan 01 Task 2).
//!
//! Uses `EnvVarStore::with_lookup` with an in-memory `HashMap` throughout — process
//! environment variables are never mutated directly (RESEARCH Pitfall 5: real env-var
//! writes race under parallel test execution).

use std::collections::HashMap;

use ironhermes_vault::{EnvVarStore, SecretStore};
use secrecy::{ExposeSecret, SecretString};

fn store_with(pairs: &[(&str, &str)]) -> EnvVarStore {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    EnvVarStore::with_lookup(map)
}

#[tokio::test]
async fn get_secret_missing_key_returns_none() {
    let store = store_with(&[]);
    let result = store.get_secret("MISSING").await.expect("no error");
    assert!(result.is_none());
}

#[tokio::test]
async fn get_secret_present_key_returns_value() {
    let store = store_with(&[("K", "v")]);
    let result = store.get_secret("K").await.expect("no error");
    let secret = result.expect("value present");
    assert_eq!(secret.expose_secret(), "v");
}

#[tokio::test]
async fn put_secret_is_read_only_and_errors() {
    let store = store_with(&[]);
    let err = store
        .put_secret("K", SecretString::from("v"))
        .await
        .expect_err("put must error — EnvVarStore is read-only diagnostic");
    assert!(err.to_string().contains("read-only diagnostic"));
}

#[tokio::test]
async fn delete_secret_is_read_only_and_errors() {
    let store = store_with(&[("K", "v")]);
    let err = store
        .delete_secret("K")
        .await
        .expect_err("delete must error — EnvVarStore is read-only diagnostic");
    assert!(err.to_string().contains("read-only diagnostic"));
}

#[tokio::test]
async fn list_secrets_no_prefix_returns_all_names() {
    let store = store_with(&[("alpha", "1"), ("beta", "2")]);
    let mut names = store.list_secrets(None).await.expect("no error");
    names.sort();
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
}

#[tokio::test]
async fn list_secrets_with_prefix_filters_names() {
    let store = store_with(&[("prefixed_a", "1"), ("prefixed_b", "2"), ("other", "3")]);
    let mut names = store
        .list_secrets(Some("prefixed"))
        .await
        .expect("no error");
    names.sort();
    assert_eq!(
        names,
        vec!["prefixed_a".to_string(), "prefixed_b".to_string()]
    );
}

#[tokio::test]
async fn max_length_value_round_trips_unchanged() {
    let big_value = "x".repeat(8 * 1024); // 8 KiB
    let store = store_with(&[("BIG", &big_value)]);
    let result = store.get_secret("BIG").await.expect("no error");
    let secret = result.expect("value present");
    assert_eq!(secret.expose_secret(), big_value.as_str());
    assert_eq!(secret.expose_secret().len(), 8 * 1024);
}

#[tokio::test]
async fn empty_string_value_round_trips_exactly() {
    let store = store_with(&[("EMPTY", "")]);
    let result = store.get_secret("EMPTY").await.expect("no error");
    let secret = result.expect("value present — an empty string is a real value, not absence");
    assert_eq!(secret.expose_secret(), "");
}

#[test]
fn secret_store_trait_object_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn SecretStore>>();
    assert_send_sync::<EnvVarStore>();
}

#[test]
fn secret_debug_output_never_contains_raw_value() {
    let secret = SecretString::from("super-secret-value-should-never-appear");
    let debug_output = format!("{:?}", secret);
    assert!(!debug_output.contains("super-secret-value-should-never-appear"));
    assert!(debug_output.contains("REDACTED"));
}
