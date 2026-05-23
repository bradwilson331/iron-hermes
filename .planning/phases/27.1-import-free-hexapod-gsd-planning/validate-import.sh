#!/usr/bin/env bash
# Phase 27.1 — Hexapod GSD planning import contract regression test.
# Runs in ≤5s. Exits 0 if the import contract is intact; exits non-zero with the failing INV ID otherwise.
# Authored 2026-05-10 by Phase 27.1 Plan 04 (Wave 3, Wave 0 validation deliverable).
#
# Total invariants: 15 (INV-27.1-01..15).
# Each INV-27.1-NN locks one or more CONTEXT.md decisions (D-01..D-04 + sub-decisions).

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
HEXAPOD_REPO="${HEXAPOD_REPO:-$HOME/code/Freenove_Big_Hexapod_Robot_Kit_for_Raspberry_Pi}"

REQ="$REPO_ROOT/.planning/REQUIREMENTS.md"
ROADMAP="$REPO_ROOT/.planning/ROADMAP.md"
STATE="$REPO_ROOT/.planning/STATE.md"
CTX="$REPO_ROOT/.planning/phases/27.1.1-safe-foundation/27.1.1-CONTEXT.md"
FROZEN="$HEXAPOD_REPO/.planning/FROZEN.md"

fail() { echo "FAIL [$1]: $2" >&2; exit 1; }
pass() { echo "  pass [$1]"; }

echo "== Phase 27.1 Import Contract Regression =="

# ---------------------------------------------------------------------------
# INV-27.1-01 (D-02 REQUIREMENTS class): `### Hexapod Integration` section exists exactly once
# ---------------------------------------------------------------------------
test -f "$REQ" || fail "INV-27.1-01" "REQUIREMENTS.md missing at $REQ"
n=$(grep -c "^### Hexapod Integration$" "$REQ")
[ "$n" = "1" ] || fail "INV-27.1-01" "expected exactly 1 '### Hexapod Integration' heading in REQUIREMENTS.md, found $n"
pass "INV-27.1-01"

# ---------------------------------------------------------------------------
# INV-27.1-02 (D-03 prefix contract, REQ surface): exactly 16 HXP- definitions
# ---------------------------------------------------------------------------
n=$(grep -cE "^- \[ \] \*\*HXP-(TOOL|LOCO|NAV|DOC)-[0-9]{2}\*\*:" "$REQ")
[ "$n" = "16" ] || fail "INV-27.1-02" "expected 16 HXP- definitions in REQUIREMENTS.md, found $n"
pass "INV-27.1-02"

# ---------------------------------------------------------------------------
# INV-27.1-03 (D-03 prefix contract, traceability surface): exactly 16 HXP- traceability rows
# ---------------------------------------------------------------------------
n=$(grep -cE "^\| HXP-(TOOL|LOCO|NAV|DOC)-[0-9]{2} \| Phase 27\.1\.[123] \| Pending \|$" "$REQ")
[ "$n" = "16" ] || fail "INV-27.1-03" "expected 16 HXP- traceability rows in REQUIREMENTS.md, found $n"
pass "INV-27.1-03"

# ---------------------------------------------------------------------------
# INV-27.1-04 (D-03b non-pollution): existing Phase 25 TOOL-01..05 definitions byte-stable
# ---------------------------------------------------------------------------
n=$(grep -cE "^- \[ \] \*\*TOOL-0[1-5]\*\*:" "$REQ")
[ "$n" = "5" ] || fail "INV-27.1-04" "Phase 25 toolset TOOL-01..05 definition count drifted: expected 5, found $n"
n=$(grep -cE "^\| TOOL-0[1-5] \| Phase 25 \| Pending \|$" "$REQ")
[ "$n" = "5" ] || fail "INV-27.1-04" "Phase 25 toolset TOOL-01..05 traceability count drifted: expected 5, found $n"
pass "INV-27.1-04"

# ---------------------------------------------------------------------------
# INV-27.1-05 (D-02 ROADMAP class): exactly 4 Phase 27.1 family (INSERTED) entries
# ---------------------------------------------------------------------------
test -f "$ROADMAP" || fail "INV-27.1-05" "ROADMAP.md missing"
n=$(grep -cE "^### Phase 27\.1(\.[123])?: .* \(INSERTED\)$" "$ROADMAP")
[ "$n" = "4" ] || fail "INV-27.1-05" "expected 4 Phase 27.1 family INSERTED entries, found $n"
pass "INV-27.1-05"

# ---------------------------------------------------------------------------
# INV-27.1-06 (D-01 numbering, dependency chain): correct Depends-on for each sub-phase
# ---------------------------------------------------------------------------
grep -A4 "^### Phase 27\.1\.1: " "$ROADMAP" | grep -qF "**Depends on:** Phase 27.1" \
  || fail "INV-27.1-06" "Phase 27.1.1 must Depends on: Phase 27.1"
