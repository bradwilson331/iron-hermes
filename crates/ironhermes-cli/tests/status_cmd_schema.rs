//! E-07 / D-20 — v1 schema stability snapshot.

use ironhermes_cli::status_cmd::StatusReport;

#[test]
fn status_report_v1_json_schema_snapshot() {
    let snap = StatusReport::fixture();
    let json = serde_json::to_string_pretty(&snap).expect("serialize fixture");
    insta::assert_snapshot!("status_report_v1", json);
}

#[test]
fn status_report_round_trips() {
    let original = StatusReport::fixture();
    let json = serde_json::to_string(&original).unwrap();
    let back: StatusReport = serde_json::from_str(&json).unwrap();
    assert_eq!(back, original, "E-07: schema must round-trip losslessly");
}

#[test]
fn top_level_keys_are_exactly_seven() {
    // Lock the v1 top-level shape (AI-SPEC §4 "JSON status schema").
    let json = serde_json::to_value(StatusReport::fixture()).unwrap();
    let obj = json.as_object().expect("object");
    let keys: std::collections::HashSet<&str> = obj.keys().map(|s| s.as_str()).collect();
    let expected: std::collections::HashSet<&str> = [
        "provider",
        "memory",
        "gateway",
        "subagents",
        "processes",
        "mcp",
        "yolo",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        keys, expected,
        "D-18 / D-20 v1 top-level keys: missing or extra key detected. \
         Expected {:?}, got {:?}",
        expected, keys
    );
}
