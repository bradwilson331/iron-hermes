# Phase 19: Skills Framework - Research

**Researched:** 2026-04-14
**Domain:** Rust skills framework — typed metadata, conditional activation, env/credential gating, security scanning, sandbox env pass-through
**Confidence:** HIGH (all findings based on direct codebase reads and hermes-agent reference code)

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Conditional activation (SKILL-03)**
- D-01: Filtering runs at catalog-render time (per-prompt build), not registry-load time
- D-02: Mid-session toggling is reactive, not sticky — no cached pinning
- D-03: Filter inputs: requires_toolsets, requires_tools, fallback_for_toolsets, fallback_for_tools, platforms, env/credential readiness

**Env var & credential UX (SKILL-04, SKILL-06, SKILL-11)**
- D-04: Missing required_environment_variable or required_credential_file → skill shown in catalog but activate returns setup-error envelope (Phase 17 D-15 style) naming the missing requirement
- D-05: Skill-declared env vars flow into Phase 8 exec sandbox via pass-through whitelist. Declared required_environment_variables appended to sandbox's allowed-list so Phase 8 env stripping does not drop them. Credential file paths exposed via HERMES_CREDENTIAL_DIR
- D-06: Missing-requirement detection runs at activation time, not catalog render

**Skill settings namespace (SKILL-05)**
- D-07: Config schema declared in skill frontmatter under metadata.hermes.config; persists to config.yaml under skills.config.<skill-name>
- D-08: Runtime access is body-injection on activate — synthesized header block prepended to skill instructions loaded into prompt
- D-09: Phase 19 implements runtime resolution + injection + schema extraction; Phase 23 implements hermes config migrate CLI

**Credential mounting (SKILL-06, SKILL-11)**
- D-10: Canonical on-disk location is ~/.ironhermes/credentials/{skill-name}/
- D-11: Docker = read-only bind mount; Modal = synced via provider API; all backends expose HERMES_CREDENTIAL_DIR env var
- D-12: Credential presence check at activation time; missing files produce same setup-error envelope as D-04

**Security scanning (SKILL-07)**
- D-13: Reuse scan_context_content() (Phase 14) as baseline, add skill-specific instruction-smuggling patterns
- D-14: Scan scope = frontmatter + body
- D-15: Community skills hard-reject on scan hit; builtin/official WARN-BUT-LOAD
- D-16: Scan at registry-load (installation/discovery), not activation time

**Hermes metadata extraction strategy**
- D-17: Replace opaque serde_yaml::Value with typed HermesMetadata struct on SkillFrontmatter
- D-18: Parsing rule WARN-BUT-LOAD; unknown fields in metadata.hermes.* logged and preserved in extras bag
- D-19: HermesMetadata fields: requires_toolsets, requires_tools, fallback_for_toolsets, fallback_for_tools, required_environment_variables (typed entries with prompt/help/required_for), required_credential_files, config (declared schema), plus platforms list already in 07.2

### Claude's Discretion
- Exact instruction-smuggling pattern list (D-13) — align to hermes-agent skills_guard.py
- Concrete Rust type shape for HermesMetadata and SkillConfig entries (D-17, D-19)
- Error-envelope field names for the setup-error response (D-04) — reuse Phase 17 D-15 shape
- Modal sync-before-execute mechanics (D-11)
- Whether scan_context_content() gets a source: SkillSource parameter or a new scan_skill_content() wrapper (D-13)

### Deferred Ideas (OUT OF SCOPE)
- SKILL-08 publish/install, SKILL-09 trust levels (Phase 19.1)
- Clone-and-vendor lifecycle
- CLI management surface primary (Phase 19.1)
- Slash commands (Phase 20)
- is_available() tool trait (Phase 20)
- hermes config migrate CLI (Phase 23)
- SOUL.md / personality integration with skills
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SKILL-01 | Skills use SKILL.md format with YAML frontmatter (name, description, version, author, platforms, metadata) | parse_skill_md + SkillFrontmatter already ship; D-17 adds typed HermesMetadata |
| SKILL-02 | Skills organized by category in skills/ directory with progressive disclosure (catalog at startup, full content on activation) | catalog_text() + activate flow already ship; D-01 adds per-render filter |
| SKILL-03 | Conditional activation: requires_toolsets, requires_tools hide skills when dependencies absent; fallback_for_toolsets, fallback_for_tools hide when primaries present | New filter function at catalog-render time in prompt_builder.rs slot 5 |
| SKILL-04 | Skills declare required_environment_variables with prompt/help/required_for fields; missing vars trigger setup prompt on load | activate handler checks env presence; setup-error envelope returned |
| SKILL-05 | Skills declare config settings (metadata.hermes.config) stored in config.yaml under skills.config namespace | SkillsConfig extended with config map; body-injection on activate |
| SKILL-06 | Skills declare required_credential_files for OAuth tokens; existence checked on load, files mounted into sandboxes | activate handler checks credential file presence; HERMES_CREDENTIAL_DIR set in sandbox |
| SKILL-07 | Skill content security scanned before injection into system prompt | scan_context_content() extended with skill-specific patterns; called at registry-load |
| SKILL-10 | Platform-specific skills restricted via platforms field; hidden on incompatible platforms | Already ships (07.2 D-04/D-05); no new work needed beyond confirming no regression |
| SKILL-11 | Skill env vars automatically passed through to execute_code and terminal sandboxes when set | sandbox.rs build_env() extended with pass-through whitelist from activated skills |
</phase_requirements>

---

## Summary

