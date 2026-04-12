# Phase 14: Context Files & SOUL.md - Research

**Researched:** 2026-04-12
**Domain:** Rust — PromptBuilder extension, filesystem walking, YAML frontmatter stripping, session-scoped visited-dirs tracking, tool-result injection
**Confidence:** HIGH (all findings verified from codebase)

## Summary

Phase 14 extends the existing `PromptBuilder` in `ironhermes-agent` to complete the context file loading pipeline. The partial implementation already handles CWD-only loading with a broken candidate list (includes lowercase variants, no git-root walk, no frontmatter stripping). Three distinct concerns must be addressed: (1) fix the initial context loading in `PromptBuilder` — git-root walk for `.hermes.md`, case-sensitive candidates, frontmatter stripping, `skip_context_files` flag; (2) add session-scoped progressive subdirectory discovery triggered by file-access tools; and (3) ensure SOUL.md + DEFAULT_AGENT_IDENTITY behavior is correct when `skip_context_files` is set.

The security scanning (`scan_context_content`) and truncation (`truncate_content`) utilities in `ironhermes-core` are complete and require no changes. File tools in `ironhermes-tools` require a post-execution hook mechanism so that discovered context can be appended to tool results. The agent loop dispatches tool results via `ChatMessage::tool_result` — subdirectory context appended to that string is the injection point.

**Primary recommendation:** Introduce a `ContextLoader` struct that owns git-root detection and directory walking logic, extend `PromptBuilder` with a `skip_context_files` bool, and add a session-scoped `SubdirDiscovery` (holding a `HashSet<PathBuf>` of visited dirs) that file-access tools consult post-execution via an `Arc<Mutex<SubdirDiscovery>>` passed through the agent construction chain.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01:** .hermes.md walks upward from CWD — first match wins. Walk stops at git root if found, otherwise stops at $HOME. Only one .hermes.md is loaded (no merging).

**D-02:** YAML frontmatter (between `---` markers) is stripped from .hermes.md before injection into the system prompt. Frontmatter is reserved for future config overrides per CTX-07. Content after stripping is scanned and truncated as normal.

**D-03:** If no git root is found, walk stops at $HOME (not filesystem root). Prevents loading context files from system directories.

**D-04:** Context files discovered in subdirectories are injected into tool results. When a file-access tool (read_file, write_file, list_directory, etc.) touches a new directory, discovered context is appended to that tool's result output.

**D-05:** Only file-access tools trigger subdirectory discovery. Other tools (web scraping, memory, etc.) do not trigger discovery even if they contain path-like arguments.

**D-06:** Walk direction is upward from the accessed file's directory, checking up to 5 parent directories. Each directory is checked at most once per session (tracked via a visited-dirs set).

**D-07:** Subdirectory discovery checks the full priority chain (.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules), not just .hermes.md. First match in each new directory wins.

**D-08:** Context file name matching is case-sensitive. Only exact names match: `.hermes.md`, `AGENTS.md`, `CLAUDE.md`, `.cursorrules`. Drop lowercase variants (`agents.md`, `claude.md`) from the candidate list.

**D-09:** AGENTS.md in HERMES_HOME and AGENTS.md in CWD serve separate roles and both load. HERMES_HOME/AGENTS.md is global agent configuration (always loaded as a separate prompt layer). CWD/AGENTS.md is project context (part of the priority chain, only loads if .hermes.md not found first). Two different purposes, both injected.

**D-10:** When `skip_context_files` is set (subagent delegation), SOUL.md is NOT loaded. The agent uses DEFAULT_AGENT_IDENTITY instead. Project context and AGENTS.md from HERMES_HOME are also skipped. Subagents get a clean, focused identity.

**D-11:** SOUL.md content is injected raw (after scan + truncate) as the first layer of the system prompt. No header wrapping — it IS the identity.

**D-12:** DEFAULT_AGENT_IDENTITY remains a hardcoded `const &str` in prompt_builder.rs. No file loading or `include_str!` needed.

### Claude's Discretion

