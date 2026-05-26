# Phase 19: Skills Framework - Context

**Gathered:** 2026-04-14
**Status:** Ready for planning

<domain>
## Phase Boundary

Extend the existing Phase 07 skills baseline with the SKILL.md metadata fields that were deliberately left opaque in Phase 07.2 — wire up conditional activation, env var / credential gating, a typed skill settings namespace, security scanning of skill content, platform + sandbox env pass-through, and typed metadata extraction.

**In scope (Phase 19):** SKILL-01, SKILL-02, SKILL-03, SKILL-04, SKILL-05, SKILL-06, SKILL-07, SKILL-10, SKILL-11.

**Out of scope for Phase 19 (moved to new Phase 19.1):** SKILL-08 (Skills Hub publish/install), SKILL-09 (trust levels). Driven by ~3,700 LOC of Python reference (`skills_hub.py` + `skills_sync.py` + `skill_manager_tool.py`) — splitting keeps Phase 19 shippable and lets Phase 20 start on top of a stable local framework.

**Out of scope for this phase series:** Slash commands (SKILL-12/13/14 → Phase 20), tool registry `is_available()` plumbing (TOOL-01..05 → Phase 20), `hermes config migrate` CLI and interactive setup wizard (→ Phase 23).

</domain>

<decisions>
## Implementation Decisions

### Conditional activation (SKILL-03)
- **D-01:** Filtering runs at **catalog-render time** (per-prompt build), not at registry-load time. Tools can be toggled via hooks or session state; a static registry-load filter would yield stale system prompts.
- **D-02:** Mid-session toggling is **reactive, not sticky** — if a tool dependency is lost, the skill is omitted from the next prompt iteration. No cached pinning.
- **D-03:** Scope of filter inputs includes `requires_toolsets`, `requires_tools`, `fallback_for_toolsets`, `fallback_for_tools`, `platforms` (already shipped in 07.2 = SKILL-10), and env/credential readiness (see D-04..D-06).

### Env var & credential UX (SKILL-04, SKILL-06, SKILL-11)
- **D-04:** When a declared `required_environment_variable` or `required_credential_file` is missing, the skill is **shown in the catalog** but its `activate` action returns a **setup-error envelope** (Phase 17 D-15 style) naming the missing requirement (e.g., `"I need a TENOR_API_KEY to search GIFs"`). The agent relays this to the user. Rationale: the agent needs to "know" the capability exists so it can explain what's blocking.
- **D-05:** Skill-declared env vars flow into the Phase 8 exec sandbox via **pass-through whitelist**. On skill activation, declared `required_environment_variables` are appended to the sandbox's allowed-list so Phase 8's env stripping does not drop them. Credential file paths are exposed via `HERMES_CREDENTIAL_DIR` (see D-10).
- **D-06:** Missing-requirement detection runs at activation time, not catalog render. Rationale: availability can change (user exports a key mid-session), and we don't want catalog rendering to touch the filesystem on every prompt.

### Skill settings namespace (SKILL-05)
- **D-07:** Config schema is **typed, declared in skill frontmatter** under `metadata.hermes.config`. Enables `hermes config migrate` (Phase 23) to validate inputs against the declared schema. Values persist to `config.yaml` under the `skills.config.<skill-name>` namespace (wire-compatible with Phase 07 SkillsConfig).
- **D-08:** Runtime access is **body-injection on activate**: when the skill activates, a synthesized header block (e.g., `[Skill config: wiki.path = ~/research]`) is prepended to the skill instructions loaded into the prompt. The agent reads config through the same text channel as the rest of the skill body. Rationale: no new tool surface, no new lookup API, no extra round-trip.
- **D-09:** Phase boundary is strict. **Phase 19** implements runtime resolution + injection + schema extraction. **Phase 23** implements the CLI `hermes config migrate` command and interactive setup wizard. Phase 19 must produce config reads that Phase 23 can later write to interactively.

