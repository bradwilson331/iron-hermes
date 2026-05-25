# Phase 19 Discussion Log

**Date:** 2026-04-14
**Mode:** discuss (not assumptions, not power)
**Session:** resumed from `.continue-here.md` checkpoint; prior session paused after presenting 7 gray areas at ~81% context.

## Gray areas presented

1. Conditional activation mechanism (SKILL-03)
2. Env var / credential setup UX (SKILL-04, SKILL-06, SKILL-11)
3. Skill settings namespace (SKILL-05)
4. Security scanning (SKILL-07)
5. Skills Hub architecture (SKILL-08, SKILL-09)
6. Credential mounting into sandboxes (SKILL-06, SKILL-11)
7. Hermes metadata extraction strategy (foundational, underpins 1–3)

## User-provided design direction (resumed session)

User pasted a complete design-direction doc covering all 7 gray areas with decisive choices (see command args on 2026-04-14). Summary of answers below — full prose preserved in CONTEXT.md decisions D-01..D-19.

| # | Area | User choice |
|---|------|-------------|
| 1 | Conditional activation — timing | Catalog-render time (per-prompt build). |
| 1 | Conditional activation — extraction | Typed `HermesMetadata` struct (locks with Q7). |
| 1 | Conditional activation — mid-session toggling | Reactive, not sticky. |
| 2 | Env/credential UX — surfacing | Option (d): shown in catalog, activate returns setup-error envelope. |
| 2 | Env/credential UX — sandbox flow | Pass-through whitelist appended to Phase 8 allowed-list. |
| 3 | Skill settings — schema | Typed schema in frontmatter `metadata.hermes.config`. |
| 3 | Skill settings — runtime access | Option (a): injected into skill body on activate. |
| 3 | Skill settings — phase boundary | Phase 19 = runtime resolution + injection; Phase 23 = CLI migrate + wizard. |
| 4 | Security — patterns | Reuse `scan_context_content()` + skill-specific patterns (instruction smuggling). |
| 4 | Security — scope | Frontmatter + body. |
| 4 | Security — enforcement | Hard-reject community; WARN-BUT-LOAD builtin/official. |
| 4 | Security — timing | Registry load (installation/discovery). |
| 5 | Hub — adapters | GitHub + skills.sh + well-known endpoints (day one). |
| 5 | Hub — trust | Origin-based labels. |
| 5 | Hub — lifecycle | Clone-and-vendor into `~/.ironhermes/skills/`. |
| 5 | Hub — surface | Primarily CLI; `skills_list()/skill_view()` on tool surface. |
| 6 | Credential mount — location | `~/.ironhermes/credentials/{skill-name}/`. |
| 6 | Credential mount — Docker | Read-only bind mount. |
| 6 | Credential mount — Modal/remote | Synced via provider API before execution. |
| 6 | Credential mount — env var | `HERMES_CREDENTIAL_DIR` points to mount. |
| 7 | Metadata extraction | Typed `HermesMetadata` struct replaces opaque `serde_yaml::Value`. |

## Scope decisions (Claude-asked, user-answered)

- **Q: Keep Skills Hub (SKILL-08/09) in Phase 19, split to 19.1, or defer to v2.1?**
  - **A: Split to Phase 19.1.** Phase 19 covers SKILL-01..07 + SKILL-10 + SKILL-11. Phase 19.1 covers SKILL-08 + SKILL-09.
  - **Rationale:** ~3,700 LOC of Python reference (`skills_hub.py` + `skills_sync.py` + `skill_manager_tool.py`). Split keeps Phase 19 shippable, unblocks Phase 20 on a stable local framework, and gives the Hub its own design pass.
  - **Action required (before Phase 19 plan-phase):** Update `.planning/ROADMAP.md` to insert a Phase 19.1 entry and move the SKILL-08 / SKILL-09 rows in the requirement-mapping table.

- **Q: Draft `HermesMetadata` + `SkillConfig` Rust struct definitions now, or defer to plan-phase?**
  - **A: Defer to plan-phase.** CONTEXT.md captures the decision (D-17, D-19 — typed struct, field surface); `/gsd-plan-phase 19` produces concrete Rust definitions as executable tasks.

## Carrying forward from prior phases

- Phase 07.1 D-09 / 07.2 D-13: WARN-BUT-LOAD parsing rule. Re-affirmed in D-18.
- Phase 07.2 D-09: opaque `serde_yaml::Value` metadata storage. **Superseded** by D-17 in this phase.
- Phase 14: `scan_context_content()` reuse pattern. Extended in D-13 (skill-specific patterns).
- Phase 15: frozen-snapshot at session start; slot 4 (Skills) ordering. Referenced by D-01 (catalog filter runs during the freeze).
- Phase 17 D-15: structured-error-envelope shape. Reused by D-04 and D-12.

## Scope creep handled

None raised in this session — user's design doc stayed inside SKILL-01..SKILL-11. Phase 20 (slash commands, `is_available()`) and Phase 23 (config migrate CLI) concerns were explicitly kept outside Phase 19 by the user's own Question-3 phase-boundary answer.

## Anti-patterns avoided

- Did not re-litigate Phase 07.2's locked decisions (WARN-BUT-LOAD, platform filter = SKILL-10, opaque metadata was the *starting* point).
- Did not silently expand scope — Skills Hub split was raised explicitly, not assumed.
- Did not skip `<canonical_refs>` — mandatory section populated with full relative paths grouped by topic.
- Did not draft struct definitions inline — deferred to plan-phase per user choice.

## Next action

1. User (or Claude, with user approval) updates ROADMAP.md to insert Phase 19.1 and remap SKILL-08/09.
2. Run `/gsd-plan-phase 19` to produce executable plans against the decisions in 19-CONTEXT.md.
3. Phase 19.1 CONTEXT.md authored separately once Phase 19 is executing or complete.