- How to implement the visited-dirs set (HashSet<PathBuf> on the session/agent, or a shared Arc structure)
- How file-access tools detect "new directory" and trigger discovery (interceptor pattern vs. per-tool check)
- YAML frontmatter parsing approach (regex vs. dedicated parser like `gray_matter`)
- How subdirectory context injection is formatted within tool results
- Whether to add a `ContextLoader` struct/trait or extend the existing `PromptBuilder` for the walk logic
- Git root detection method (walk up looking for `.git` directory)

### Deferred Ideas (OUT OF SCOPE)

None from discussion. "Add setup wizard and config scaffolding for gateway testing" belongs in Phase 23.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CTX-01 | Context file priority chain: .hermes.md > AGENTS.md > CLAUDE.md > .cursorrules (first match wins) | `load_project_context()` needs candidate list fixed and git-root walk added |
| CTX-02 | .hermes.md walks CWD to git root; AGENTS.md/CLAUDE.md/cursorrules check CWD only | New `find_git_root()` helper + walk loop in PromptBuilder |
| CTX-03 | Progressive subdirectory discovery injected into tool results | New `SubdirDiscovery` struct + file-tool post-execution hook |
| CTX-04 | Each subdirectory checked at most once per session; max 5 parent directories | `HashSet<PathBuf>` in `SubdirDiscovery` + depth counter in walk |
| CTX-05 | All context files security scanned | `scan_context_content()` already exists — apply to all load paths |
| CTX-06 | Context files truncated at 20,000 chars, 70/20 ratio | `truncate_content()` already exists — apply to all load paths |
| CTX-07 | .hermes.md YAML frontmatter stripped before injection | New `strip_yaml_frontmatter()` function, called before scan+truncate |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| std::collections::HashSet | stdlib | visited-dirs tracking | Zero-dep, O(1) lookup, PathBuf-hashable |
| std::path::PathBuf | stdlib | Directory walking, git root detection | Native path manipulation |
| regex (already in Cargo.toml) | workspace | YAML frontmatter stripping | Already in dependency graph; simple `---` fence detection needs no external crate |
| ironhermes-core (existing) | workspace | scan_context_content, truncate_content, CONTEXT_FILE_MAX_CHARS | Fully implemented, no changes needed |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| Arc<Mutex<SubdirDiscovery>> | stdlib | Share visited-dirs between agent and tools | Session-scoped state shared across concurrent tool calls |
| tracing::debug | workspace | Discovery event logging | Consistent with existing debug logging pattern in prompt_builder.rs |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Regex for frontmatter stripping | `gray_matter` crate | gray_matter is heavier, adds a dep; frontmatter is a simple `---\n...\n---\n` pattern that regex handles in 3 lines |
| Arc<Mutex<SubdirDiscovery>> | Arc<RwLock<SubdirDiscovery>> | RwLock allows parallel reads; but discovery writes are rare and tool calls are sequential; Mutex is simpler |
| Per-tool post-execution hook | Wrapper in agent_loop execute_tool_call | execute_tool_call is in agent_loop.rs and already returns a String result — appending there keeps tool implementations clean |

**Installation:** No new dependencies required. All functionality uses stdlib + existing workspace deps.

## Architecture Patterns

### Recommended Project Structure

```
crates/ironhermes-agent/src/
├── prompt_builder.rs     # PromptBuilder: add skip_context_files, git-root walk, frontmatter strip
├── context_loader.rs     # NEW: ContextLoader (git root detection, walk logic, strip_yaml_frontmatter)
├── subdir_discovery.rs   # NEW: SubdirDiscovery struct (visited-dirs set, check_directory logic)
└── agent_loop.rs         # Wire Arc<SubdirDiscovery> through execute_tool_call result appending

crates/ironhermes-tools/src/
└── file_tools.rs         # No struct changes needed; discovery runs at execute_tool_call layer
```

### Pattern 1: Git Root Detection via .git Walk

Walk from a starting directory upward, looking for a `.git` entry (directory or file for worktrees). Stop at `$HOME` if not found.

```rust
// Source: [VERIFIED: codebase — ironhermes-core/src/config.rs walk pattern, stdlib]
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir(); // or std::env::var("HOME")
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        // Stop at $HOME — do not walk into system directories (D-03)
        if home.as_deref() == Some(&current) {
            return None;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => return None,
        }
    }
}
```

