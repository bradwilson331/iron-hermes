#!/usr/bin/env bash
# Phase 21.7 CI gates — runnable locally and from .github/workflows/ci.yml.
#
# Each gate maps to an AI-SPEC §5 eval dimension + a locked CONTEXT decision.
# For the three gates that already exist as Rust tests (E-05, E-08, E-09) we
# call `cargo test` instead of duplicating them as shell greps — the
# #[test] names are authoritative and fail with a richer diagnostic. The
# "no per-request yolo" gate (D-12) is a straight static-grep since there's
# no concrete runtime surface to probe.
#
# Exit 0 on all-pass; non-zero on any fail.

set -euo pipefail

# Run from the workspace root (directory containing Cargo.toml / crates/).
# Resolve relative to this script so `bash scripts/ci-gates.sh` from any cwd
# still finds the right path.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

echo "==> Phase 21.7 CI gates (workspace: ${WORKSPACE_ROOT})"
echo

# -----------------------------------------------------------------------------
# Gate 1 / E-05 / AI-SPEC Pitfall 9:
#   BudgetHandle must use only SeqCst ordering (no Ordering::Relaxed).
#   Rust test lives at crates/ironhermes-agent/tests/budget_ordering_grep.rs.
# -----------------------------------------------------------------------------
echo "--> Gate 1 (E-05): BudgetHandle SeqCst-only ordering"
cargo test -p ironhermes-agent --test budget_ordering_grep --quiet
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 2 / E-08 / AI-SPEC Pitfall 3:
#   Transcript writer path must never .unwrap() or .expect(...) — all write
#   errors resolve to tracing::warn and are swallowed (fire-and-forget).
#   Rust test: crates/ironhermes-agent/tests/transcript_no_unwrap_lint.rs.
# -----------------------------------------------------------------------------
echo "--> Gate 2 (E-08): transcript writer fire-and-forget (no unwrap/expect)"
cargo test -p ironhermes-agent --test transcript_no_unwrap_lint --quiet
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 3 / E-09 / AI-SPEC Pitfall 1:
#   Three-site wiring parity — AgentSubagentRunner::new,
#   register_delegate_task_tool, register_execute_code_tool_with_* each
#   appear in exactly 3 call sites across main.rs (plus the gateway drain
#   and subagent registry / transcript / yolo / budget threading checks).
#   Rust test: crates/ironhermes-cli/tests/invariants_21_7.rs — covers
#   INV-21.7-01 through INV-21.7-11 (eleven invariants).
# -----------------------------------------------------------------------------
echo "--> Gate 3 (E-09): three-site wiring parity + phase invariants"
cargo test -p ironhermes-cli --test invariants_21_7 --quiet
echo "    OK"
echo

# -----------------------------------------------------------------------------
# Gate 4 / D-12 / INV-21.7-05:
#   Gateway + main.rs must NOT read a per-request yolo field — yolo is a
#   process-scoped flag (--yolo) and a config file setting; it is NEVER
#   trust-elevated by individual inbound messages.
# -----------------------------------------------------------------------------
echo "--> Gate 4 (D-12): no per-request yolo reads in gateway or CLI"
if grep -RE 'request\.yolo|req\.yolo' \
        crates/ironhermes-gateway/src \
        crates/ironhermes-cli/src/main.rs \
        > /dev/null 2>&1; then
    echo "    GATE FAIL (D-12): per-request yolo read detected. Offending lines:"
    grep -RnE 'request\.yolo|req\.yolo' \
         crates/ironhermes-gateway/src \
         crates/ironhermes-cli/src/main.rs || true
    exit 1
fi
echo "    OK"
echo

echo "==> All 21.7 CI gates green."
