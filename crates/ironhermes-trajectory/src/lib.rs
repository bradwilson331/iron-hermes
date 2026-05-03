//! Append-only JSONL trajectory ledger for IronHermes.
//!
//! Phase 25.3 D-T-1 / D-T-2: writes per-tool-call records to
//! `<workspace-or-home>/.ironhermes/sessions/<id>/trajectories.jsonl` from all
//! four dispatch surfaces (CLI run_single, classic-TUI run_chat, ratatui REPL,
//! Telegram gateway). Format is IronHermes-original — see RESEARCH.md
//! "CRITICAL FINDING" — the upstream Python `agent/trajectory.py` is session-level
//! ShareGPT, NOT per-tool-call. D-T-1 is the authoritative spec.
//!
//! Plan 1 (this plan): scaffold + format spec + golden test.
//! Plan 4: TrajectoryWriter (append-only, fsync per line, Drop sync_data) + TrajectoryReader.
//! Plan 9: AgentLoop callback wires writer.append() after each tool result.

pub mod format;

pub use format::{ImpactLevel, TrajectoryEntry};
