//! Tier 2 long-term memory (ADR-0008, phase 58.2).
//!
//! A standalone subsystem beside Tier 1 (`MEMORY.md` / `USER.md`): a per-profile SQLite claim
//! store under `<home>/tier2/`, plus the search service behind `memory_search`.

pub mod consolidate;
pub mod dreams;
pub mod embed;
pub mod forget;
pub mod index;
pub mod live;
pub mod llm;
pub mod paths;
pub mod render;
pub mod search;
pub mod service;
pub mod store;
pub mod synthesis;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use paths::Tier2Paths;
pub use service::Tier2Service;