### Credential mounting (SKILL-06, SKILL-11)
- **D-10:** Canonical on-disk location is `~/.ironhermes/credentials/{skill-name}/` (per-skill subdirectory under `HERMES_HOME`).
- **D-11:** Sandbox visibility is backend-specific:
  - **Docker:** read-only bind mount of the per-skill directory into the container.
  - **Modal / remote:** **synced via the provider API** before command execution (not bind-mounted — Modal has no host filesystem).
  - **All backends:** `HERMES_CREDENTIAL_DIR` env var in the sandbox points helper scripts to the mount point so skill code can locate credentials without hardcoding paths.
- **D-12:** Credential presence check happens at activation time (same code path as D-06). Missing credential files produce the same setup-error envelope shape as D-04.

### Security scanning (SKILL-07)
- **D-13:** Reuse `scan_context_content()` (Phase 14) as the baseline pattern engine, and **add skill-specific patterns**. Required addition: **"instruction smuggling" detection** — flag skill body content that attempts to redefine `allowed_tools`, override system prompt headers, or inject prompt-role markers that a SKILL should never emit.
- **D-14:** Scan **scope = frontmatter + body**. Metadata can hide prompt injections (e.g., a long `description:` field that smuggles instructions); frontmatter-only scanning is insufficient.
- **D-15:** Enforcement policy follows Phase 07.2's WARN-BUT-LOAD divergence but differentiated by source:
  - **Community-sourced skills: hard-reject** on scan hit. The skill is not loaded into the registry.
  - **Builtin + official skills: WARN-BUT-LOAD** — log the hit, continue loading. Rationale: first-party skills are code-reviewed; a false-positive should not break the agent.
  - Scan results log to the standard security event path.
- **D-16:** Scan timing is **registry-load (installation/discovery)**, not activation time. Rationale: a malicious skill should never be eligible for activation in the first place. Also avoids re-scanning every prompt build.

### Hermes metadata extraction strategy (foundational)
- **D-17:** Replace the Phase 07.2 opaque `serde_yaml::Value` storage with a **typed `HermesMetadata` struct** on `SkillFrontmatter`. Rationale: cross-referencing `requires_toolsets` with the active `ContextEngine` state (Phase 18) and the sandbox whitelisting in D-05 are error-prone against an opaque map; typed access gives compile-time guarantees.
- **D-18:** Parsing rule remains **WARN-BUT-LOAD** (Phase 07.1 D-09 / 07.2 D-13): unknown or extra fields inside `metadata.hermes.*` are logged and preserved in an extras bag, never cause rejection. Unit test must cover: (a) existing Phase 07.2 skills with only `metadata.hermes: <empty or unknown>` still load cleanly, (b) skills with all new fields populated round-trip correctly.
- **D-19:** `HermesMetadata` struct fields mirror the SKILL-01..11 requirement surface: `requires_toolsets`, `requires_tools`, `fallback_for_toolsets`, `fallback_for_tools`, `required_environment_variables` (typed entries with prompt/help/required_for), `required_credential_files`, `config` (declared schema), plus the platforms list already in 07.2. Concrete field types are drafted in plan-phase.

### Claude's Discretion
- Exact instruction-smuggling pattern list (D-13) — planner-driven; align to hermes-agent `skills_guard.py`.
- Concrete Rust type shape for `HermesMetadata` and `SkillConfig` entries (D-17, D-19) — drafted in plan-phase.
- Error-envelope field names for the setup-error response (D-04) — reuse Phase 17 D-15 shape where applicable.
- Modal sync-before-execute mechanics (D-11) — implementation detail.
- Whether `scan_context_content()` gets a `source: SkillSource` parameter or a new `scan_skill_content()` wrapper (D-13) — planner choice; behavior is fixed.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents (researcher, planner, executor) MUST read these before acting on this phase.**

