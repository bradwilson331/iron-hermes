//! Public CLI surface (Phase 21.7 Plan 08, ISS-06).
//!
//! Mirrors the clap structure defined in `main.rs` so integration tests
//! can exercise the parser WITHOUT spawning the binary. The `--yolo`
//! flag placement (present on `Chat` + top-level, absent on `Gateway`)
//! is the ONLY invariant this module is currently called on to protect
//! — INV-21.7-10 parses `hermes gateway --yolo` here and asserts clap
//! rejects it (D-12 / INV-21.7-05).
//!
//! NOTE: This is a lib-reachable mirror. The binary's `main.rs` keeps
//! its own `Cli`/`Commands` definition so the real entry point remains
//! self-contained. If the binary's CLI shape drifts, update this mirror
//! to match — the `invariant_21_7_10_gateway_subcommand_rejects_yolo_flag`
//! test is the canary.

use clap::{Parser, Subcommand};

/// Top-level CLI surface (lib-reachable mirror of `main::Cli`).
///
/// Only the fields that matter for static parse-level invariants are
/// included here. The binary's `Cli` is the live entry point; this copy
/// exists so tests can call `Cli::try_parse_from(...)` without linking
/// to the binary crate.
#[derive(Parser, Debug)]
#[command(
    name = "ironhermes",
    about = "IronHermes — The self-improving AI agent, rewritten in Rust",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Model to use (e.g., "anthropic/claude-sonnet-4-20250514")
    #[arg(short, long)]
    pub model: Option<String>,

    /// Provider (openrouter, anthropic, openai)
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Enable streaming output
    #[arg(short, long, default_value_t = true)]
    pub stream: bool,

    /// Maximum iterations for the agent loop
    #[arg(long)]
    pub max_turns: Option<usize>,

    /// Run a single prompt non-interactively
    #[arg(short = 'e', long = "execute")]
    pub execute: Option<String>,

    /// Quiet mode (less output)
    #[arg(short, long)]
    pub quiet: bool,

    /// Phase 21.7 Plan 08 (D-11 / D-12): enable autonomous (yolo) mode
    /// for the batch (`-e`) entry point. Blanket-bypasses dangerous-
    /// command approval. Gateway path does NOT expose this flag.
    #[arg(long)]
    pub yolo: bool,
}

/// Subcommands — mirrors `main::Commands` shape for parse-level tests.
/// Field payloads are trimmed to only what the tests need.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Interactive chat mode (default).
    ///
    /// D-11 / D-12: `--yolo` is accepted HERE (Chat) and on the top-
    /// level batch entry, but NOT on `Gateway` (D-12).
    Chat {
        /// Initial message to send.
        message: Option<String>,
        /// Plan 21.7 Plan 08: enable autonomous (yolo) mode.
        #[arg(long)]
        yolo: bool,
    },
    /// Show current configuration and status.
    Status,
    /// Check configuration and dependencies.
    Doctor,
    /// Show version information.
    Version,
    /// Start the Telegram gateway bot.
    ///
    /// INV-21.7-10 / D-12: MUST NOT accept `--yolo`. Gateway reads
    /// autonomous.yolo from the on-disk config only.
    Gateway {
        /// Override Telegram bot token (or set TELEGRAM_BOT_TOKEN env var).
        #[arg(long)]
        token: Option<String>,
    },
}
