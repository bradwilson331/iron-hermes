//! Handle resolution + fallback policies + self-mention quick reject.
//!
//! # Overview
//!
//! `resolve_mention(handle, &ctx)` maps a raw `@handle` string (as produced
//! by [`super::parser::parse_mentions`]) to one of three outcomes:
//!
//! - [`Resolution::Resolved`]  — handle is valid and known.
//! - [`Resolution::Fallback`]  — handle is valid but unknown; `Pending` policy.
//! - [`Resolution::Skipped`]   — handle is malformed, self-referential, or
//!   unknown when the policy is `Skip` or `Error`.
//!
//! # Pipeline (per call)
//!
//! 1. Lowercase the handle (D-03 step 1 + Landmine 5).
//! 2. Validate via `ironhermes_core::profile::validate_profile_name` — malformed → `Skipped(Malformed)`.
//! 3. Self-mention quick reject — constant-time string compare against `ctx.parent_assignee`.
//! 4. Existence gate — check `ctx.known_assignees`.
//! 5. Apply `ctx.fallback_policy` for unknown handles.
//!
//! # What this module does NOT do
//!
//! Ancestor-chain depth checking (REQ-10 layer 2) lives in
//! `store.rs::create_mention_children` (Plan 02). This module exposes only
//! `MAX_MENTION_CHAIN_DEPTH` as a constant for the store to consume.

use std::collections::HashSet;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum ancestor-chain depth checked before skipping with `Cycle`.
///
/// Exported so that `store.rs::create_mention_children` (Plan 02) and tests
/// can consume the same constant without re-defining it (REQ-10).
pub const MAX_MENTION_CHAIN_DEPTH: usize = 4;

// ---------------------------------------------------------------------------
// Public enums
// ---------------------------------------------------------------------------

/// Controls what happens when `resolve_mention` encounters an unknown handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackPolicy {
    /// Drop the mention silently and record a skip reason. This is the default.
    Skip,
    /// Materialize a child anyway using the literal handle as the assignee name.
    /// Requires that the normalized handle passes `validate_profile_name`
    /// (guaranteed by the resolver pipeline), so `"pending"` itself is a valid
    /// profile name under the rules of `validate_profile_name`.
    Pending,
    /// Signal a hard error to the caller. The resolver returns
    /// `Skipped(UnknownHandle)` — the caller MUST detect
    /// `fallback_policy == Error && matches!(r, Skipped(UnknownHandle))` and
    /// reject the entire batch. See [`Resolution::Skipped`] doc comment.
    Error,
}

impl FromStr for FallbackPolicy {
    type Err = anyhow::Error;

    /// Parse from lowercase string `"skip"`, `"pending"`, or `"error"`.
    ///
    /// Any other value (including uppercase variants or empty string) returns
    /// `Err`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "skip" => Ok(FallbackPolicy::Skip),
            "pending" => Ok(FallbackPolicy::Pending),
            "error" => Ok(FallbackPolicy::Error),
            other => Err(anyhow::anyhow!("invalid fallback policy: {other}")),
        }
    }
}

/// Reason attached to a [`Resolution::Fallback`] variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// The handle did not appear in `ctx.known_assignees`.
    UnknownHandle,
}

/// Reason a mention was skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Handle failed `validate_profile_name` (empty, reserved, invalid chars, etc.).
    Malformed,
    /// Resolved handle equals the parent task's assignee — self-delegation
    /// creates a busy-loop and adds no value (REQ-09, D-04 layer 1).
    SelfReference,
    /// Reserved for a future resolver-side ancestor-chain pre-check.
    ///
    /// WR-04 (Phase 36.3.7.8 code review): the resolver does NOT currently
    /// produce this variant. Today, ancestor-chain cycle detection (D-04
    /// layer 2) lives exclusively in `store.rs::create_mention_children` and
    /// surfaces as a whole-batch `KanbanError::Other("mention_cycle: ...")`
    /// — NOT as a per-handle skip. The variant is kept so that a future
    /// resolver-side pre-check (e.g. for cost-avoiding batch aborts) has a
    /// stable shape to fill, but downstream code MUST treat it as
    /// `#[allow(unreachable_patterns)]` until a producer is wired up.
    Cycle,
    /// Handle is valid but unknown and the policy is `Skip` or `Error`.
    ///
    /// When `ctx.fallback_policy == Error`, the caller MUST treat a
    /// `Skipped(UnknownHandle)` return as a hard error and abort the entire
    /// batch — do NOT commit partial results. The resolver itself does not
    /// panic; it always returns a `Resolution`.
    UnknownHandle,
}