### Phase 19 requirements
- `.planning/ROADMAP.md` §"Phase 19: Skills Framework" — goal, success criteria, requirement mapping (SKILL-01..SKILL-11). Note: ROADMAP.md needs a follow-up edit to split SKILL-08/09 into Phase 19.1.
- `.planning/REQUIREMENTS.md` lines 71-81 — exact text for SKILL-01..SKILL-11.
- `.planning/PROJECT.md` — vendor constraints ("port hermes-agent faithfully"), single-binary rule.

### Prior phase context that locks downstream behavior
- `.planning/phases/14-context-files-soul-md/14-CONTEXT.md` — `scan_context_content()` reuse pattern; D-13/D-14 build on this.
- `.planning/phases/15-10-layer-prompt-assembly/15-CONTEXT.md` — slot 4 (Skills) ordering, frozen-snapshot pattern, durable/ephemeral split.
- `.planning/phases/17-memory-tools-external-providers/17-CONTEXT.md` — D-15 structured-error-envelope shape; reused by D-04 + D-12.
- `.planning/phases/18-context-compression/18-CONTEXT.md` — ContextEngine toggling; interacts with D-01/D-02.
- `.planning/milestones/v1.1-phases/07-skills-system/07-CONTEXT.md` — original skills baseline decisions.
- `.planning/milestones/v1.1-phases/07.1-*/07.1-GAP-REPORT.md` — gap list against hermes-agent reference; rows tagged "v2" map to Phase 19 work.
- `.planning/milestones/v1.1-phases/07.2-*/07.2-CONTEXT.md` — opaque-metadata decision (D-09), WARN-BUT-LOAD rule (D-13), platform-filter = SKILL-10.

### Existing Rust code (baseline to extend)
- `crates/ironhermes-core/src/skills.rs` — SkillRegistry, parse_skill_md, SkillFrontmatter, platform filter, name validation. D-17 modifies SkillFrontmatter.
- `crates/ironhermes-tools/src/skills_tool.rs` — SkillsTool list/view/activate/deactivate. D-04/D-08/D-12 modify activate.
- `crates/ironhermes-core/src/config.rs` (lines 70-80, 347-370, 525-680 tests) — SkillsConfig round-trip. D-07 extends with `skills.config.<name>` namespace.
- `crates/ironhermes-agent/src/prompt_builder.rs` — consumer of `SkillRegistry::catalog_text()` (slot 4). D-01 changes this call site (per-render filter).

### Hermes-agent reference implementations (port targets)
- `~/code/hermes-agent/tools/skills_tool.py` — canonical skills tool behavior.
- `~/code/hermes-agent/tools/skills_guard.py` — instruction-smuggling pattern source for D-13.
- `~/code/hermes-agent/tools/skill_manager_tool.py` — activation error shapes, setup-error envelope reference.
- `https://hermes-agent.nousresearch.com/docs/developer-guide/creating-skills` — SKILL.md format spec; authoritative for D-01..D-19 field semantics.

### Deferred to Phase 19.1 (Skills Hub) — listed here so planner does NOT pull them in
- `~/code/hermes-agent/tools/skills_hub.py`
- `~/code/hermes-agent/tools/skills_sync.py`

</canonical_refs>

<specifics>
## Specific Ideas

- **Setup-error envelope wording** — user wants the agent to be able to say things like *"I need a `TENOR_API_KEY` to search GIFs"* naturally. The envelope must carry a human-readable message the agent can verbatim-relay, not just a typed error code.
- **Agent self-discovery** — `skills_list()` and `skill_view()` remain on the tool surface (already shipped in Phase 07) so the agent can introspect capabilities. No Phase 19 regression here.
- **Primary surface = CLI** for hub/management interactions (Phase 19.1 scope), but **tool surface is secondary and must work** — the agent needs to be able to see installed skills without going through the CLI.
- **Typed metadata is load-bearing** — D-17 underpins D-01/D-03 (filter logic), D-05 (sandbox whitelist), D-07 (config schema). Getting the struct shape right in plan-phase is the single highest-leverage decision.
- **Phase 8 interaction** is explicit — env stripping must NOT drop skill-declared vars. Planner should add a regression test that activates a skill with a dummy env var and asserts it reaches the sandboxed child.

