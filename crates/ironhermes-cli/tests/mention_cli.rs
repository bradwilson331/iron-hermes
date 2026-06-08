//! Phase 36.3.7.8 — CLI parity smoke tests for `hermes kanban mention`.
//!
//! Tests parse argv through a local `TestCli` wrapper using clap-derive
//! to verify that `KanbanCommands::Mention { ... }` is reachable.
//! They do NOT exercise `cmd_mention` end-to-end (those tests live in Plan 05's
//! `tools_smoke.rs` which exercises the full parser+resolver+store path).
//!
//! NOTE: `KanbanCommands` does NOT derive `Debug` (Phase 36.3.7 baseline; not
//! changed in this phase). So we cannot derive Debug on TestSub either — the
//! `{other:?}` panic-message form is unavailable. Tests use a plain string
//! panic if the match arm is wrong, which is sufficient for parse-shape
//! verification (PATTERNS.md Landmine 7).

use clap::Parser;
use ironhermes_cli::kanban::KanbanCommands;

// ---------------------------------------------------------------------------
// TestCli scaffold — mirrors swarm_cli.rs wrapper verbatim
// ---------------------------------------------------------------------------

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    cmd: TestSub,
}

#[derive(clap::Subcommand)]
enum TestSub {
    Kanban {
        #[command(subcommand)]
        sub: KanbanCommands,
    },
}

// ---------------------------------------------------------------------------
// REQ-14 locked test names (4 required)
// ---------------------------------------------------------------------------

/// REQ-14 test 1: positional task_id parses correctly; fallback_policy defaults to "skip".
#[test]
fn mention_verb_parses_minimal() {
    let parsed = TestCli::try_parse_from(["hermes", "kanban", "mention", "t_abc123"])
        .expect("parse should succeed for `kanban mention t_abc123`");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Mention {
                    task_id,
                    fallback_policy,
                    idempotency_key,
                    body_override,
                    json,
                },
        } => {
            assert_eq!(task_id.as_deref(), Some("t_abc123"));
            assert_eq!(fallback_policy, "skip"); // default_value
            assert!(idempotency_key.is_none());
            assert!(body_override.is_none());
            assert!(!json);
        }
        _ => panic!("expected Kanban::Mention variant"),
    }
}

/// REQ-14 test 2: --fallback-policy flag overrides the "skip" default.
#[test]
fn mention_verb_parses_fallback_policy_flag() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "mention",
        "t_abc",
        "--fallback-policy",
        "pending",
    ])
    .expect("parse should succeed for `kanban mention t_abc --fallback-policy pending`");
    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Mention {
                fallback_policy, ..
            },
        } => {
            assert_eq!(fallback_policy, "pending");
        }
        _ => panic!("expected Kanban::Mention variant"),
    }
}

/// REQ-14 test 3: --idempotency-key flag is captured correctly.
#[test]
fn mention_verb_parses_idempotency_key_flag() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "mention",
        "t_abc",
        "--idempotency-key",
        "my-key",
    ])
    .expect("parse should succeed for `kanban mention t_abc --idempotency-key my-key`");
    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Mention {
                idempotency_key, ..
            },
        } => {
            assert_eq!(idempotency_key.as_deref(), Some("my-key"));
        }
        _ => panic!("expected Kanban::Mention variant"),
    }
}

/// REQ-14 test 4: all flags together produce no clap conflict errors;
/// verifies the full dispatch-ready invocation shape.
#[test]
fn mention_verb_parses_dispatch_happy_path() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "mention",
        "t_abc",
        "--fallback-policy",
        "error",
        "--idempotency-key",
        "k1",
        "--body-override",
        "@reviewer please",
        "--json",
    ])
    .expect("parse should succeed with all flags present");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Mention {
                    task_id,
                    fallback_policy,
                    idempotency_key,
                    body_override,
                    json,
                },
        } => {
            assert_eq!(task_id.as_deref(), Some("t_abc"));
            assert_eq!(fallback_policy, "error");
            assert_eq!(idempotency_key.as_deref(), Some("k1"));
            assert_eq!(body_override.as_deref(), Some("@reviewer please"));
            assert!(json);
        }
        _ => panic!("expected Kanban::Mention variant"),
    }
}