**Note:** The codebase has no `dirs` dependency. Use `std::env::var("HOME").ok().map(PathBuf::from)` as the $HOME sentinel [VERIFIED: grep on Cargo.toml workspace].

### Pattern 2: YAML Frontmatter Stripping

YAML frontmatter is content between the first `---` line at the start of the file and the next `---` line. Strip it; return the remaining content (trimmed).

```rust
// Source: [VERIFIED: codebase — D-02 from CONTEXT.md, regex already in dep graph]
pub fn strip_yaml_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content; // no frontmatter
    }
    // Find the closing --- (skip first line)
    let after_open = &trimmed[3..]; // skip opening ---
    // Find next line that is exactly "---"
    if let Some(close_pos) = after_open.find("\n---") {
        let after_close = &after_open[close_pos + 4..]; // skip \n---
        // Skip optional newline after closing marker
        after_close.trim_start_matches('\n')
    } else {
        content // malformed frontmatter — return as-is
    }
}
```

### Pattern 3: SubdirDiscovery — Session-Scoped Visited-Dirs

```rust
// Source: [VERIFIED: codebase — D-06, D-07, Arc<Mutex<>> pattern from agent_loop.rs]
pub struct SubdirDiscovery {
    visited: HashSet<PathBuf>,
}

impl SubdirDiscovery {
    pub fn new() -> Self {
        Self { visited: HashSet::new() }
    }

    /// Check a file path for new context. Returns Some(context_text) if a new
    /// context file was found in an unvisited directory, None otherwise.
    pub fn check_path(&mut self, file_path: &Path) -> Option<String> {
        let start_dir = if file_path.is_dir() {
            file_path.to_path_buf()
        } else {
            file_path.parent()?.to_path_buf()
        };

        let mut current = start_dir;
        let mut depth = 0;
        let mut found: Option<String> = None;

        while depth < 5 {
            if self.visited.contains(&current) {
                break; // already checked this dir and all above it this session
            }
            self.visited.insert(current.clone());

            // Priority chain — case-sensitive, first match wins (D-07, D-08)
            let candidates = [".hermes.md", "AGENTS.md", "CLAUDE.md", ".cursorrules"];
            for &name in &candidates {
                let path = current.join(name);
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if !content.trim().is_empty() {
                        let stripped = if name == ".hermes.md" {
                            strip_yaml_frontmatter(&content).to_string()
                        } else {
                            content.clone()
                        };
                        let scanned = scan_context_content(&stripped, name);
                        let truncated = truncate_content(&scanned, name, CONTEXT_FILE_MAX_CHARS);
                        found = Some(format!(
                            "\n\n[Context: {}/{}]\n{}",
                            current.display(), name, truncated
                        ));
                        break; // first match wins
                    }
                }
            }

            if found.is_some() {
                break; // only inject one per check_path call
            }

            match current.parent() {
                Some(p) => { current = p.to_path_buf(); depth += 1; }
                None => break,
            }
        }

        found
    }
}
```

### Pattern 4: Wiring SubdirDiscovery Through AgentLoop

The `execute_tool_call` method in `agent_loop.rs` returns a `String`. Subdirectory discovery appends to this return value when the tool is a file-access tool.

```rust
// Source: [VERIFIED: codebase — agent_loop.rs execute_tool_call lines 485+]
// In AgentLoop struct, add:
subdir_discovery: Option<Arc<std::sync::Mutex<SubdirDiscovery>>>,

// In execute_tool_call, after obtaining result:
let is_file_tool = matches!(name.as_str(), "read_file" | "write_file" | "patch" | "list_directory");
if is_file_tool {
    if let Some(ref disc) = self.subdir_discovery {
        // extract path arg from args["path"]
        if let Some(path_str) = args.get("path").and_then(|v| v.as_str()) {
            let path = std::path::Path::new(path_str);
            if let Ok(mut discovery) = disc.lock() {
                if let Some(ctx) = discovery.check_path(path) {
                    result.push_str(&ctx);
                }
            }
        }
    }
}
```

