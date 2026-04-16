//! Phase 20-04 Task 20-04-02: pin duckdb provider name() and get_config_schema().
//!
//! These assertions are the plugin-contract surface. Changing them is a
//! breaking change for the setup wizard (Phase 20-03) and must be done
//! with a corresponding wizard update.

use ironhermes_core::memory_provider::MemoryProvider;
use memory_duckdb::DuckDbMemoryProvider;

#[test]
fn duckdb_provider_name_is_duckdb() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = DuckDbMemoryProvider::new(&tmp.path().join("mem.duckdb")).unwrap();
    assert_eq!(provider.name(), "duckdb");
}

#[test]
fn duckdb_provider_config_schema_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = DuckDbMemoryProvider::new(&tmp.path().join("mem.duckdb")).unwrap();
    let schema = provider.get_config_schema();

    let keys: Vec<&str> = schema.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, vec!["db_path", "threads"]);

    let db_path = schema.iter().find(|f| f.key == "db_path").unwrap();
    assert!(
        db_path.description.as_ref().is_some_and(|d| !d.is_empty()),
        "db_path description must be non-empty"
    );
    assert!(!db_path.required);
    assert!(!db_path.secret);
    assert!(db_path.env_var.is_none());
    assert_eq!(
        db_path.default,
        Some(serde_json::json!("$HERMES_HOME/memory.duckdb")),
    );

    let threads = schema.iter().find(|f| f.key == "threads").unwrap();
    assert!(
        threads.description.as_ref().is_some_and(|d| !d.is_empty()),
        "threads description must be non-empty"
    );
    assert!(!threads.required);
    assert!(!threads.secret);
    assert!(threads.env_var.is_none());
    assert_eq!(threads.default, Some(serde_json::json!(1)));
}

#[test]
fn duckdb_provider_secret_implies_env_var() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = DuckDbMemoryProvider::new(&tmp.path().join("mem.duckdb")).unwrap();
    for field in provider.get_config_schema() {
        if field.secret {
            assert!(
                field.env_var.is_some(),
                "secret field {} must declare env_var",
                field.key
            );
        }
    }
}