/// Outcome of resolving one `@handle` mention.
///
/// The `Error` fallback policy is signalled at the caller level:
/// check `fallback_policy == FallbackPolicy::Error && matches!(r, Resolution::Skipped(SkipReason::UnknownHandle))`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Handle resolved to a validated, known profile name.
    Resolved(String),
    /// Handle is valid but unknown; `Pending` policy was active. The string is
    /// the assignee name to use (`"pending"` — a valid profile name).
    Fallback(String, FallbackReason),
    /// Handle was skipped. See [`SkipReason`] for details.
    ///
    /// When `ctx.fallback_policy == FallbackPolicy::Error` and the reason is
    /// [`SkipReason::UnknownHandle`], the caller MUST abort the batch.
    Skipped(SkipReason),
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Immutable context passed to [`resolve_mention`].
///
/// This is a pure-function context — it carries NO database connection.
/// Ancestor-chain walking (D-04 layer 2) requires a DB connection and lives in
/// `store.rs::create_mention_children` (Plan 02, Landmine 3).
pub struct ResolverCtx<'a> {
    /// ID of the task whose body contains the `@mention` (used by Plan 02 for
    /// ancestor-chain lookups — stored here for forward compatibility).
    pub parent_task_id: String,
    /// Validated, lowercased assignee of the parent task (used for self-mention
    /// quick reject, D-04 layer 1).
    pub parent_assignee: String,
    /// Set of known assignee profile names (all lowercase, all pre-validated).
    /// Used as the existence gate in step 4 of the pipeline.
    pub known_assignees: &'a HashSet<String>,
    /// Policy applied when a valid handle is not in `known_assignees`.
    pub fallback_policy: FallbackPolicy,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve a raw `@handle` string to a [`Resolution`].
