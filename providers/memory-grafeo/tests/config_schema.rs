//! Phase 20-04 Task 20-04-02: pin grafeo provider name() and get_config_schema().
//!
//! These assertions are the plugin-contract surface. Changing them is a
//! breaking change for the setup wizard (Phase 20-03) and must be done
//! with a corresponding wizard update.

use ironhermes_core::memory_provider::MemoryProvider;
use memory_grafeo::GrafeoMemoryProvider;

#[test]
fn grafeo_provider_name_is_grafeo() {
    let provider = GrafeoMemoryProvider::new_in_memory();
    assert_eq!(provider.name(), "grafeo");
}

#[test]
fn grafeo_provider_config_schema_shape() {
    let provider = GrafeoMemoryProvider::new_in_memory();
    let schema = provider.get_config_schema();

    assert_eq!(schema.len(), 1, "expected one field (graph_dir)");
    let graph_dir = &schema[0];
    assert_eq!(graph_dir.key, "graph_dir");
    assert!(
        graph_dir
            .description
            .as_ref()
            .is_some_and(|d| !d.is_empty()),
        "graph_dir description must be non-empty"
    );
    assert!(!graph_dir.required);
    assert!(!graph_dir.secret);
    assert!(graph_dir.env_var.is_none());
    assert_eq!(
        graph_dir.default,
        Some(serde_json::json!("$HERMES_HOME/grafeo")),
    );
}

#[test]
fn grafeo_provider_secret_implies_env_var() {
    let provider = GrafeoMemoryProvider::new_in_memory();
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