### Pattern 5: PromptBuilder skip_context_files

```rust
// Source: [VERIFIED: codebase — D-10, D-11, D-12 from CONTEXT.md + prompt_builder.rs]
pub struct PromptBuilder {
    // ... existing fields ...
    skip_context_files: bool,
}

impl PromptBuilder {
    pub fn skip_context_files(mut self) -> Self {
        self.skip_context_files = true;
        self
    }
}

// In load_context():
pub fn load_context(mut self, cwd: &Path) -> Self {
    if self.skip_context_files {
        return self; // D-10: subagents get clean identity
    }
    self.load_soul_md();
    self.load_project_context(cwd);
    self.load_agents_md();
    self
}

// In build():
let identity = if self.skip_context_files {
    DEFAULT_AGENT_IDENTITY  // D-10: use default, not SOUL.md
} else {
    self.soul_content.as_deref().unwrap_or(DEFAULT_AGENT_IDENTITY)
};
```

### Pattern 6: Fixed Priority Chain (case-sensitive, git-root walk for .hermes.md)

```rust
// Source: [VERIFIED: codebase — D-01, D-02, D-08 + current load_project_context()]
fn load_project_context(&mut self, cwd: &Path) {
    // Step 1: Try .hermes.md with git-root walk (D-01, D-02)
    let walk_stop = find_git_root(cwd)
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from));

    let mut dir = cwd.to_path_buf();
    loop {
        let path = dir.join(".hermes.md");
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    let stripped = strip_yaml_frontmatter(&content);
                    let scanned = scan_context_content(stripped, ".hermes.md");
                    let truncated = truncate_content(&scanned, ".hermes.md", CONTEXT_FILE_MAX_CHARS);
                    self.project_context = Some(format!("## .hermes.md\n\n{}", truncated));
                    return; // D-01: first match wins
                }
            }
        }
        if walk_stop.as_deref() == Some(&dir) || dir.parent().is_none() {
            break;
        }
        dir = dir.parent().unwrap().to_path_buf();
    }

    // Step 2: Check CWD only for remaining candidates (D-08: case-sensitive)
    let cwd_candidates = ["AGENTS.md", "CLAUDE.md", ".cursorrules"];
    for &filename in &cwd_candidates {
        let path = cwd.join(filename);
        if let Ok(content) = std::fs::read_to_string(&path) {
            if !content.trim().is_empty() {
                let scanned = scan_context_content(&content, filename);
                let truncated = truncate_content(&scanned, filename, CONTEXT_FILE_MAX_CHARS);
                self.project_context = Some(format!("## {}\n\n{}", filename, truncated));
                return;
            }
        }
    }
}
```

### Anti-Patterns to Avoid

- **Merging multiple .hermes.md files:** D-01 says first match wins, walk stops. Do not concatenate context from multiple directories.
- **Case-insensitive filename matching:** D-08 requires exact names. The current code includes `agents.md` and `claude.md` — these must be removed.
- **Modifying context mid-session:** The frozen-snapshot pattern means `load_context()` runs once at session start. SubdirDiscovery is session-scoped but only appends to tool results, never mutates the system prompt.
- **Walking past $HOME:** D-03 explicitly forbids walking to filesystem root. The walk-stop sentinel must be set before the loop.
- **Scanning frontmatter content:** Strip frontmatter BEFORE calling `scan_context_content`. Frontmatter is internal config, not user-facing content, and should not pollute the injected context.
- **Triggering discovery for non-file tools:** D-05. Only tools with explicit path args from the file-access set (`read_file`, `write_file`, `patch`, `list_directory`) trigger discovery.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Security scanning | Custom regex/threat detection | `scan_context_content()` in ironhermes-core | Already has 10 threat patterns + invisible unicode; fully tested |
| Content truncation | Custom head/tail logic | `truncate_content()` in ironhermes-core | Already implements exact 70/20 ratio with correct truncation marker |
| YAML parsing (general) | Custom YAML parser | Simple frontmatter regex | Phase only needs `---` fence detection, not full YAML parsing |
| Concurrent state sharing | Custom lock-free structure | `Arc<std::sync::Mutex<>>` | Matches existing pattern throughout codebase (agent_loop.rs active_skills) |

