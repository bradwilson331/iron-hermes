# GSD Command Reference — IronHermes

Reference for the GSD slash commands exercised in this project. Commands operate on `.planning/` (ROADMAP.md, STATE.md, phase directories) and commit changes through `gsd-sdk`.

---

## `/gsd-phase` — manage phases in ROADMAP.md

Single consolidated entry point. Behavior depends on the leading flag.

### Default — append a new integer phase

```
/gsd-phase <description>
```

Adds a new integer phase at the end of the current milestone (e.g., `Phase 27`, `Phase 28`). Use for planned work that fits at the tail of the roadmap.

### `--insert` — insert urgent decimal phase between existing phases

```
/gsd-phase --insert <after-phase-number> <description>
```

- `<after-phase-number>` is the **integer phase to insert after** (e.g., `25`, not `25.7`).
- The CLI auto-calculates the next available decimal under that integer (`25.1`, `25.2`, …, picking the lowest free slot).
- Phase entry is tagged `(INSERTED)` in ROADMAP.md and recorded in STATE.md's `### Roadmap Evolution` log with `(URGENT)`.
- Does **not** displace `Current Position` if you're mid-execution on a later phase — the insertion is recorded out-of-band.

**Example (this session):**
```
/gsd-phase --insert 25 registering all skills in .ironhermes/skills and .ironhermes/optional-skills on install or commandline skills --scan <PATH> option
```
→ Created `.planning/phases/25.7-registering-all-skills-…/` (next free decimal under 25 since 25.1–25.6 already existed).

### `--remove` — delete a future/unstarted phase

```
/gsd-phase --remove <phase-number>
```

- Accepts integer (`17`) or decimal (`16.1`).
- Deletes the phase directory, renumbers any subsequent same-tier phases, updates ROADMAP.md + STATE.md, and creates a `chore: remove phase …` commit. The git commit is the historical record — no "removed" note is left in STATE.md.
- Refuses if the phase has executed plans (SUMMARY.md present) unless `--force` is added with explicit confirmation.
- Numerical-position guard: by default the workflow blocks removal of phases at or before `Current Position`. For freshly inserted decimal phases that are numerically *less than* an in-flight later phase, the guard is bypassed when the directory is empty (no plans, no SUMMARY) — that case is recognized as "undo a recent insert" rather than "abandon current work".

**Example (this session):**
```
/gsd-phase --remove 25.7
```
→ Deleted the just-inserted empty 25.7 directory, committed `a2033f3b chore: remove phase 25.7 (...)`.

### `--edit` — edit fields of an existing phase in place

```
/gsd-phase --edit <phase-number> [--force]
```

Edit any field (name, description, requirements, depends-on) of an existing phase. `--force` skips the in-place confirmation. Not exercised this session — listed for completeness.

---

## `/gsd-validate-phase` — Nyquist validation audit

```
/gsd-validate-phase <phase-number>
```

Audits a completed phase's validation coverage. Phase argument is optional (defaults to last completed phase).

**Three states it handles:**

| State | Trigger | Behavior |
|-------|---------|----------|
| **A** | `*-VALIDATION.md` exists | Audit existing — gap-fill or update frontmatter to match reality |
| **B** | No VALIDATION.md, but SUMMARY.md files exist | Reconstruct VALIDATION.md from artifacts via the template |
| **C** | No SUMMARY.md files | Exit — phase not executed yet |

**What it does:**
1. Reads all `*-PLAN.md` and `*-SUMMARY.md` files in the phase dir.
2. Builds a requirement → task → test map.
3. Detects test infrastructure (parses from existing VALIDATION.md in State A; filesystem scan in State B).
4. Cross-references each requirement against existing test files; classifies as `COVERED` / `PARTIAL` / `MISSING`.
5. If gaps exist: spawns `gsd-nyquist-auditor` agent to write missing tests. If gaps == 0: jumps straight to doc update.
6. Updates `*-VALIDATION.md`:
   - Frontmatter: `status`, `nyquist_compliant`, `wave_0_complete`, `audited` date.
   - Per-Decision (or Per-Task) verification map populated with real test names.
   - Audit trail appended (`## Validation Audit YYYY-MM-DD`) with metrics: gaps found / resolved / escalated.
7. Commits the doc update via `gsd-sdk query commit "docs(phase-N): …"`.

**Example (this session):**
```
/gsd-validate-phase 21.8
```
→ Audited 24 locked decisions + 5 Wave 0 items + 3 manual-only items. Zero gaps. Flipped `nyquist_compliant: false → true`, populated the Per-Decision Map with ~30 real test names from the SUMMARY outputs, appended audit trail. Committed `4f22aefa docs(phase-21.8): mark validation nyquist-compliant after retroactive audit`.

**Manual-only items:** the workflow treats decisions deliberately marked manual-only (e.g., live-endpoint UX review, agent-restart activation) as covered when UAT signs them off. They appear in the `## Manual-Only Verifications` table with their UAT result, not in the gap list.

---

## Related GSD commands (not exercised this session)

- `/gsd-progress` — see updated roadmap status after phase manipulation
- `/gsd-plan-phase <N>` — create PLAN.md for a phase (run after `/gsd-phase --insert` or default add)
- `/gsd-execute-phase <N>` — wave-based execution of all plans in a phase
- `/gsd-verify-work <N>` — UAT-style verification of completed phase
- `/gsd-audit-milestone` — milestone-level completion audit
- `/gsd-undo` — safe git revert for phase or plan commits

Run `/gsd-help` for the full command list with current toggles.

---

## State files touched

| File | Updated by |
|------|-----------|
| `.planning/ROADMAP.md` | `--insert`, `--remove`, `--edit`, default add |
| `.planning/STATE.md` | `--insert` (Roadmap Evolution log), `--remove` (count decrement), default add |
| `.planning/phases/<N>-<slug>/` | `--insert` and default add create; `--remove` deletes |
| `.planning/phases/<N>-<slug>/<N>-VALIDATION.md` | `/gsd-validate-phase` writes/updates |

All commits routed through `gsd-sdk query commit` so they pick up the project's commit style and pre-commit hooks.
