//! Phase 39.1: Concurrency primitives for async multi-turn agent execution.
//!
//! Provides the global `TurnRegistry`, the two-level `ConcurrencyLayer`, and
//! supporting types (`TurnId`, `TurnEntry`, `TurnSummary`, `Surface`).

pub mod layer;
pub mod registry;

pub use layer::ConcurrencyLayer;
pub use registry::{Surface, TurnEntry, TurnId, TurnRegistry, TurnSummary};