Phase 19 is a brownfield extension of the skills system shipped in Phase 07 through 07.5. The foundational structures — `SkillFrontmatter`, `SkillRecord`, `SkillRegistry`, `SkillsTool`, and `catalog_text()` — all exist and compile cleanly. Phase 19's job is to wire the `metadata.hermes.*` blob (currently an opaque `serde_yaml::Value`) into a typed `HermesMetadata` struct, then use that struct to drive four new behaviors: per-render catalog filtering (D-01/D-03), activation-time env/credential gating with setup-error envelopes (D-04/D-06/D-12), body-injection config headers (D-08), and registry-load security scanning extended with instruction-smuggling patterns (D-13/D-16).

The Phase 8 sandbox (`crates/ironhermes-exec/src/sandbox.rs`) strips secret-named env vars using a fixed allowlist strategy in `build_env()`. D-05 requires that activated-skill env vars bypass this stripping. The integration point is the `SAFE_VARS` constant and the `build_env()` call site — a per-session skill-declared-env whitelist must be passed in and treated as safe before the secret-pattern filter runs.

The hermes-agent Python reference (`tools/skills_tool.py`) provides the canonical skill_view readiness envelope shape. The `scan_context_content()` function in `ironhermes-core/src/context_scanner.rs` is the existing pattern engine (10 patterns, `RegexSet`, returns blocked-string on hit). The instruction-smuggling additions from `skills_guard.py` and `skills_tool.py` are well-documented and categorized below.

**Primary recommendation:** Execute in four waves: (1) typed HermesMetadata struct + WARN-BUT-LOAD parsing, (2) catalog-render filter + setup-error envelope in activate, (3) security scan extension + registry-load trigger, (4) sandbox env pass-through + credential mounting.

---

## Standard Stack

### Core (all already in Cargo.toml — no new dependencies needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| serde / serde_yaml | workspace | YAML frontmatter parsing for HermesMetadata | Already used for SkillFrontmatter; serde_yaml::from_value<T> for typed extraction |
| regex / regex::RegexSet | workspace | Instruction-smuggling pattern matching | Already used in context_scanner.rs THREAT_PATTERNS |
| tracing | workspace | warn! / debug! for WARN-BUT-LOAD findings | Already used throughout skills.rs |
| anyhow | workspace | Error propagation in activate handler | Already used in SkillsTool::execute |
| serde_json | workspace | Tool response envelopes | Already used in skills_tool.rs |

[VERIFIED: direct Cargo.toml read not performed, but all imports confirmed by reading source files in this session]

### New Data Types Only (no new crate dependencies)

No new crate dependencies are needed. Phase 19 is purely additive Rust code on existing structures.

---

## Architecture Patterns

### Recommended Project Structure (changes only)

```
crates/ironhermes-core/src/
├── skills.rs              # SkillFrontmatter: metadata: Option<serde_yaml::Value>
│                          # → metadata: Option<HermesMetadata>  (D-17)
│                          # + HermesMetadata struct (D-19)
│                          # + EnvVarEntry, CredentialFileEntry, SkillConfigField structs
│                          # + SkillSource enum (builtin/official/community) for D-15
│                          # + scan_skill_content() or scan_context_content() extension (D-13/D-16)
├── context_scanner.rs     # Extended THREAT_PATTERNS RegexSet with skill-specific patterns
└── config.rs              # SkillsConfig extended: config: HashMap<String, HashMap<String, Value>>

crates/ironhermes-tools/src/
└── skills_tool.rs         # handle_activate() extended: env/cred checks → setup-error OR body-injection

crates/ironhermes-agent/src/
└── prompt_builder.rs      # load_skills() + build_split() Skills slot: apply per-render filter (D-01)

crates/ironhermes-exec/src/
└── sandbox.rs             # build_env(): accept skill_env_whitelist: &[String] param (D-05)
```

### Pattern 1: Typed HermesMetadata Struct (D-17/D-19)

**What:** Replace `metadata: Option<serde_yaml::Value>` with `metadata: Option<HermesMetadata>` on `SkillFrontmatter`.