</specifics>

<code_context>
## Existing Code Insights

### Reusable Assets
- `SkillRegistry` (ironhermes-core/src/skills.rs) — scanning across 3 default paths + extras, name validation, platform filter all shipped. Phase 19 extends `SkillFrontmatter.metadata` from `serde_yaml::Value` → typed `HermesMetadata` and adds the filter hooks.
- `scan_context_content()` (ironhermes-core security module, Phase 14) — pattern engine ready for reuse. Phase 19 adds skill-specific patterns + instruction-smuggling detection.
- `SkillsConfig` (ironhermes-core/src/config.rs) — `enabled` + `extra_paths` exist. Phase 19 adds `config` map under `skills.config.<name>`.
- `SkillsTool` (ironhermes-tools) — activate/deactivate/list/view actions. Phase 19 replaces activate's success-or-fail path with a setup-error-envelope branch.
- Phase 17 `D-15 envelope pattern` — structured error shape already in use for memory tool; generalizes directly to skill activation errors.

### Established Patterns
- **Frozen-snapshot prompt building** — skill catalog frozen at session start (Phase 15). Catalog-render-time filtering (D-01) runs during that freeze, not continuously.
- **WARN-BUT-LOAD parsing** — applies to D-18 metadata extraction; do NOT hard-reject on unknown fields.
- **Opaque → typed migration precedent** — Phase 12 moved from trait objects to enum dispatch; similar discipline applies to D-17 (serde_yaml::Value → HermesMetadata).
- **Config round-trip tests** — Phase 13/17 style with golden YAML fixtures; D-07 must follow the same pattern.

### Integration Points
- `prompt_builder.rs` slot 4 (Skills) — where per-render catalog filter (D-01) plugs in.
- `ironhermes-exec` sandbox env construction — where D-05 pass-through whitelist appends entries before Phase 8 stripping.
- `SkillsTool::activate` — where D-04/D-12 setup-error envelope replaces the current success-only path.
- `parse_skill_md` — where D-17 typed extraction replaces opaque `serde_yaml::Value` capture.
- Config serde layer (`config.rs`) — where D-07 `skills.config.<name>` round-trips.

</code_context>

<deferred>
## Deferred Ideas

### Split to Phase 19.1 (Skills Hub — needs its own CONTEXT.md + plan)
- SKILL-08 publish/install across GitHub, skills.sh, well-known endpoints (source adapters).
- SKILL-09 trust levels (builtin / official / trusted / community) as origin-based labels.
- Clone-and-vendor lifecycle into `~/.ironhermes/skills/` for offline stability.
- CLI management surface (primary) + `skills_list()/skill_view()` tool surface visibility of installed skills.
- Scan-on-install enforcement matrix (hard-reject community, WARN-BUT-LOAD builtin/official — D-15 rule applies the same way; Phase 19.1 just wires the install-time trigger).
- **Action required:** update ROADMAP.md to insert Phase 19.1 entry and move SKILL-08/09 mapping table rows before Phase 19 planning begins.

### Captured for other phases
- Slash commands (`/skills`, `/help`, `/memory`, etc.) — Phase 20 (SKILL-12/13/14).
- `is_available()` tool trait + toolset management — Phase 20 (TOOL-01..05).
- `hermes config migrate` CLI + interactive setup wizard — Phase 23 (per D-09).
- SOUL.md / personality integration with skills ("personality as a skill") — not currently requested; capture to backlog if raised later.

</deferred>

---

*Phase: 19-skills-framework*
*Context gathered: 2026-04-14*
*Gray areas discussed: all 7 (Conditional activation, Env/credential UX, Skill settings, Security, Hub architecture — split to 19.1, Credential mounting, Metadata extraction strategy)*
