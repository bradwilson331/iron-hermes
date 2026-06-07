//! Field-list drift test scaffold for cli-config.yaml.example vs typed Config sections.
//! REQ-37.1-02: Every top-level Config section must appear as a top-level key in
//! cli-config.yaml.example. These tests are INTENTIONALLY RED (Wave 0 scaffolding)
//! because cli-config.yaml.example is missing mcp_servers, autonomous, browser,
//! extract, prompt_caching, kanban, dashboard, tts, audio_cache.
//! They will be made GREEN by Plan 03 (cli-config.yaml.example parity).

use ironhermes_core::config::Config;

/// Load cli-config.yaml.example as a raw serde_yaml::Value.
/// Path is relative to the crate manifest dir, two levels up to the workspace root.
fn load_example_yaml() -> serde_yaml::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli-config.yaml.example");
    let contents = std::fs::read_to_string(path).expect("cli-config.yaml.example must exist");
    serde_yaml::from_str(&contents).expect("cli-config.yaml.example must be valid YAML")
}

/// All 27 top-level Config sections that cli-config.yaml.example must document.
/// Derived from the typed Config struct (RESEARCH.md §Q2).
const REQUIRED_SECTIONS: &[&str] = &[
    "model",
    "agent",
    "terminal",
    "web",
    "gateway",
    "cron",
    "security",
    "rate_limit",
    "skills",
    "exec",
    "delegation",
    "batch",
    "memory",
    "compression",
    "providers",
    "custom_providers",
    "mcp_servers",
    "autonomous",
    "tools",
    "auxiliary",
    "browser",
    "extract",
    "prompt_caching",
    "kanban",
    "dashboard",
    "tts",
    "audio_cache",
];

/// REQ-37.1-02: cli-config.yaml.example must contain all 27 Config top-level section keys.
/// WAVE-0 RED: This test FAILS today because the example is missing mcp_servers, autonomous,
/// browser, extract, prompt_caching, kanban, dashboard, tts, audio_cache.
/// Turns GREEN in Plan 03.
#[test]
fn example_covers_all_typed_config_sections() {
    let example = load_example_yaml();
    let map = example
        .as_mapping()
        .expect("cli-config.yaml.example top level must be a YAML mapping");
    for section in REQUIRED_SECTIONS {
        assert!(
            map.contains_key(&serde_yaml::Value::String(section.to_string())),
            "cli-config.yaml.example is missing section: `{section}` — \
             add a commented-default block for this Config field (REQ-37.1-02)"
        );
    }
}

/// REQ-37.1-02: The example file must deserialize cleanly to Config (no unknown-key errors,
/// no missing required fields).
/// WAVE-0 RED: This test FAILS today once the example gains new sections that the typed
/// Config does not yet have serde-unknown-field handling for, or if any required typed
/// field is missing.
/// Turns GREEN in Plan 03 (after both the example and any serde annotations are aligned).
#[test]
fn example_deserializes_to_config_without_errors() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli-config.yaml.example");
    let contents = std::fs::read_to_string(path).unwrap();
    let _: Config = serde_yaml::from_str(&contents)
        .expect("cli-config.yaml.example must deserialize cleanly to Config (REQ-37.1-02)");
}

/// REQ-37.1-02 (Open Question #3): The `kanban` key must exist at the top level.
/// Kanban coverage is KEY-presence only (not field-level — that is KanbanConfig's job).
/// WAVE-0 RED: This test FAILS today because cli-config.yaml.example has no `kanban:` section.
/// Turns GREEN in Plan 03.
#[test]
fn kanban_key_present() {
    let example = load_example_yaml();
    let map = example
        .as_mapping()
        .expect("cli-config.yaml.example top level must be a YAML mapping");
    assert!(
        map.contains_key(&serde_yaml::Value::String("kanban".to_string())),
        "cli-config.yaml.example is missing top-level `kanban:` key (REQ-37.1-02, D-09)"
    );
}