**Key insight:** The security scanning and truncation infrastructure is production-complete. The work is plumbing (wiring, extension), not net-new implementation.

## Common Pitfalls

### Pitfall 1: Forgetting to Canonicalize Paths Before HashSet Insert
**What goes wrong:** Two paths pointing to the same directory (e.g., `./src` vs `/abs/path/src`) miss deduplication, causing re-scanning.
**Why it happens:** HashSet equality is byte-level; relative and absolute forms of the same path are not equal.
**How to avoid:** Canonicalize paths with `std::fs::canonicalize()` before inserting into the visited set. Fall back to the original path if canonicalization fails (symlink targets, nonexistent dirs).
**Warning signs:** Same context file injected twice in tool results.

### Pitfall 2: Scanning Frontmatter Content
**What goes wrong:** If `strip_yaml_frontmatter` is called AFTER `scan_context_content`, the YAML frontmatter is scanned and may trigger false positives (e.g., a YAML key named `override`).
**Why it happens:** Wrong order of operations.
**How to avoid:** Always call `strip_yaml_frontmatter` first, then `scan_context_content`, then `truncate_content`.

### Pitfall 3: Visited-Dirs Set Not Shared Between Tool Calls
**What goes wrong:** Each tool call creates a fresh `SubdirDiscovery`, so the same directory is re-discovered on every tool call.
**Why it happens:** `SubdirDiscovery` is constructed per-call instead of per-session.
**How to avoid:** `SubdirDiscovery` must be wrapped in `Arc<Mutex<>>` and attached to `AgentLoop` at construction, not inside `execute_tool_call`.

### Pitfall 4: Walk Direction for Subdirectory Discovery
**What goes wrong:** Walking DOWNWARD into subdirectories from CWD instead of upward from the accessed file's directory.
**Why it happens:** Misreading D-06 ("up to 5 parent directories").
**How to avoid:** D-06 says "walk upward from the accessed file's directory." The starting point is the directory containing the accessed file, then parent, grandparent, etc. — not CWD down.

### Pitfall 5: Injecting Context Even When File Tool Errors
**What goes wrong:** If a file tool returns an error (e.g., "file not found"), appending context to the error message is confusing and misleads the agent.
**Why it happens:** Context injection happens unconditionally after tool dispatch.
**How to avoid:** Only append subdirectory context when the tool succeeds (result does not start with error prefix). Or inject regardless — the context is valid metadata about the directory regardless of read outcome. Either approach is defensible; choose one and document it.

### Pitfall 6: Breaking Existing Tests When Removing Lowercase Candidates
**What goes wrong:** Existing `test_project_context_priority` uses `.hermes.md` (lowercase correct) but other tests may rely on `agents.md` / `claude.md` behavior.
**Why it happens:** The current candidate list has lowercase variants that tests may exercise.
**How to avoid:** Audit all `load_project_context` tests before removing lowercase entries. Update tests to use exact-case filenames per D-08.

## Code Examples

### strip_yaml_frontmatter — Verified Pattern
```rust
// Source: [VERIFIED: codebase — D-02, standard YAML frontmatter convention]
pub fn strip_yaml_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after_open = trimmed.trim_start_matches("---");
    // Find closing --- on its own line
    for (i, line) in after_open.lines().enumerate() {
        if i > 0 && line.trim() == "---" {
            // Skip past this closing line
            let close_byte = after_open
                .match_indices('\n')
                .nth(i - 1) // newline before closing ---
                .map(|(pos, _)| pos + 1)
                .unwrap_or(0);
            let rest = &after_open[close_byte..];
            // rest starts with "---\n" or "---"
            return rest.trim_start_matches("---").trim_start_matches('\n');
        }
    }
    content // no closing marker found — return as-is
}
```

### Existing scan + truncate pipeline (no changes needed)
```rust
// Source: [VERIFIED: codebase — context_scanner.rs lines 44-94]
// Usage pattern already established in prompt_builder.rs:
let scanned = scan_context_content(&content, filename);
let truncated = truncate_content(&scanned, filename, CONTEXT_FILE_MAX_CHARS);
```