///
/// # Pipeline
///
/// 1. Lowercase the handle.
/// 2. Run `validate_profile_name` — malformed → `Skipped(Malformed)`.
/// 3. Self-mention check — resolves to parent assignee → `Skipped(SelfReference)`.
/// 4. Existence gate — in `known_assignees` → `Resolved`.
/// 5. Apply `ctx.fallback_policy` for unknown handles.
///
/// Never panics on malformed input.
pub fn resolve_mention(handle: &str, ctx: &ResolverCtx<'_>) -> Resolution {
    // Step 1 — normalize to lowercase (D-03 step 1 + Landmine 5).
    let normalized = handle.to_lowercase();

    // Step 2 — validate profile name (must come before self-mention check so
    // that a self-referential but malformed handle is rejected as Malformed,
    // not SelfReference).
    let name = match ironhermes_core::profile::validate_profile_name(&normalized) {
        Ok(n) => n,
        Err(_) => return Resolution::Skipped(SkipReason::Malformed),
    };

    // Step 3 — self-mention quick reject (constant-time string compare, D-04
    // layer 1). Must happen BEFORE the existence gate so that a self-mention
    // from an unknown handle is reported as SelfReference, not UnknownHandle.
    if name == ctx.parent_assignee {
        return Resolution::Skipped(SkipReason::SelfReference);
    }

    // Step 4 — existence gate.
    if ctx.known_assignees.contains(&name) {
        return Resolution::Resolved(name);
    }

    // Step 5 — apply fallback policy.
    match ctx.fallback_policy {
        FallbackPolicy::Skip => Resolution::Skipped(SkipReason::UnknownHandle),
        FallbackPolicy::Pending => {
            // `"pending"` is itself a valid profile name under
            // `validate_profile_name` (confirmed in RESEARCH.md §Pitfall 5).
            Resolution::Fallback("pending".to_string(), FallbackReason::UnknownHandle)
        }
        FallbackPolicy::Error => {
            // Caller detects: `fallback_policy == Error && matches!(r, Skipped(UnknownHandle))`.
            Resolution::Skipped(SkipReason::UnknownHandle)
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx<'a>(
        parent_assignee: &str,
        known: &'a HashSet<String>,
        policy: FallbackPolicy,
    ) -> ResolverCtx<'a> {
        ResolverCtx {
            parent_task_id: "t_test001".to_string(),
            parent_assignee: parent_assignee.to_string(),
            known_assignees: known,
            fallback_policy: policy,
        }
    }

    // ------------------------------------------------------------------
    // 1. Three resolution variants: Resolved / Fallback / Skipped
    // ------------------------------------------------------------------
    #[test]
    fn three_resolution_variants() {
        let known: HashSet<String> = ["reviewer".to_string()].into_iter().collect();

        // Resolved path
        let ctx = make_ctx("alice", &known, FallbackPolicy::Skip);
        assert_eq!(
            resolve_mention("reviewer", &ctx),
            Resolution::Resolved("reviewer".to_string())
        );

        // Fallback (Pending policy, unknown handle)
        let ctx = make_ctx("alice", &known, FallbackPolicy::Pending);
        assert_eq!(
            resolve_mention("ghost", &ctx),
            Resolution::Fallback("pending".to_string(), FallbackReason::UnknownHandle)
        );

        // Skipped (Skip policy, unknown handle)
        let ctx = make_ctx("alice", &known, FallbackPolicy::Skip);
        assert_eq!(
            resolve_mention("ghost", &ctx),
            Resolution::Skipped(SkipReason::UnknownHandle)
        );
    }

    // ------------------------------------------------------------------
    // 2. Case-insensitive handle resolution (@Reviewer → "reviewer")
    // ------------------------------------------------------------------
    #[test]
    fn case_insensitive_handle() {
        let known: HashSet<String> = ["reviewer".to_string()].into_iter().collect();
        let ctx = make_ctx("alice", &known, FallbackPolicy::Skip);

        // Mixed case
        assert_eq!(
            resolve_mention("Reviewer", &ctx),
            Resolution::Resolved("reviewer".to_string())
        );
        // All caps
        assert_eq!(
            resolve_mention("REVIEWER", &ctx),
            Resolution::Resolved("reviewer".to_string())
        );
        // Lowercase
        assert_eq!(
            resolve_mention("reviewer", &ctx),
            Resolution::Resolved("reviewer".to_string())
        );
    }

    // ------------------------------------------------------------------
    // 3. Malformed handles return Skipped(Malformed)
    // ------------------------------------------------------------------
    #[test]
    fn malformed_handle_returns_skipped() {
        let known: HashSet<String> = HashSet::new();
        let ctx = make_ctx("alice", &known, FallbackPolicy::Skip);

        // Leading underscore
        assert_eq!(
            resolve_mention("_underscore", &ctx),
            Resolution::Skipped(SkipReason::Malformed)
        );
        // Reserved name "default"
        assert_eq!(
            resolve_mention("default", &ctx),
            Resolution::Skipped(SkipReason::Malformed)
        );
        // Reserved name "current"
        assert_eq!(
            resolve_mention("current", &ctx),
            Resolution::Skipped(SkipReason::Malformed)
        );
        // Reserved name "none"
        assert_eq!(
            resolve_mention("none", &ctx),
            Resolution::Skipped(SkipReason::Malformed)
        );
        // Empty string
        assert_eq!(
            resolve_mention("", &ctx),
            Resolution::Skipped(SkipReason::Malformed)
        );
        // Too long (65 chars)
        let too_long = "a".repeat(65);
        assert_eq!(
            resolve_mention(&too_long, &ctx),
            Resolution::Skipped(SkipReason::Malformed)
        );
    }

    // ------------------------------------------------------------------
    // 4. Self-mention returns Skipped(SelfReference) — beats UnknownHandle
    // ------------------------------------------------------------------
    #[test]
    fn self_mention_returns_skipped_self_reference() {
        let known: HashSet<String> = HashSet::new(); // alice is NOT in known_assignees
        let ctx = make_ctx("alice", &known, FallbackPolicy::Skip);

        // Self-mention should be SelfReference, not UnknownHandle
        assert_eq!(
            resolve_mention("alice", &ctx),
            Resolution::Skipped(SkipReason::SelfReference)
        );

        // Case-folded self-mention also produces SelfReference
        assert_eq!(
            resolve_mention("Alice", &ctx),
            Resolution::Skipped(SkipReason::SelfReference)
        );
    }

    // ------------------------------------------------------------------
    // 5. Unknown handle respects all three fallback policies
    // ------------------------------------------------------------------
    #[test]
    fn unknown_handle_respects_fallback_policy() {
        let known: HashSet<String> = ["reviewer".to_string()].into_iter().collect();

        // Skip policy → Skipped(UnknownHandle)
        let ctx = make_ctx("alice", &known, FallbackPolicy::Skip);
        assert_eq!(
            resolve_mention("ghost", &ctx),
            Resolution::Skipped(SkipReason::UnknownHandle)
        );

        // Pending policy → Fallback("pending", UnknownHandle)
        let ctx = make_ctx("alice", &known, FallbackPolicy::Pending);
        assert_eq!(
            resolve_mention("ghost", &ctx),
            Resolution::Fallback("pending".to_string(), FallbackReason::UnknownHandle)
        );

        // Error policy → Skipped(UnknownHandle) [caller must treat as hard error]
        let ctx = make_ctx("alice", &known, FallbackPolicy::Error);
        let result = resolve_mention("ghost", &ctx);
        assert_eq!(result, Resolution::Skipped(SkipReason::UnknownHandle));
        // Caller contract: detect Error + UnknownHandle combination.
        assert!(
            ctx.fallback_policy == FallbackPolicy::Error
                && matches!(result, Resolution::Skipped(SkipReason::UnknownHandle))
        );
    }

    // ------------------------------------------------------------------
    // 6. FallbackPolicy::from_str parses three values and rejects others
    // ------------------------------------------------------------------
    #[test]
    fn fallback_policy_from_str_parses_three_values_and_rejects_others() {
        assert_eq!(
            "skip".parse::<FallbackPolicy>().unwrap(),
            FallbackPolicy::Skip
        );
        assert_eq!(
            "pending".parse::<FallbackPolicy>().unwrap(),
            FallbackPolicy::Pending
        );
        assert_eq!(
            "error".parse::<FallbackPolicy>().unwrap(),
            FallbackPolicy::Error
        );

        // Uppercase rejected
        assert!("SKIP".parse::<FallbackPolicy>().is_err());
        // Mixed case rejected
        assert!("Skip".parse::<FallbackPolicy>().is_err());
        // Unrecognized value rejected
        assert!("abort".parse::<FallbackPolicy>().is_err());
        // Empty string rejected
        assert!("".parse::<FallbackPolicy>().is_err());
    }
}