grep -A4 "^### Phase 27\.1\.2: " "$ROADMAP" | grep -qF "**Depends on:** Phase 27.1.1" \
  || fail "INV-27.1-06" "Phase 27.1.2 must Depends on: Phase 27.1.1"
grep -A4 "^### Phase 27\.1\.3: " "$ROADMAP" | grep -qF "**Depends on:** Phase 27.1.2" \
  || fail "INV-27.1-06" "Phase 27.1.3 must Depends on: Phase 27.1.2"
pass "INV-27.1-06"

# ---------------------------------------------------------------------------
# INV-27.1-07 (D-03 prefix contract, ROADMAP surface): no bare TOOL/LOCO/NAV/DOC IDs
#   inside any Phase 27.1.x **Requirements:** line. (Bare = not preceded by HXP-.)
#   Uses perl for negative-lookbehind (macOS grep -P unavailable in /usr/bin/grep).
# ---------------------------------------------------------------------------
for phase in 27.1.1 27.1.2 27.1.3; do
  line=$(grep -A4 "^### Phase ${phase}: " "$ROADMAP" | grep "^\*\*Requirements:\*\*" || true)
  [ -n "$line" ] || fail "INV-27.1-07" "missing Requirements line for $phase"
  bare=$(echo "$line" | perl -ne 'print if /(?<!HXP-)\b(TOOL|LOCO|NAV|DOC)-[0-9]+/' || true)
  [ -n "$bare" ] && fail "INV-27.1-07" "bare ID in Requirements line for $phase: $line"
done
pass "INV-27.1-07"

# ---------------------------------------------------------------------------
# INV-27.1-08 (D-01a Phase 27 byte-stable, D-01a/D-02b Phase 28 byte-stable):
#   adjacent phases unchanged (heading-level sentinel)
# ---------------------------------------------------------------------------
grep -qF "### Phase 27: Prompt Caching" "$ROADMAP" \
  || fail "INV-27.1-08" "Phase 27 heading missing or renamed"
grep -qF "### Phase 28: Skills Trust Tiers" "$ROADMAP" \
  || fail "INV-27.1-08" "Phase 28 heading missing or renamed"
# Pre-import stub MUST be gone from Phase 27.1 block specifically (other phases may retain stubs)
grep -A3 "^### Phase 27\.1: " "$ROADMAP" | grep -qF "[Urgent work - to be planned]" \
  && fail "INV-27.1-08" "Phase 27.1 placeholder Goal '[Urgent work - to be planned]' still present"
pass "INV-27.1-08"

# ---------------------------------------------------------------------------
# INV-27.1-09 (D-02 CONTEXT class + D-04a phase dir): 27.1.1-CONTEXT.md exists
# ---------------------------------------------------------------------------
test -f "$CTX" || fail "INV-27.1-09" "27.1.1-CONTEXT.md missing at $CTX"
test -d "$(dirname "$CTX")" || fail "INV-27.1-09" "phase directory missing"
pass "INV-27.1-09"

# ---------------------------------------------------------------------------
# INV-27.1-10 (D-03a surface 3): zero un-prefixed (TOOL|LOCO|NAV|DOC)-NN in 27.1.1-CONTEXT.md
#   Uses perl for negative-lookbehind (macOS grep -P unavailable in /usr/bin/grep).
# ---------------------------------------------------------------------------
matches=$(perl -ne 'print "$.: $_" if /(?<!HXP-)\b(TOOL|LOCO|NAV|DOC)-[0-9]+/' "$CTX" || true)
if [ -n "$matches" ]; then
  echo "FAIL [INV-27.1-10]: un-prefixed IDs in 27.1.1-CONTEXT.md:" >&2
  echo "$matches" >&2
  exit 1
fi
pass "INV-27.1-10"

# ---------------------------------------------------------------------------
# INV-27.1-11 (D-02 CONTEXT class, fidelity): D-01..D-20 present in copied file
# ---------------------------------------------------------------------------
n=$(grep -cE "^- \*\*D-[0-9]{2}:\*\*" "$CTX")
[ "$n" = "20" ] || fail "INV-27.1-11" "expected 20 decisions D-01..D-20 in 27.1.1-CONTEXT.md, found $n"
# Title + footer cosmetic rewrites
grep -q "^# Phase 27.1.1: Safe Foundation - Context$" "$CTX" \
  || fail "INV-27.1-11" "title line not rewritten to '# Phase 27.1.1: Safe Foundation - Context'"
grep -qF "*Phase: 27.1.1-safe-foundation*" "$CTX" \
  || fail "INV-27.1-11" "footer slug not rewritten to '*Phase: 27.1.1-safe-foundation*'"