### $HOME sentinel without external deps
```rust
// Source: [VERIFIED: codebase — no dirs crate in workspace Cargo.toml]
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Lowercase candidate variants (agents.md, claude.md) | Case-sensitive exact names only | Phase 14 (D-08) | Simplifies matching, prevents unexpected loads |
| CWD-only .hermes.md check | Git-root walk for .hermes.md | Phase 14 (D-01, D-02) | Finds project-level hermes config in monorepos/nested dirs |
| No frontmatter stripping | Strip YAML frontmatter before injection | Phase 14 (D-02, CTX-07) | Prevents config leaking into agent identity |
| No subdirectory discovery | Progressive discovery injected into tool results | Phase 14 (CTX-03, CTX-04) | Agent learns project context as it navigates |
| No skip_context_files | skip_context_files flag for subagents | Phase 14 (D-10) | Subagents get clean focused identity |

**Deprecated/outdated:**
- Lowercase candidate group entries (`agents.md`, `claude.md`) in `load_project_context`: must be removed per D-08 [VERIFIED: prompt_builder.rs lines 109-114]

## Open Questions

1. **list_directory tool exists?**
   - What we know: `file_tools.rs` contains ReadFileTool, WriteFileTool, PatchFileTool, SearchFilesTool. No `ListDirectoryTool` was found.
   - What's unclear: Is `list_directory` a planned tool name, or does a different tool name cover directory listing?
   - Recommendation: Audit `file_tools.rs` and `ToolRegistry` for all registered file-access tools before hardcoding the tool name allowlist in the discovery trigger. Use the actual registered names.

2. **Path argument extraction for search_files**
   - What we know: `search_files` takes a `path` arg (optional, defaults to "."). It is plausibly a file-access tool.
   - What's unclear: D-05 says "file-access tools" — does search_files count? It doesn't touch individual files.
   - Recommendation: Treat `search_files` as a file-access tool for discovery purposes (it accesses file system content). Include it in the trigger set.

3. **Subagent construction path**
   - What we know: `subagent_runner.rs` exists in the agent crate. `skip_context_files` needs to be set when running subagents (D-10).
   - What's unclear: Whether `subagent_runner.rs` constructs its own `PromptBuilder` or reuses the parent's system message.
   - Recommendation: Read `subagent_runner.rs` before implementing the `skip_context_files` plumbing to understand the construction site.

## Environment Availability

Step 2.6: SKIPPED — Phase 14 is purely Rust code changes with no external tool/service dependencies. All required infrastructure (Rust toolchain, Cargo) is verified present by prior phases completing successfully.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness (cargo test) |
| Config file | None — standard `#[cfg(test)]` modules in-source |
| Quick run command | `cargo test -p ironhermes-agent -- prompt_builder` |
| Full suite command | `cargo test -p ironhermes-agent -p ironhermes-core -p ironhermes-tools` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| CTX-01 | Priority chain: .hermes.md wins over AGENTS.md wins over CLAUDE.md | unit | `cargo test -p ironhermes-agent -- test_project_context_priority` | Partial — existing test covers .hermes.md vs CLAUDE.md only |
| CTX-02 | .hermes.md found in parent dir (git root walk) | unit | `cargo test -p ironhermes-agent -- test_hermes_md_git_root_walk` | ❌ Wave 0 |
| CTX-03 | Tool result gets context appended for new directory | unit | `cargo test -p ironhermes-agent -- test_subdir_discovery_injects` | ❌ Wave 0 |
| CTX-04 | Same directory not checked twice | unit | `cargo test -p ironhermes-agent -- test_subdir_discovery_visited_once` | ❌ Wave 0 |
| CTX-04 | Walk stops at 5 parent directories | unit | `cargo test -p ironhermes-agent -- test_subdir_discovery_depth_limit` | ❌ Wave 0 |
| CTX-05 | Injected context is scanned | unit | Existing `context_scanner.rs` tests cover scan; wire-up test needed | Partial |
| CTX-06 | Truncation applied at 20K chars | unit | Existing `test_truncate_long_content` in context_scanner | ✅ |
| CTX-07 | Frontmatter stripped from .hermes.md | unit | `cargo test -p ironhermes-agent -- test_hermes_md_frontmatter_stripped` | ❌ Wave 0 |
| D-08 | Lowercase variants not loaded | unit | `cargo test -p ironhermes-agent -- test_case_sensitive_candidates` | ❌ Wave 0 |
| D-10 | skip_context_files uses default identity | unit | `cargo test -p ironhermes-agent -- test_skip_context_files_default_identity` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-agent -- prompt_builder`
- **Per wave merge:** `cargo test -p ironhermes-agent -p ironhermes-core -p ironhermes-tools`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `test_hermes_md_git_root_walk` — covers CTX-02 (new test in prompt_builder.rs `#[cfg(test)]`)
- [ ] `test_subdir_discovery_injects` — covers CTX-03 (new test in subdir_discovery.rs)
- [ ] `test_subdir_discovery_visited_once` — covers CTX-04 dedup
- [ ] `test_subdir_discovery_depth_limit` — covers CTX-04 depth cap
- [ ] `test_hermes_md_frontmatter_stripped` — covers CTX-07
- [ ] `test_case_sensitive_candidates` — covers D-08
- [ ] `test_skip_context_files_default_identity` — covers D-10

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | no | — |
| V5 Input Validation | yes | `scan_context_content()` — existing, blocks injection/exfiltration/invisible unicode |
| V6 Cryptography | no | — |