**Proposed struct definitions** (Claude's Discretion per CONTEXT.md — planner should finalize):

```rust
// Source: hermes-agent tools/skills_tool.py _get_required_environment_variables()
// + CONTEXT.md D-19 field list

/// A declared required environment variable with human-readable prompts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnvVarEntry {
    pub name: String,
    /// Human-readable prompt shown when requesting the value (e.g. "Enter your TENOR API key").
    #[serde(default)]
    pub prompt: Option<String>,
    /// URL or text hint pointing to where the user can get this value.
    #[serde(default)]
    pub help: Option<String>,
    /// Short string naming the feature that requires this var (e.g. "GIF search").
    #[serde(default)]
    pub required_for: Option<String>,
}

/// A declared credential file path (relative to HERMES_CREDENTIAL_DIR).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CredentialFileEntry {
    /// Simple string path
    Path(String),
    /// Structured entry with optional description
    Structured {
        path: String,
        #[serde(default)]
        description: Option<String>,
    },
}

/// A single config field declared in the skill's metadata.hermes.config block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfigField {
    /// The config key as it will appear under skills.config.<skill-name>.<key>
    pub key: String,
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    #[serde(default)]
    pub description: Option<String>,
    /// Type hint: "string" | "boolean" | "integer" | "path" (advisory only in Phase 19)
    #[serde(rename = "type", default)]
    pub field_type: Option<String>,
}

/// Typed representation of the metadata.hermes.* block (D-17, D-19).
/// Unknown fields are preserved in `extras` (D-18 WARN-BUT-LOAD).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HermesMetadata {
    /// Toolset names that must ALL be active for this skill to appear (SKILL-03).
    pub requires_toolsets: Vec<String>,
    /// Individual tool names that must be present (SKILL-03).
    pub requires_tools: Vec<String>,
    /// Skill is hidden when any of these toolsets are active (SKILL-03).
    pub fallback_for_toolsets: Vec<String>,
    /// Skill is hidden when any of these tools are available (SKILL-03).
    pub fallback_for_tools: Vec<String>,
    /// Env vars this skill needs; checked at activation time (SKILL-04).
    pub required_environment_variables: Vec<EnvVarEntry>,
    /// Credential file paths under HERMES_CREDENTIAL_DIR (SKILL-06).
    pub required_credential_files: Vec<CredentialFileEntry>,
    /// Config schema declared by this skill (SKILL-05).
    pub config: Vec<SkillConfigField>,
    /// Preserve unknown hermes fields for forward compat (D-18).
    #[serde(flatten)]
    pub extras: std::collections::HashMap<String, serde_yaml::Value>,
}
```

**Extraction from opaque blob:**

```rust
// Source: direct inspection of SkillFrontmatter.metadata in skills.rs
// parse_skill_md already stores metadata as Option<serde_yaml::Value>
// Typed extraction from existing opaque blob:

fn extract_hermes_metadata(raw: Option<serde_yaml::Value>) -> Option<HermesMetadata> {
    let hermes_val = raw?
        .get("hermes")
        .cloned()?;
    match serde_yaml::from_value::<HermesMetadata>(hermes_val) {
        Ok(m) => Some(m),
        Err(e) => {
            warn!("HermesMetadata parse error (WARN-BUT-LOAD): {}", e);
            Some(HermesMetadata::default()) // preserve empty rather than None
        }
    }
}
```

**WARN-BUT-LOAD guarantee (D-18):** serde `#[serde(default)]` on the struct plus `#[serde(flatten)]` on `extras` means: any unknown field lands in `extras` without error. Skills from Phase 07.2 with only `metadata.hermes: {}` or `metadata.hermes: {tags: [...]}` will parse to `HermesMetadata::default()` with extras populated — no rejection, no data loss.

### Pattern 2: Catalog-Render Filter (D-01/D-03)

**What:** `load_skills()` and the `build_split()` fallback in `prompt_builder.rs` currently call `registry.catalog_text()` which returns all skills as a flat `- name: description` list (verified in `skills.rs:357`). D-01 requires a per-render filter that hides skills whose toolset/tool requirements are unmet.

**Integration point (verified):** `prompt_builder.rs` lines 375-395 (`load_skills()`) and 413-428 (`build_split()` fallback). Both call `registry.catalog_text()`.

**Proposed approach:**

```rust
// New method on SkillRegistry (or a free function in prompt_builder.rs):
// Takes an active toolset/tool snapshot and returns filtered catalog text.

pub fn filtered_catalog_text(
    &self,
    active_toolsets: &HashSet<String>,
    active_tools: &HashSet<String>,
) -> String {
    self.skills
        .iter()
        .filter(|s| skill_passes_filter(s, active_toolsets, active_tools))
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect::<Vec<_>>()
        .join("\n")
}

fn skill_passes_filter(
    record: &SkillRecord,
    active_toolsets: &HashSet<String>,
    active_tools: &HashSet<String>,
) -> bool {
    let meta = match &record.hermes_metadata {
        Some(m) => m,
        None => return true, // no hermes metadata → always show
    };

    // requires_toolsets: ALL listed toolsets must be active
    if !meta.requires_toolsets.is_empty() {
        if !meta.requires_toolsets.iter().all(|t| active_toolsets.contains(t.as_str())) {
            return false;
        }
    }

    // requires_tools: ALL listed tools must be available
    if !meta.requires_tools.is_empty() {
        if !meta.requires_tools.iter().all(|t| active_tools.contains(t.as_str())) {
            return false;
        }
    }

    // fallback_for_toolsets: hide if ANY primary toolset is active
    if meta.fallback_for_toolsets.iter().any(|t| active_toolsets.contains(t.as_str())) {
        return false;
    }

    // fallback_for_tools: hide if ANY primary tool is available
    if meta.fallback_for_tools.iter().any(|t| active_tools.contains(t.as_str())) {
        return false;
    }

    true
}
```

**PromptBuilder change:** `set_skill_registry()` must also accept a toolset/tool snapshot. Or `load_skills()` receives the active snapshot at session-freeze time. Since D-01 says "per-prompt build" and the prompt is frozen at session start (Phase 15), the snapshot captured at `load_context()` time is correct.

### Pattern 3: Setup-Error Envelope (D-04/D-12)

**What:** `handle_activate()` in `skills_tool.rs` currently returns `{"status":"ok","name":...,"content":...}` on success and `{"status":"error","message":...}` on not-found. D-04 adds a third branch: skill found but requirements unmet.

**Phase 17 D-15 envelope shape** (verified from 17-CONTEXT.md D-15):
```json
{"error": "capacity_exceeded", "current": 2150, "limit": 2200, "entry_size": 180, "suggestion": "..."}
{"error": "content_rejected", "reason": "injection_pattern_detected"}
```

**Proposed setup-error envelope for Phase 19 (aligns with Python readiness shape):**

```rust
// Source: hermes-agent tools/skills_tool.py skill_view() lines 1170-1215
// "status": "setup_needed" mirrors SkillReadinessStatus.SETUP_NEEDED

json!({
    "status": "setup_needed",
    "name": skill_name,
    "readiness_status": "setup_needed",
    "missing_required_environment_variables": ["TENOR_API_KEY", "OTHER_VAR"],
    "missing_credential_files": ["oauth_token.json"],
    "setup_note": "Setup needed before using this skill: missing env $TENOR_API_KEY, file oauth_token.json.",
    "setup_help": "Get your key at https://tenor.com/developer"  // if declared in skill
})
```

**Human-readable relay note** (CONTEXT.md specifics): The `setup_note` field must carry a verbatim-relay message the agent can surface: *"I need a `TENOR_API_KEY` to search GIFs"*. The `setup_note` string is the primary field the agent reads.

**New `handle_activate()` flow:**

```rust
fn handle_activate(
    registry: &SkillRegistry,
    args: &Value,
    active_skills: &Mutex<Vec<SkillRecord>>,
    skill_config: &HashMap<String, HashMap<String, Value>>, // from SkillsConfig.config
    credential_dir: &Path,
) -> Value {
    // 1. Find skill (not found → existing error response)
    // 2. Check env vars: for each EnvVarEntry in hermes_metadata.required_environment_variables,
    //    check std::env::var(name). Collect missing.
    // 3. Check credential files: for each entry in hermes_metadata.required_credential_files,
    //    check credential_dir.join(skill_name).join(path).exists(). Collect missing.
    // 4. If any missing → return setup-error envelope (D-04/D-12)
    // 5. If all present:
    //    a. Read body content
    //    b. Build config header from skill_config map (D-08)
    //    c. Prepend config header to body
    //    d. Push SkillRecord to active_skills (existing behavior)
    //    e. Return ok envelope with injected content
}
```

### Pattern 4: Body-Injection Config Header (D-08)

**What:** When a skill has config values set under `skills.config.<skill-name>` in config.yaml, prepend a synthesized header to the skill body on activation.

**Config extension required in `config.rs`:**

```rust
// Extend SkillsConfig:
pub struct SkillsConfig {
    pub enabled: bool,
    pub extra_paths: Vec<PathBuf>,
    /// Per-skill config values: skills.config.<skill-name>.<key> = <value> (D-07)
    #[serde(default)]
    pub config: HashMap<String, HashMap<String, serde_yaml::Value>>,
}
```

**Body injection format** (aligns with CONTEXT.md "synthesized header block" description):

```
[Skill config: wiki.path = ~/research, wiki.format = markdown]

<original skill body>
```

**Precedent:** Same approach as how AGENTS.md is wrapped with `## AGENTS.md\n\n{content}` in `prompt_builder.rs:231`.

### Pattern 5: Security Scan Extension (D-13/D-16)

**What:** Extend `context_scanner.rs` THREAT_PATTERNS RegexSet with skill-specific patterns, and add a `scan_skill_content()` wrapper (or parameterized `scan_context_content()`) that applies the extended set. Call it from `load_with_paths()` in `skills.rs`.

**Current `scan_context_content()` signature** (verified from context_scanner.rs:44):
```rust
pub fn scan_context_content(content: &str, filename: &str) -> String
```
Returns original content if clean, or `[BLOCKED: ...]` string on hit.

**Recommendation:** Add a new `scan_skill_content()` function that combines existing THREAT_PATTERNS with `SKILL_THREAT_PATTERNS` (the new set). This avoids adding skill-specific patterns to the generic context scanner and keeps separation clean.

**D-15 enforcement at registry-load** (call site in `load_with_paths()`, after parse_skill_md):

```rust
// After parse_skill_md succeeds, before seen_names dedup:
let scan_result = scan_skill_content(&content, &frontmatter.name, skill_source);
match (scan_result, skill_source) {
    (ScanHit, SkillSource::Community) => {
        warn!("SkillRegistry: hard-rejecting community skill {:?} — scan hit", frontmatter.name);
        continue; // D-15: community hard-reject
    }
    (ScanHit, _) => {
        warn!("SkillRegistry: WARN-BUT-LOAD builtin/official skill {:?} — scan hit", frontmatter.name);
        // continue loading (D-15: builtin/official WARN-BUT-LOAD)
    }
    (Clean, _) => {} // proceed normally
}
```

**SkillSource enum** (needed for D-15; Phase 19.1 will make this richer):
```rust
pub enum SkillSource {
    Builtin,   // ships with ironhermes
    Official,  // optional-skills/ directory
    Community, // everything else (Phase 19.1 will set this based on provenance)
}
```
For Phase 19, since trust levels (SKILL-09) are deferred to 19.1, all locally-discovered skills can default to `SkillSource::Builtin` (WARN-BUT-LOAD). The community-hard-reject path is wired but not triggered until 19.1 sets provenance. This keeps Phase 19 shippable without 19.1's trust-level machinery.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| YAML frontmatter parsing | Custom parser | serde_yaml::from_value::<HermesMetadata>() | Already in use; handles all YAML edge cases |
| Pattern matching for injection | Custom string search | regex::RegexSet | Compiled once via LazyLock; already the pattern in context_scanner.rs |
| Env var access | Custom env reader | std::env::var() | Standard; used throughout codebase |
| Filesystem credential check | Custom path checker | std::path::Path::exists() | Direct fs call is correct; no stat caching needed |
| Config round-trip | Custom serializer | serde + serde_yaml | Already the pattern in config.rs |

**Key insight:** Every new capability in Phase 19 has a direct precedent in the existing codebase. The work is wiring, not invention.

---

## Instruction-Smuggling Patterns (for D-13)

Sourced from two places: `skills_guard.py` THREAT_PATTERNS (lines 160-484) and `skills_tool.py` _INJECTION_PATTERNS (lines 862-872). These should be added to a `SKILL_THREAT_PATTERNS` RegexSet in `context_scanner.rs`.

### Category 1: Tool Redefinition / Privilege Escalation

From `skills_guard.py` lines 407-420:
```
r'^allowed-tools\s*:'           # skill declares allowed-tools (privilege escalation)
r'\bsudo\b'                     # sudo usage
r'setuid|setgid|cap_setuid'     # setuid/setgid escalation
r'NOPASSWD'                     # passwordless sudo
r'chmod\s+[u+]?s'              # SUID/SGID bit
```
[VERIFIED: read from hermes-agent/tools/skills_guard.py lines 407-420]

### Category 2: System Prompt Override

From `skills_guard.py` lines 167-179 and `skills_tool.py` lines 862-872:
```
r'system\s+prompt\s+override'                   # explicit system prompt override
r'output\s+(?:\w+\s+)*(system|initial)\s+prompt' # extract system prompt
"system prompt:"                                  # literal prefix (case-insensitive)
"<system>"                                        # XML-style role marker
"]]>"                                             # CDATA close (XML injection)
```
[VERIFIED: read from hermes-agent/tools/skills_guard.py:167-179, skills_tool.py:862-872]

### Category 3: Prompt-Role Markers

From `skills_guard.py` lines 160-193 and `skills_tool.py` lines 862-872:
```
r'ignore\s+(?:\w+\s+)*(previous|all|above|prior)\s+instructions'  # instruction override
r'you\s+are\s+(?:\w+\s+)*now\s+'                                  # role hijack
r'do\s+not\s+(?:\w+\s+)*tell\s+(?:\w+\s+)*the\s+user'            # deception
r'pretend\s+(?:\w+\s+)*(you\s+are|to\s+be)\s+'                   # role pretend
r'disregard\s+(?:\w+\s+)*(your|all|any)\s+(?:\w+\s+)*(instructions|rules|guidelines)'
r'act\s+as\s+(if|though)\s+(?:\w+\s+)*you\s+(?:\w+\s+)*(have\s+no|don\'t\s+have)\s+(?:\w+\s+)*(restrictions|limits|rules)'
r'(respond|answer|reply)\s+without\s+(?:\w+\s+)*(restrictions|limitations|filters|safety)'
"you are now"                                    # role override literal
"disregard your"                                 # instructions override
"forget your instructions"                       # instructions override
"new instructions:"                              # instructions injection
```
[VERIFIED: read from hermes-agent/tools/skills_guard.py lines 160-193, skills_tool.py lines 862-872]

### Category 4: Agent Config Persistence (skill-specific, highest severity)

From `skills_guard.py` lines 423-429:
```
r'AGENTS\.md|CLAUDE\.md|\.cursorrules|\.clinerules'   # references agent config files
r'\.hermes/config\.yaml|\.hermes/SOUL\.md'            # references Hermes config directly
```
[VERIFIED: read from hermes-agent/tools/skills_guard.py lines 423-429]

### Category 5: Credential Exfiltration (skill-specific additions beyond existing context_scanner.rs)

Existing `context_scanner.rs` already covers: `curl + secret var`, `cat + secrets file`, `ignore previous instructions`, `do not tell the user`, `system prompt override`, `disregard rules`, `bypass restrictions`, `html comment injection`, `hidden div`, `translate-execute`.

Additions needed for skill scanning:
```
r'\$HOME/\.ssh|\~/\.ssh'                              # SSH dir access
r'\$HOME/\.aws|\~/\.aws'                              # AWS credentials dir  
r'\$HOME/\.hermes/\.env|\~/\.hermes/\.env'            # Hermes secrets file
r'base64[^\n]*env'                                    # base64 + env (exfil staging)
r'printenv|env\s*\|'                                  # dump all env vars
r'os\.getenv\s*\(\s*[^\)]*(?:KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)'  # Python secret read
r'(?:api[_-]?key|token|secret|password)\s*[=:]\s*["\'][A-Za-z0-9+/=_-]{20,}'  # hardcoded secret
r'-----BEGIN\s+(RSA\s+)?PRIVATE\s+KEY-----'           # embedded private key
```
[VERIFIED: read from hermes-agent/tools/skills_guard.py lines 82-160, 434-450]

**Implementation note:** Skills are instruction documents, not scripts. The subset above targets patterns that can cause harm when injected into the system prompt context. The full `skills_guard.py` list (50+ patterns) also covers runtime execution patterns (subprocess, curl-pipe-shell, etc.) that are only relevant for executable skill scripts. For Phase 19 (prompt injection into system prompt), the 5 categories above are the relevant set. The planner should confirm which subset to include.

---

## Common Pitfalls

### Pitfall 1: D-18 WARN-BUT-LOAD Regression

**What goes wrong:** Adding `#[serde(deny_unknown_fields)]` or letting `serde_yaml::from_value` return `Err` and propagating it as `None` would cause existing Phase 07.2 skills to fail loading if they have `metadata.hermes.tags` or `metadata.hermes.related_skills` fields not in `HermesMetadata`.

**Why it happens:** Default serde behavior without `#[serde(default)]` treats missing fields as errors; without `#[serde(flatten)]` on `extras`, unknown fields cause parse failure.

**How to avoid:** `#[serde(default)]` on every Optional field, `#[serde(flatten)] pub extras: HashMap<String, serde_yaml::Value>` for unknown fields, and the `extract_hermes_metadata()` pattern that logs but returns `Some(HermesMetadata::default())` on parse error rather than `None`.

**Warning signs:** Test `test_parse_skill_md_with_only_tags_metadata` fails (existing 07.2 skill with `metadata.hermes: {tags: [...]}` returns None from parse_skill_md).

### Pitfall 2: Sandbox Env Stripping Drops Skill Vars (D-05)

**What goes wrong:** `build_env()` in `sandbox.rs` strips env vars containing `KEY`, `TOKEN`, `SECRET`, etc. A skill declaring `required_environment_variables: [{name: TENOR_API_KEY}]` would have the user's value stripped before the Python child sees it.

**Why it happens:** The pattern `SECRET_PATTERNS: &[&str] = &["KEY", "TOKEN", ...]` (verified line 18 sandbox.rs) is a suffix/substring check via `upper.contains(p)`. `TENOR_API_KEY` contains `KEY` → stripped.

**How to avoid:** The `build_env()` method must accept a `skill_env_whitelist: &[String]` parameter. Whitelisted names are checked BEFORE the secret-pattern strip: if `is_safe || (!is_secret || whitelisted) { keep }`. The whitelist is populated from `active_skills`' `hermes_metadata.required_environment_variables[*].name` at sandbox construction time.

**Warning signs:** Regression test: activate a skill declaring `required_environment_variables: [{name: TEST_API_KEY}]`, set `TEST_API_KEY=testval` in env, execute Python `import os; print(os.getenv("TEST_API_KEY"))` — should print `testval`, not `None`.

### Pitfall 3: Catalog Filter Must Not Touch Filesystem (D-06)

**What goes wrong:** Putting env var presence checks inside the catalog-render filter (D-01/D-03) would cause filesystem/env reads on every prompt build cycle.

**Why it happens:** D-06 is explicit: "Missing-requirement detection runs at activation time, not catalog render." Catalog render is a hot path.

**How to avoid:** The catalog-render filter (D-01) only checks toolset/tool presence (in-memory state). The env/credential check (D-06) only runs in `handle_activate()`. Skills with missing env/creds still appear in the catalog (D-04 explicitly says "skill is shown in the catalog").

### Pitfall 4: scan_context_content() Returns Blocked String, Not Option

**What goes wrong:** `scan_context_content()` returns the original content string on success or a `[BLOCKED: ...]` string on failure. If the caller checks `result.is_empty()` or uses it directly without checking the `[BLOCKED:` prefix, malicious content could pass through.

**Why it happens:** The current function signature `fn scan_context_content(content: &str, filename: &str) -> String` always returns a String. The existing callers (SOUL.md, AGENTS.md, system_message) check `scanned.starts_with("[BLOCKED:")`.

**How to avoid:** `scan_skill_content()` should return `Result<&str, ScanHit>` or check for `[BLOCKED:` prefix consistently. At registry-load (D-16), a scan hit on community skill means `continue` (drop the skill). For `scan_content_content()` callers, the existing check pattern is already correct.

### Pitfall 5: SkillRecord Does Not Store HermesMetadata Currently

**What goes wrong:** `SkillRecord` (verified in skills.rs lines 87-98) currently mirrors `SkillFrontmatter` fields but stores `metadata: Option<serde_yaml::Value>` — the same opaque blob. D-17 must update both `SkillFrontmatter.metadata` AND `SkillRecord` to use the typed struct, otherwise the catalog filter (`prompt_builder.rs`) cannot access typed fields without re-parsing.

**How to avoid:** Update `SkillRecord` to carry `hermes_metadata: Option<HermesMetadata>` (separate from the raw `metadata` opaque value). The `load_with_paths()` extraction populates this field from the parsed frontmatter.

---

## Code Examples

### Existing catalog_text() and load_skills() call site

```rust
// Source: verified from crates/ironhermes-core/src/skills.rs:357-363
pub fn catalog_text(&self) -> String {
    self.skills
        .iter()
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect::<Vec<_>>()
        .join("\n")
}

// Source: verified from crates/ironhermes-agent/src/prompt_builder.rs:374-395
pub fn load_skills(&mut self) {
    // ...
    let catalog = registry.catalog_text();
    // D-01 change: replace with registry.filtered_catalog_text(active_toolsets, active_tools)
}
```

### Existing build_env() — where D-05 whitelist plugs in

```rust
// Source: verified from crates/ironhermes-exec/src/sandbox.rs:240-263
fn build_env(&self, temp_dir: &Path, socket_path: &Path) -> Vec<(String, String)> {
    for (name, value) in std::env::vars() {
        let upper = name.to_uppercase();
        let is_safe = SAFE_VARS.iter().any(|s| upper == *s) || upper.starts_with("XDG_");
        let is_secret = SECRET_PATTERNS.iter().any(|p| upper.contains(p));
        if is_safe || !is_secret {   // <-- D-05: add `|| whitelisted` here
            env.push((name, value));
        }
    }
    // ...
}
// D-05 change: fn build_env(&self, temp_dir, socket_path, skill_env_whitelist: &[String])
// Add: let whitelisted = skill_env_whitelist.iter().any(|w| upper == w.to_uppercase());
```

### Phase 17 D-15 envelope shape (setup-error precedent)

```rust
// Source: verified from 17-CONTEXT.md D-15 — the two existing envelope shapes:
// {"error": "capacity_exceeded", "current": N, "limit": N, "entry_size": N, "suggestion": "..."}
// {"error": "content_rejected", "reason": "injection_pattern_detected"}
//
// Phase 19 setup-error envelope (aligned with Python skill_view readiness shape):
json!({
    "status": "setup_needed",
    "name": skill_name,
    "readiness_status": "setup_needed",
    "missing_required_environment_variables": ["VAR1", "VAR2"],
    "missing_credential_files": ["path/to/token.json"],
    "setup_note": "Setup needed before using this skill: missing env $VAR1.",
})
```

### scan_context_content() current signature

```rust
// Source: verified from crates/ironhermes-core/src/context_scanner.rs:44
// Returns original content OR "[BLOCKED: {filename} contained potential prompt injection ({patterns}). Content not loaded.]"
pub fn scan_context_content(content: &str, filename: &str) -> String
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| opaque serde_yaml::Value for hermes metadata | Typed HermesMetadata struct | Phase 19 (D-17) | compile-time field access for filter and whitelist logic |
| static catalog text injected once at session start | per-render filtered catalog (toolset/tool aware) | Phase 19 (D-01) | skills disappear from catalog when their tool deps are toggled off |
| activate always succeeds or returns not-found | activate returns setup-error envelope on missing env/creds | Phase 19 (D-04/D-12) | agent can explain what's missing to user |
| sandbox env strips all vars containing KEY/TOKEN | sandbox env whitelists skill-declared vars before strip | Phase 19 (D-05) | skill-declared API keys reach Python child process |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | All Phase 19 crate dependencies already present; no new Cargo.toml entries needed | Standard Stack | Low risk — every pattern uses existing serde_yaml, regex, tracing; would be caught at cargo build |
| A2 | `SkillsTool::new()` is called with access to the full `Config` (including `SkillsConfig.config` map) at construction time | Pattern 3 | If SkillsTool is constructed before Config is fully loaded, body-injection (D-08) would need a separate init call; planner should verify the construction call site in agent/main.rs |
| A3 | `Sandbox::run()` is called with access to the list of currently-active skills' declared env vars | Pattern, Pitfall 2 | If sandbox is constructed without skill env context, a new parameter or shared state must thread the whitelist; planner should verify the call site in exec_tool.rs |
| A4 | Phase 19 treats all locally-discovered skills as `SkillSource::Builtin` (WARN-BUT-LOAD) since trust levels are Phase 19.1 | Pattern 5 | Correct per CONTEXT.md deferred section; Phase 19.1 will set SkillSource::Community for hub installs |
| A5 | `SkillsTool` is constructed once per session and holds an `Arc<SkillRegistry>` — the registry is not rebuilt per turn | Architecture | Consistent with Phase 07 design and load_with_paths() usage in tests; per-render filter in prompt_builder.rs reads from the same frozen registry |

---

## Open Questions

1. **Active toolset/tool snapshot for D-01 filter**
   - What we know: PromptBuilder has `skill_registry: Option<Arc<SkillRegistry>>` but no toolset/tool snapshot field
   - What's unclear: Phase 20 (TOOL-01..05) is where toolset management lands. For Phase 19, the filter can only check against an empty/stub active-toolset set unless the planner threads a real snapshot
   - Recommendation: Phase 19 implements the filter function with the correct signature and logic; passes an empty `HashSet` stub until Phase 20 wires the real toolset state. This means `requires_toolsets` / `requires_tools` filtering is correctly implemented but will show all skills until Phase 20 provides real toolset state. Functionally correct for Phase 19 scope.

2. **SkillsTool constructor call site for Config access**
   - What we know: `SkillsTool::new(registry, active_skills)` takes only registry and active_skills (verified skills_tool.rs:26)
   - What's unclear: body-injection (D-08) needs `SkillsConfig.config` map; credential dir (D-10) needs `HERMES_HOME`
   - Recommendation: Extend `SkillsTool::new()` to accept `config: Arc<Config>` or relevant extracted fields. Planner should audit `ironhermes-agent/src/main.rs` or agent factory to find the SkillsTool construction site.

3. **Sandbox construction call site for D-05 whitelist**
   - What we know: `Sandbox::new(config)` in sandbox.rs takes only `SandboxConfig`
   - What's unclear: active skill env vars must be threaded to `build_env()` at call time; the exec tool constructs the sandbox
   - Recommendation: Add `skill_env_whitelist: Vec<String>` to `SandboxConfig` or as a parameter to `Sandbox::run()`. Planner should verify exec_tool.rs construction pattern.

---

## Environment Availability

Step 2.6: SKIPPED — Phase 19 is code-only changes with no new external tool dependencies. The existing Rust toolchain, serde_yaml, and regex crates are already confirmed present by the project building in prior phases.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` (consistent with all prior phases) |
| Config file | none — workspace-level `cargo test` |
| Quick run command | `cargo test -p ironhermes-core skills 2>&1 \| head -40` |
| Full suite command | `cargo test --workspace 2>&1 \| tail -20` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SKILL-01 | HermesMetadata typed extraction from opaque blob | unit | `cargo test -p ironhermes-core test_hermes_metadata` | ❌ Wave 0 |
| SKILL-01 (D-18) | Unknown hermes fields preserved in extras bag, skill loads | unit | `cargo test -p ironhermes-core test_warn_but_load_unknown_fields` | ❌ Wave 0 |
| SKILL-01 (D-18) | Existing 07.2 skill with only tags metadata loads cleanly | unit | `cargo test -p ironhermes-core test_07_2_compat_metadata` | ❌ Wave 0 |
| SKILL-03 | requires_toolsets filter hides skill when toolset absent | unit | `cargo test -p ironhermes-core test_filter_requires_toolsets` | ❌ Wave 0 |
| SKILL-03 | fallback_for_tools hides skill when primary present | unit | `cargo test -p ironhermes-core test_filter_fallback_for_tools` | ❌ Wave 0 |
| SKILL-04 | activate returns setup-error envelope when env var missing | unit | `cargo test -p ironhermes-tools test_activate_missing_env_var` | ❌ Wave 0 |
| SKILL-05 | config header injected into skill body on activate | unit | `cargo test -p ironhermes-tools test_activate_config_injection` | ❌ Wave 0 |
| SKILL-06 | activate returns setup-error envelope when credential file missing | unit | `cargo test -p ironhermes-tools test_activate_missing_credential` | ❌ Wave 0 |
| SKILL-07 | community skill hard-rejected on scan hit at registry load | unit | `cargo test -p ironhermes-core test_community_skill_scan_reject` | ❌ Wave 0 |
| SKILL-07 | builtin skill WARN-BUT-LOAD on scan hit | unit | `cargo test -p ironhermes-core test_builtin_skill_scan_warn_load` | ❌ Wave 0 |
| SKILL-10 | platform filter regression: macos skill hidden on linux | unit | `cargo test -p ironhermes-core test_platform_filter` | ✅ existing |
| SKILL-11 | skill-declared env var reaches sandboxed child | integration | `cargo test -p ironhermes-exec test_skill_env_passthrough` | ❌ Wave 0 |

### Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-core skills 2>&1 | tail -20`
- **Per wave merge:** `cargo test --workspace 2>&1 | tail -20`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-core/src/skills.rs` — add unit tests for HermesMetadata parse, D-18 extras preservation, D-15 scan enforcement (covers SKILL-01, SKILL-07)
- [ ] `crates/ironhermes-tools/src/skills_tool.rs` — add unit tests for setup-error envelope, config body injection (covers SKILL-04, SKILL-05, SKILL-06)
- [ ] `crates/ironhermes-core/src/context_scanner.rs` — add unit tests for new skill-specific patterns (covers SKILL-07)
- [ ] `crates/ironhermes-exec/src/sandbox.rs` — add integration test for skill env whitelist passthrough (covers SKILL-11)

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | n/a |
| V3 Session Management | no | n/a |
| V4 Access Control | yes (skill source gating) | SkillSource enum + hard-reject for community on scan hit (D-15) |
| V5 Input Validation | yes | scan_skill_content() with RegexSet; WARN-BUT-LOAD for trusted sources |
| V6 Cryptography | no | n/a — credentials handled by presence check only, not encryption |

### Known Threat Patterns for Skills Framework

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via skill body | Tampering | scan_skill_content() at registry-load (D-16); hard-reject community (D-15) |
| Role hijack via role-marker strings | Spoofing | `<system>`, `you are now` patterns in SKILL_THREAT_PATTERNS |
| Credential exfiltration via skill instructions | Information Disclosure | `$HOME/.ssh`, `$HOME/.aws`, `HERMES_HOME/.env` patterns in SKILL_THREAT_PATTERNS |
| Env var dump via skill instructions | Information Disclosure | `printenv`, `env |`, `os.getenv(SECRET)` patterns |
| allowed-tools field privilege escalation | Elevation of Privilege | `^allowed-tools:` pattern triggers scan hit; already WARN-BUT-LOAD for builtins |
| Skill env var leaking into unintended sandbox | Information Disclosure | Whitelist only declared-and-present vars (D-05); missing vars not whitelisted |
| Hardcoded secrets embedded in skill | Information Disclosure | hardcoded_secret pattern detects `api_key = "..."` literals >= 20 chars |

---

## Sources

### Primary (HIGH confidence — verified by direct file read in this session)

- `crates/ironhermes-core/src/skills.rs` — SkillFrontmatter, SkillRecord, parse_skill_md, load_with_paths, catalog_text (lines 1-383)
- `crates/ironhermes-tools/src/skills_tool.rs` — SkillsTool, handle_activate current flow (lines 1-164)
- `crates/ironhermes-agent/src/prompt_builder.rs` — load_skills, build_split Skills slot (lines 1-450)
- `crates/ironhermes-core/src/config.rs` — SkillsConfig struct and tests (lines 347-376, 525-568)
- `crates/ironhermes-core/src/context_scanner.rs` — scan_context_content signature, THREAT_PATTERNS (lines 1-94)
- `crates/ironhermes-exec/src/sandbox.rs` — build_env, SECRET_PATTERNS, SAFE_VARS (lines 1-289)
- `hermes-agent/tools/skills_guard.py` — THREAT_PATTERNS full list (lines 82-500)
- `hermes-agent/tools/skills_tool.py` — _INJECTION_PATTERNS, readiness envelope shape, env var registration flow (lines 845-1227)
- `hermes-agent/tools/skill_manager_tool.py` — setup-error UX patterns (lines 1-75)
- `.planning/phases/17-memory-tools-external-providers/17-CONTEXT.md` — D-15 error envelope shape
- `.planning/milestones/v1.1-phases/07.2-.../07.2-CONTEXT.md` — WARN-BUT-LOAD precedent (D-13/D-14/D-15)

### Tertiary (LOW confidence — not verified this session)

- CONTEXT.md reference to `https://hermes-agent.nousresearch.com/docs/developer-guide/creating-skills` — not fetched (private/internal docs)

---

## Metadata

**Confidence breakdown:**
- Standard Stack: HIGH — all libraries confirmed present in source imports
- Architecture: HIGH — all integration points verified by reading actual code
- Pitfalls: HIGH — root causes verified from sandbox.rs and serde pattern analysis
- Instruction-smuggling patterns: HIGH — directly read from hermes-agent/tools/skills_guard.py and skills_tool.py
- Validation Architecture: HIGH — consistent with prior phases

**Research date:** 2026-04-14
**Valid until:** 2026-05-14 (stable codebase; patterns won't change without explicit refactor)