pass "INV-27.1-11"

# ---------------------------------------------------------------------------
# INV-27.1-12 (Claude's-Discretion log update): 3 new STATE.md Roadmap Evolution lines
# ---------------------------------------------------------------------------
test -f "$STATE" || fail "INV-27.1-12" "STATE.md missing"
n=$(grep -cE "^- Phase 27\.1\.[123] inserted after Phase 27\.1(\.[12])?: " "$STATE")
[ "$n" = "3" ] || fail "INV-27.1-12" "expected 3 sub-phase Roadmap Evolution lines in STATE.md, found $n"
# Original Phase 27.1 line still exactly once
n=$(grep -cF -- "- Phase 27.1 inserted after Phase 27: Import Free_Hexapod gsd planning (URGENT)" "$STATE")
[ "$n" = "1" ] || fail "INV-27.1-12" "Phase 27.1 parent log line count drifted: expected 1, found $n"
pass "INV-27.1-12"

# ---------------------------------------------------------------------------
# INV-27.1-13 (D-04 + D-04a): FROZEN.md exists in source repo with required content elements
# ---------------------------------------------------------------------------
if [ -f "$FROZEN" ]; then
  grep -qF "**Import date:** 2026-05-10" "$FROZEN" \
    || fail "INV-27.1-13" "FROZEN.md missing import date"
  grep -qF "**Target repository:** \`~/code/ironhermes\`" "$FROZEN" \
    || fail "INV-27.1-13" "FROZEN.md missing target repository line"
  grep -qF "Phase 27.1.x" "$FROZEN" \
    || fail "INV-27.1-13" "FROZEN.md missing Phase 27.1.x reference"
  grep -qF "**Do not create or modify any planning files in this directory.**" "$FROZEN" \
    || fail "INV-27.1-13" "FROZEN.md missing do-not-edit warning"
  grep -qF "27.1.1-safe-foundation/27.1.1-CONTEXT.md" "$FROZEN" \
    || fail "INV-27.1-13" "FROZEN.md missing pointer to IronHermes Phase 27.1.1 CONTEXT"
  pass "INV-27.1-13"
else
  echo "  skip [INV-27.1-13] FROZEN.md not present at $FROZEN (Plan 03 may not have run yet — non-fatal)"
fi

# ---------------------------------------------------------------------------
# INV-27.1-14 (D-01b + D-04a non-import): IronHermes PROJECT.md not extended with hexapod content
# ---------------------------------------------------------------------------
if [ -f "$REPO_ROOT/.planning/PROJECT.md" ]; then
  grep -q "Hexapod" "$REPO_ROOT/.planning/PROJECT.md" \
    && fail "INV-27.1-14" "PROJECT.md must NOT mention Hexapod (D-01b + D-04a — IronHermes PROJECT.md untouched by import)"
  pass "INV-27.1-14"
else
  echo "  skip [INV-27.1-14] PROJECT.md absent — cannot verify D-01b/D-04a"
fi

# ---------------------------------------------------------------------------
# INV-27.1-15 (D-04 source-byte stability, D-03b source untouched):
#   Hexapod source repo's .planning/ working tree contains ONLY FROZEN.md as a change.
#   Tolerant of both `?? .planning/FROZEN.md` (untracked) and `A  .planning/FROZEN.md`
#   (staged) statuses — committing FROZEN.md is recommended but not required (D-04).
#   Skip gracefully if the source repo is absent (same pattern as INV-27.1-13).
# ---------------------------------------------------------------------------
if [ -d "$HEXAPOD_REPO/.git" ]; then
  # `git status --porcelain` is two-column: XY <space> path. Strip the FROZEN.md
  # row regardless of its status code (?? or A or M etc.), and verify nothing else remains.
  MODS=$(git -C "$HEXAPOD_REPO" status --porcelain -- .planning/ 2>/dev/null \
           | grep -v -E '^\?\? \.planning/FROZEN\.md$' \
           | grep -v -E '^A  \.planning/FROZEN\.md$' \
           | grep -v -E '^M  \.planning/FROZEN\.md$' \
           | grep -v -E '^ M \.planning/FROZEN\.md$' \
           | grep -v -E '\.DS_Store$' \
           || true)
  if [ -n "$MODS" ]; then
    echo "FAIL [INV-27.1-15]: source repo .planning/ has unexpected modifications beyond FROZEN.md:" >&2
    echo "$MODS" >&2
    exit 1
  fi
  pass "INV-27.1-15"
else
  echo "  skip [INV-27.1-15] Hexapod source repo absent at $HEXAPOD_REPO — cannot verify D-04 source-byte stability"
fi

echo ""
echo "== All 15 Phase 27.1 import invariants passed (or skipped where source/optional files absent) =="
exit 0