### Known Threat Patterns for Context File Loading

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via .hermes.md | Tampering | `scan_context_content()` before injection — blocks 10 known patterns |
| Invisible unicode in context files | Tampering | `scan_context_content()` checks `INVISIBLE_CHARS` list |
| Credential exfiltration via malicious context | Information Disclosure | `scan_context_content()` blocks curl+secret and cat .env patterns |
| Frontmatter leaking internal config into prompt | Information Disclosure | `strip_yaml_frontmatter()` removes frontmatter before scan |
| Walk past filesystem boundaries | Elevation of Privilege | Walk stops at $HOME (D-03) — prevents loading /etc or system context files |
| Re-injection of already-scanned context | Tampering | Scan runs on every load path including subdirectory discovery — not cached pre-scan |

## Sources

### Primary (HIGH confidence)
- [VERIFIED: codebase] `crates/ironhermes-agent/src/prompt_builder.rs` — current PromptBuilder implementation, candidate list bug confirmed at lines 109-114
- [VERIFIED: codebase] `crates/ironhermes-core/src/context_scanner.rs` — scan_context_content and truncate_content, fully functional
- [VERIFIED: codebase] `crates/ironhermes-tools/src/file_tools.rs` — file tool list, execute signatures, no discovery hook
- [VERIFIED: codebase] `crates/ironhermes-agent/src/agent_loop.rs` — execute_tool_call at line 485, tool dispatch, result return
- [VERIFIED: codebase] `crates/ironhermes-agent/Cargo.toml` — no dirs crate, regex already in dep graph
- [VERIFIED: codebase] `.planning/phases/14-context-files-soul-md/14-CONTEXT.md` — all locked decisions D-01 through D-12

### Secondary (MEDIUM confidence)
- [ASSUMED] Standard YAML frontmatter convention: `---` on its own line at start of file, closing `---` on its own line. Used by Jekyll, Hugo, Obsidian, and most markdown tooling.

### Tertiary (LOW confidence)
None.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | list_directory tool exists or will exist as a file-access tool | Open Questions #1 | Discovery trigger list may need adjustment; low risk, just update the name allowlist |
| A2 | YAML frontmatter convention uses `---` fences at line boundaries | Standard Stack / Code Examples | If .hermes.md uses a different frontmatter format (e.g., TOML `+++`), stripping will silently no-op; low risk since format is controlled by ironhermes project |

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all libraries already in codebase, no new deps needed
- Architecture: HIGH — all integration points verified in source, patterns match established codebase conventions
- Pitfalls: HIGH — identified from direct code inspection of existing implementation gaps

**Research date:** 2026-04-12
**Valid until:** Stable (no external deps; valid until codebase changes)
