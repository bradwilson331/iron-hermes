//! Phase 20-04 Task 20-04-01: pin sqlite provider name() and get_config_schema().
//!
//! These assertions are the plugin-contract surface. Changing them is a
//! breaking change for the setup wizard (Phase 20-03) and must be done
//! with a corresponding wizard update.

use ironhermes_core::memory_provider::MemoryProvider;
use memory_sqlite::SqliteMemoryProvider;

#[test]
fn sqlite_provider_name_is_sqlite() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = SqliteMemoryProvider::new(&tmp.path().join("mem.db")).unwrap();
    assert_eq!(provider.name(), "sqlite");
}

#[test]
fn sqlite_provider_config_schema_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = SqliteMemoryProvider::new(&tmp.path().join("mem.db")).unwrap();
    let schema = provider.get_config_schema();

    assert_eq!(schema.len(), 1, "expected one field (db_path)");
    let db_path = &schema[0];
    assert_eq!(db_path.key, "db_path");
    assert!(
        db_path.description.as_ref().is_some_and(|d| !d.is_empty()),
        "db_path description must be a non-empty string"
    );
    assert!(!db_path.required);
    assert!(!db_path.secret);
    assert!(db_path.env_var.is_none());
    assert_eq!(
        db_path.default,
        Some(serde_json::json!("$HERMES_HOME/memory.db")),
    );
}

#[test]
fn sqlite_provider_secret_implies_env_var() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = SqliteMemoryProvider::new(&tmp.path().join("mem.db")).unwrap();
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
