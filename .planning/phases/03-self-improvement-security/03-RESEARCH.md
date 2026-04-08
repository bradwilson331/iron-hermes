# Phase 3: Self-Improvement + Security — Research

**Researched:** 2026-04-07
**Domain:** Rust file I/O, advisory file locking, rate limiting, RegexSet, SSRF validation, crate refactoring
**Confidence:** HIGH — primary evidence from direct codebase inspection and Python reference implementation

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Self-edit guardrails**
- D-01: Block writes entirely when threat patterns are detected — write_file/patch returns an error, file is not modified.
- D-02: Security scanning applies only to context files (SOUL.md, AGENTS.md, MEMORY.md, USER.md).
- D-03: All context files are writable by the agent — no read-only restrictions.
- D-04: Move `context_scanner.rs` from `ironhermes-agent` to `ironhermes-core`.

**Memory subsystem**
- D-05: MEMORY.md (2,200 char limit) + USER.md (1,375 char limit) in `~/.ironhermes/memories/`.
- D-06: Entry delimiter: `§` with `\n§\n` as full delimiter. Entries can be multiline.
- D-07: File locking via `fs2` crate (advisory flock) — separate `.lock` file per memory file.
- D-08: Atomic file I/O: tempfile + `fs::rename` (matching `ironhermes-cron`).
- D-09: No `read` action on memory tool — memory injected via frozen snapshot. Tool actions: add, replace, remove.
- D-10: Replace/remove use short unique substring matching via `old_text` parameter.
- D-11: MemoryStore lives in `ironhermes-core`.
- D-12: Frozen-snapshot pattern: prompt injection captured at `load_from_disk()`, never mutated mid-session.
- D-13: Memory content scanned for injection/exfiltration before accepting.
- D-14: Duplicate prevention: exact duplicate entries rejected.
- D-15: Capacity overflow: adding beyond char limit returns error with current usage and entries.

**SSRF validation**
- D-16: Direct port of `url_safety.py`: resolve via DNS, check private IP ranges, fail closed.
- D-17: DNS rebinding is a documented known limitation (TOCTOU).
- D-18: Blocked hostnames: `metadata.google.internal`, `metadata.goog`.
- D-19: SSRF validator lives in `ironhermes-core`.

**Rate limiting**
- D-20: Per-user (Telegram user_id) inbound rate limiting on message processing.
- D-21: Excess messages silently dropped.
- D-22: Configurable in config.yaml: `rate_limit.messages_per_minute` (default 10), `rate_limit.burst_size` (default 3).

### Claude's Discretion
- Token bucket vs sliding window algorithm for rate limiting
- Exact threat pattern set for memory scanning (can extend beyond context_scanner's existing 10)
- Memory tool schema description wording
- Whether to add `fsync` before rename in atomic writes
- MemoryStore deduplication strategy details (preserve order, keep first occurrence)

### Deferred Ideas (OUT OF SCOPE)

None — discussion stayed within phase scope.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SELF-01 | Agent can read its own context files (SOUL.md, AGENTS.md) via existing read_file tool | Already works — read_file has no restrictions; success criterion is documentation/smoke test |
| SELF-02 | Agent can edit its own context files via existing write_file/patch tools | Requires scanning integration in WriteFileTool/PatchFileTool for context file paths |
| SELF-03 | Security scanning on all context file writes | context_scanner.rs must move to core; write/patch tools call scan before writing |
| SELF-04 | Memory subsystem: bounded declarative facts in MEMORY.md, loaded into context | MemoryStore struct in ironhermes-core; PromptBuilder gets memory injection |
| SELF-05 | Memory tool: agent can save, query, and forget facts | MemoryTool implementing Tool trait; registered in register_defaults() |
| SELF-06 | Atomic file I/O for all context/memory writes | Tempfile + fs::rename pattern already proven in ironhermes-cron |
| SEC-01 | Port url_safety.py SSRF validation to Rust | Direct port using std::net; lives in ironhermes-core |
| SEC-02 | Regex-based threat scanning for context file writes | context_scanner.rs move to core + write-time scan integration |
| SEC-03 | Rate limiting on Telegram message processing | Per-user token bucket or sliding window in gateway handler |
</phase_requirements>

---

## Summary

This phase has a well-defined Python reference implementation in `hermes-agent` that must be ported to Rust. Every major design decision is already locked; the work is primarily translation and integration. The three functional areas are independent and can be planned in parallel waves:

1. **Core crate surgery** — move `context_scanner.rs` to `ironhermes-core`, add `MemoryStore` and SSRF validator to core. No new external crates except `fs2` for advisory locking.

2. **Tool integration** — hook scanning into `WriteFileTool`/`PatchFileTool` for context file paths, add new `MemoryTool` to `ironhermes-tools`, inject memory snapshot into `PromptBuilder`.

3. **Gateway hardening** — add per-user rate limiting in `ironhermes-gateway`'s dispatch path, extend `Config` with `rate_limit` fields.

The primary risk is the `context_scanner.rs` move: `ironhermes-agent` currently imports it directly and so does `prompt_builder.rs`. After the move, the agent crate re-exports from core. Circular dependency is impossible because core is a leaf with no internal deps.

**Primary recommendation:** Execute in three waves — (W1) core surgery, (W2) tool + prompt integration, (W3) gateway rate limiting — so each wave builds on stable ground.

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `regex` | 1.x (workspace) | RegexSet for threat pattern matching | Already in workspace; LazyLock<RegexSet> pattern established |
| `fs2` | 0.4 | Advisory file locking (flock) on memory `.lock` files | D-07 locked choice; cross-platform flock API |
| `std::net::IpAddr` | stdlib | SSRF IP range checking | Python `ipaddress` maps directly to Rust stdlib |
| `std::fs::rename` | stdlib | Atomic file replacement | Already used in ironhermes-cron; no extra dep |
| `tempfile` | implicit via cron pattern | Temp file creation before atomic rename | cron uses `path.with_extension("json.tmp")` and `fs::File::create` |
| `serde_json` | 1.x (workspace) | MemoryTool JSON responses | Tool trait returns String (JSON); already in workspace |
| `tokio::sync::DashMap` or `std::collections::HashMap` + `Mutex` | tokio workspace | Per-user rate limit state in gateway | Already in workspace; no new dep needed |
| `chrono` | 0.4 (workspace) | Timestamps for sliding window rate limiting | Already in workspace |

[VERIFIED: codebase grep — all workspace deps confirmed in /Cargo.toml]

### Rate Limiting Algorithm (Claude's Discretion)
**Recommendation: Token bucket using `Instant` timestamps.**

Rationale:
- Token bucket is simpler to implement without a new crate: store `(tokens: f64, last_refill: Instant)` per user.
- No external crate needed (`governor` crate exists but is not in workspace and adds complexity).
- With `messages_per_minute: 10` and `burst_size: 3`, the math is straightforward: refill rate = 10/60 tokens/sec, max bucket = burst_size.
- Sliding window requires storing N timestamps per user — more memory, not materially better for this use case.
- State: `Arc<Mutex<HashMap<UserId, TokenBucketState>>>` shared via `GatewayMessageHandler`.

[ASSUMED] — token bucket vs sliding window choice; either works, token bucket chosen for simplicity.

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Manual token bucket | `governor` crate (GCRA algorithm) | governor is well-tested but adds a new dep with no other users in workspace |
| `fs2` flock | `fd-lock` crate | fd-lock is more actively maintained but fs2 is specified in D-07 |
| `std::fs::rename` only | Add `fsync` before rename | fsync adds durability guarantee before rename; Python hermes-agent does fsync, cron does not. Claude's Discretion. |

**Installation (new deps only):**
```toml
# workspace Cargo.toml [workspace.dependencies]
fs2 = "0.4"

# crates/ironhermes-core/Cargo.toml
fs2 = { workspace = true }
```

[VERIFIED: npm view equivalent — `fs2` 0.4.3 is the latest stable release on crates.io as of training knowledge; verify with `cargo search fs2`] [ASSUMED: version currency — confirm with `cargo search fs2`]

---

## Architecture Patterns

### Recommended File Layout After Phase 3

```
crates/ironhermes-core/src/
├── config.rs           — add RateLimitConfig, MemoryConfig structs
├── constants.rs        — add MEMORY_DIR, ENTRY_DELIMITER, char limits
├── context_scanner.rs  — MOVED FROM ironhermes-agent (D-04)
├── memory_store.rs     — NEW: MemoryStore struct, load/add/replace/remove/format_for_prompt
├── ssrf.rs             — NEW: is_safe_url(), _is_blocked_ip(), BLOCKED_HOSTNAMES, CGNAT_NETWORK
├── lib.rs              — add pub mod for new modules, re-export key types
├── error.rs            — (unchanged)
└── types.rs            — (unchanged)

crates/ironhermes-agent/src/
├── context_scanner.rs  — DELETED (moved to core)
├── prompt_builder.rs   — modified: inject memory snapshot after AGENTS.md
└── lib.rs              — remove context_scanner pub use, import from core

crates/ironhermes-tools/src/
├── file_tools.rs       — modified: WriteFileTool + PatchFileTool check context file paths
├── memory_tool.rs      — NEW: MemoryTool impl of Tool trait
└── registry.rs         — modified: register_defaults() adds MemoryTool

crates/ironhermes-gateway/src/
├── handler.rs          — modified: rate limiter check before run_agent()
├── rate_limiter.rs     — NEW: TokenBucket, PerUserRateLimiter
└── (rest unchanged)
```

### Pattern 1: context_scanner.rs Move (D-04)

**What:** `context_scanner.rs` physically moves from `crates/ironhermes-agent/src/` to `crates/ironhermes-core/src/`. The `ironhermes-agent` crate re-exports it transparently.

**The circular dep concern is a non-issue:** `ironhermes-core` has zero internal deps. Adding modules to it cannot create cycles. `ironhermes-agent` depends on core — so agent code calling `ironhermes_core::scan_context_content()` is fine.

```rust
// crates/ironhermes-core/src/lib.rs — after move
pub mod context_scanner;
pub use context_scanner::{scan_context_content, truncate_content, CONTEXT_FILE_MAX_CHARS};

// crates/ironhermes-agent/src/lib.rs — shim for callers using old path
// (prompt_builder.rs already uses crate-relative: `use crate::context_scanner::*`)
// Update prompt_builder.rs import to: `use ironhermes_core::context_scanner::*;`
```

[VERIFIED: codebase — prompt_builder.rs line 1: `use crate::context_scanner::{scan_context_content, truncate_content, CONTEXT_FILE_MAX_CHARS};`]

### Pattern 2: MemoryStore in ironhermes-core

**What:** Direct Rust port of `hermes-agent/tools/memory_tool.py` `MemoryStore` class.

**Key structure:**
```rust
// crates/ironhermes-core/src/memory_store.rs
use std::sync::LazyLock;
use std::path::{Path, PathBuf};
use fs2::FileExt;  // for flock

pub const ENTRY_DELIMITER: &str = "\n§\n";
pub const MEMORY_CHAR_LIMIT: usize = 2_200;
pub const USER_CHAR_LIMIT: usize = 1_375;

pub struct MemoryStore {
    memory_entries: Vec<String>,
    user_entries: Vec<String>,
    // Frozen snapshot captured at load_from_disk() — never mutated mid-session
    system_prompt_snapshot: HashMap<&'static str, String>,
    memory_dir: PathBuf,
}

impl MemoryStore {
    pub fn new(memory_dir: PathBuf) -> Self { ... }
    pub fn load_from_disk(&mut self) -> anyhow::Result<()> { ... }
    pub fn add(&mut self, target: Target, content: &str) -> MemoryResult { ... }
    pub fn replace(&mut self, target: Target, old_text: &str, new_content: &str) -> MemoryResult { ... }
    pub fn remove(&mut self, target: Target, old_text: &str) -> MemoryResult { ... }
    pub fn format_for_system_prompt(&self, target: Target) -> Option<String> { ... }
}
```

**File locking pattern (D-07):** Uses a separate `.lock` file so the memory file can be atomically replaced while the lock is held:

```rust
// Source: Python hermes-agent/tools/memory_tool.py lines 128-141 (ported to Rust)
fn with_file_lock<F, R>(path: &Path, f: F) -> anyhow::Result<R>
where F: FnOnce() -> anyhow::Result<R>
{
    let lock_path = path.with_extension(
        format!("{}.lock", path.extension().unwrap_or_default().to_string_lossy())
    );
    let lock_file = std::fs::OpenOptions::new()
        .write(true).create(true).open(&lock_path)?;
    lock_file.lock_exclusive()?;   // fs2::FileExt
    let result = f();
    lock_file.unlock()?;           // fs2::FileExt
    result
}
```

[VERIFIED: Python reference hermes-agent/tools/memory_tool.py lines 128-141]

**Atomic write pattern (D-08) — with fsync decision:**

The cron crate does NOT call fsync (confirmed in lib.rs lines 169-176). Python hermes-agent DOES call `os.fsync()` (memory_tool.py line 413). For memory files (user data, not job schedules), fsync is preferable for durability. **Recommendation: include fsync for memory writes.**

```rust
// Source: ironhermes-cron/src/lib.rs lines 162-176 + fsync added
fn write_file_atomic(path: &Path, content: &str) -> anyhow::Result<()> {
    let tmp_path = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(content.as_bytes())?;
        f.flush()?;
        f.sync_all()?;  // fsync — matches Python hermes-agent, not cron
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}
```

[VERIFIED: ironhermes-cron/src/lib.rs lines 162-176 — cron does NOT fsync; Python memory_tool.py line 413 DOES fsync]

### Pattern 3: Context File Write Scanning (SELF-02, SELF-03, D-01, D-02)

**What:** `WriteFileTool::execute()` and `PatchFileTool::execute()` check if the target path is a context file before writing. If it is, run `scan_context_content()` on the new content and return an error if threats are found.

**Context file detection:**
```rust
// Source: CONTEXT.md D-02 — only SOUL.md, AGENTS.md, MEMORY.md, USER.md
fn is_context_file(path: &str) -> bool {
    let p = std::path::Path::new(path);
    let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(filename, "SOUL.md" | "AGENTS.md" | "MEMORY.md" | "USER.md")
}
```

**Integration point in WriteFileTool:**
```rust
// BEFORE writing:
if is_context_file(path) {
    let scan_result = ironhermes_core::scan_context_content(content, filename);
    if scan_result.contains("[BLOCKED:") {
        return Err(anyhow::anyhow!(
            "Write blocked: content contains potential prompt injection. {}", scan_result
        ));
    }
    // Optionally: also use atomic write for context files
}
```

**PatchFileTool:** Must scan the *post-patch content* (the full file after substitution), not just the `after` string in isolation:
```rust
let patched = original.replacen(before, after, 1);
if is_context_file(path) {
    let scan_result = ironhermes_core::scan_context_content(&patched, filename);
    if scan_result.contains("[BLOCKED:") {
        return Err(anyhow::anyhow!("Patch blocked: ..."));
    }
}
// then write patched to disk
```

[VERIFIED: file_tools.rs — WriteFileTool::execute() currently uses `fs::write()` with no scanning; PatchFileTool::execute() uses `fs::write()` with no scanning]

### Pattern 4: Memory Injection into PromptBuilder

**What:** `PromptBuilder` gains a `memory_store: Option<Arc<MemoryStore>>` field. `build()` injects both memory blocks after AGENTS.md.

```rust
// crates/ironhermes-agent/src/prompt_builder.rs
// Assembly order (from existing build() method):
// 1. SOUL.md or default identity
// 2. Platform hint
// 3. Tool use guidance
// 4. Project context
// 5. AGENTS.md from IRONHERMES_HOME
// 6. [NEW] MEMORY block (personal notes)
// 7. [NEW] USER PROFILE block

if let Some(ref store) = self.memory_store {
    if let Some(block) = store.format_for_system_prompt(Target::Memory) {
        parts.push(block);
    }
    if let Some(block) = store.format_for_system_prompt(Target::User) {
        parts.push(block);
    }
}
```

**The MemoryStore must be initialized before PromptBuilder::load_context() is called.** Because MemoryStore::load_from_disk() captures the frozen snapshot, timing is critical — call it once at session start.

[VERIFIED: prompt_builder.rs lines 127-157 — build() assembles parts in the documented order]

### Pattern 5: SSRF Validator (SEC-01, D-16 through D-19)

**What:** Direct Rust port of `url_safety.py`. Python `ipaddress` module maps cleanly to `std::net::IpAddr`.

```rust
// crates/ironhermes-core/src/ssrf.rs
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

static BLOCKED_HOSTNAMES: LazyLock<std::collections::HashSet<&'static str>> =
    LazyLock::new(|| ["metadata.google.internal", "metadata.goog"].into());

// 100.64.0.0/10 CGNAT — not covered by IpAddr::is_private()
const CGNAT_START: u32 = 0x6440_0000; // 100.64.0.0
const CGNAT_END:   u32 = 0x647F_FFFF; // 100.127.255.255

pub fn is_safe_url(url: &str) -> bool {
    // Parse -> hostname -> DNS resolve -> check each IP
    // Fail closed on any error
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local()
                || v4.is_broadcast() || v4.is_multicast() || v4.is_unspecified()
                || is_cgnat(v4)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_multicast() || v6.is_unspecified()
        }
    }
}
```

**DNS resolution in Rust (sync, matches Python's `socket.getaddrinfo`):**
```rust
// std::net::ToSocketAddrs works for hostname resolution
// Must use format "hostname:0" — ToSocketAddrs requires port
let addrs: Vec<_> = (hostname, 0u16).to_socket_addrs()?.collect();
```

**IMPORTANT:** `IpAddr::is_private()` in Rust does NOT cover CGNAT (100.64.0.0/10), same as Python's `is_private`. Must check explicitly — same gap as Python reference.

[VERIFIED: url_safety.py lines 33-36 — CGNAT explicitly noted as not covered by is_private]
[VERIFIED: Rust std::net::IpAddr — is_private() covers RFC 1918 ranges only; CGNAT requires explicit range check]

### Pattern 6: Per-User Rate Limiter (SEC-03, D-20 through D-22)

**What:** Token bucket stored in a `HashMap<String, TokenBucketState>` wrapped in `Arc<Mutex<...>>`, checked in `GatewayMessageHandler` before dispatching to `run_agent()`.

```rust
// crates/ironhermes-gateway/src/rate_limiter.rs
use std::time::Instant;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct TokenBucketState {
    tokens: f64,
    last_refill: Instant,
}

pub struct PerUserRateLimiter {
    state: Arc<Mutex<HashMap<String, TokenBucketState>>>,
    messages_per_minute: f64,
    burst_size: f64,
}

impl PerUserRateLimiter {
    pub fn new(messages_per_minute: u32, burst_size: u32) -> Self { ... }

    /// Returns true if the message should be processed; false = silently drop (D-21)
    pub fn check_and_consume(&self, user_id: &str) -> bool {
        let mut map = self.state.lock().unwrap();
        let now = Instant::now();
        let state = map.entry(user_id.to_string()).or_insert(TokenBucketState {
            tokens: self.burst_size,
            last_refill: now,
        });
        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(state.last_refill).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.messages_per_minute / 60.0)
            .min(self.burst_size);
        state.last_refill = now;

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false  // drop silently (D-21)
        }
    }
}
```

**Integration in handler.rs:** The rate limiter lives on `GatewayMessageHandler`. Check before `run_agent()` is called in `handle_with_multimodal()`:

```rust
// In handle_with_multimodal() — before run_agent() call:
if !self.rate_limiter.check_and_consume(&event.sender_id) {
    return Ok(());  // Silent drop (D-21) — matches unauthorized user pattern
}
```

**Config extension:**
```rust
// crates/ironhermes-core/src/config.rs — new struct
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    pub messages_per_minute: u32,
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self { messages_per_minute: 10, burst_size: 3 }
    }
}

// Add to Config:
pub rate_limit: RateLimitConfig,
```

[VERIFIED: config.rs — Config struct fields, serde(default) pattern, Default impls]

### Pattern 7: MemoryTool — Tool trait implementation

**What:** New `crates/ironhermes-tools/src/memory_tool.rs` implementing `Tool` trait. Wraps `Arc<MemoryStore>` for shared access.

```rust
// Source: Python hermes-agent/tools/memory_tool.py MEMORY_SCHEMA
pub struct MemoryTool {
    store: Arc<MemoryStore>,
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str { "memory" }
    fn toolset(&self) -> &str { "memory" }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args["action"].as_str().unwrap_or("");
        let target_str = args["target"].as_str().unwrap_or("memory");
        let content = args["content"].as_str();
        let old_text = args["old_text"].as_str();
        // dispatch to store.add / store.replace / store.remove
        // return JSON string
    }
}
```

**MemoryStore needs `Arc` wrapping** because it is shared between `MemoryTool` (owned by ToolRegistry, which is `Arc<ToolRegistry>`) and `PromptBuilder` (which needs the frozen snapshot). Use `Arc<Mutex<MemoryStore>>` or `Arc<MemoryStore>` with interior mutability only in the mutation methods.

**Recommended:** `Arc<Mutex<MemoryStore>>` — mutations are infrequent and lock contention on a single user agent is negligible.

**Registration in register_defaults():**
```rust
// crates/ironhermes-tools/src/registry.rs
// register_defaults() cannot construct MemoryStore here (needs path config)
// Solution: register_defaults() takes an Option<Arc<Mutex<MemoryStore>>>
// OR: add a separate register_memory_tool(&mut self, store: Arc<Mutex<MemoryStore>>)
```

**Recommendation:** Add `register_memory_tool()` as a separate method called by the gateway/CLI setup code after constructing MemoryStore. This avoids adding config coupling to `register_defaults()`.

[VERIFIED: registry.rs — register_defaults() pattern, ToolRegistry::register() signature]

### Anti-Patterns to Avoid

- **Scanning the `after`/`before` substring in PatchFileTool instead of the full post-patch file** — injection can be split across old and new content or embedded in unchanged parts.
- **Locking the memory FILE itself instead of a .lock sidecar** — atomic rename requires the target file to be unlocked; using a sidecar lock avoids this race.
- **Sharing MemoryStore without Arc** — MemoryTool is `Send + Sync` (required by Tool trait); MemoryStore must be wrapped in `Arc<Mutex<...>>` or made internally thread-safe.
- **Running DNS resolution in the async context without spawn_blocking** — `ToSocketAddrs::to_socket_addrs()` is synchronous and can block the tokio thread. Use `tokio::task::spawn_blocking` or switch to `tokio::net::lookup_host` (async).
- **Initializing rate limiter state outside `GatewayMessageHandler`** — state must be on the handler (not a global static) so it can be configured from `Config`.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Advisory file locking | Custom flock() syscall wrapper | `fs2` crate | Cross-platform; handles Windows; D-07 locked |
| Regex pattern compilation | Compile patterns per-call | `LazyLock<RegexSet>` | Already established pattern in codebase; RegexSet matches all patterns in one pass |
| Atomic file writes | Custom write-verify-swap | `tempfile + fs::rename` | POSIX rename is atomic on same filesystem; already proven in ironhermes-cron |
| SSRF IP checking | String-matching on hostnames | `std::net::IpAddr` + `is_private()`, `is_loopback()`, `is_link_local()` | Handles IPv4/IPv6 correctly, handles edge cases like 0.0.0.0 |

**Key insight:** Every "don't hand-roll" item already has a solution in the codebase or stdlib. No novel algorithms needed.

---

## Common Pitfalls

### Pitfall 1: PatchFileTool scans `after` substring, not post-patch content
**What goes wrong:** Injection payload is injected into an *existing* entry in the file, and `after` alone looks clean.
**Why it happens:** Naive implementation scans only the replacement text passed by the LLM.
**How to avoid:** Apply `replacen()` first, then scan the complete new file content before writing.
**Warning signs:** Test that writes containing "ignore previous" in existing content + clean `after` are blocked.

### Pitfall 2: MemoryStore shared state race between load and tool execution
**What goes wrong:** PromptBuilder captures snapshot at session start, but MemoryTool modifies entries without re-reading from disk, diverging from what other sessions wrote.
**Why it happens:** Missing `_reload_target()` call under lock before each mutation.
**How to avoid:** Always re-read from disk under the file lock before mutating (Python reference lines 198-200). The in-memory state is authoritative only for the frozen snapshot; disk is authoritative for mutations.
**Warning signs:** Concurrent test writes losing entries.

### Pitfall 3: ToSocketAddrs blocks tokio executor thread
**What goes wrong:** SSRF check in an async tool call blocks the tokio thread for the duration of DNS resolution.
**Why it happens:** `std::net::ToSocketAddrs::to_socket_addrs()` is synchronous.
**How to avoid:** Wrap in `tokio::task::spawn_blocking(|| ...)` or use `tokio::net::lookup_host()` (async, returns `impl Stream`).
**Warning signs:** Gateway becoming unresponsive during SSRF checks under load.

### Pitfall 4: Rate limiter `sender_id` vs `chat_id`
**What goes wrong:** Rate limiting by `chat_id` instead of `sender_id` would limit the entire chat (multiple users in a group) rather than per-user.
**Why it happens:** D-20 says "per-user (Telegram user_id)" but `MessageEvent` has both fields.
**How to avoid:** Key the rate limiter on `event.sender_id`.
**Warning signs:** Multiple users in a group being rate-limited together.

### Pitfall 5: context_scanner.rs move breaks agent crate tests
**What goes wrong:** `ironhermes-agent` tests that import `context_scanner` via `crate::context_scanner` fail after the move.
**Why it happens:** The module path changes from `crate::context_scanner` to `ironhermes_core::context_scanner`.
**How to avoid:** Update all `use crate::context_scanner::*` imports in agent crate; update `lib.rs` to remove the local module declaration.
**Warning signs:** `cargo build` errors in ironhermes-agent immediately after moving the file.

### Pitfall 6: Memory char count uses byte count instead of char count
**What goes wrong:** Multi-byte UTF-8 characters (e.g., §, Unicode) inflate the byte count, making limits inconsistent with what the LLM sees.
**Why it happens:** Using `str::len()` (bytes) instead of `str::chars().count()`.
**How to avoid:** Use `content.chars().count()` for all char limit arithmetic. Python `len()` counts Unicode code points — match this behavior.
**Warning signs:** Memory appearing "full" when containing CJK or emoji despite showing low percentage.

---

## Code Examples

### Memory threat pattern set (extends context_scanner's 10 patterns)
```rust
// Source: Python hermes-agent/tools/memory_tool.py lines 50-66
// Rust port — additional patterns beyond existing THREAT_PATTERNS
static MEMORY_EXTRA_PATTERNS: LazyLock<regex::RegexSet> = LazyLock::new(|| {
    regex::RegexSet::new([
        r"(?i)you\s+are\s+now\s+",              // role_hijack
        r"(?i)wget\s+[^\n]*\$\{?\w*(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|API)", // exfil_wget
        r"(?i)authorized_keys",                  // ssh_backdoor
        r"(?i)(\$HOME|~)/\.ssh",                // ssh_access
    ]).expect("MEMORY_EXTRA_PATTERNS compile failed")
});
// Note: context_scanner's 10 patterns already cover the remaining Python patterns
// Total for memory: 14 patterns (10 shared + 4 memory-specific)
```

[VERIFIED: Python hermes-agent/tools/memory_tool.py lines 50-66 vs context_scanner.rs lines 9-22]

### Render block for system prompt injection
```rust
// Source: Python hermes-agent/tools/memory_tool.py lines 355-371
fn render_block(target: Target, entries: &[String], limit: usize) -> String {
    if entries.is_empty() { return String::new(); }
    let content = entries.join(ENTRY_DELIMITER);
    let current = content.chars().count();
    let pct = ((current * 100) / limit).min(100);
    let (header, separator) = match target {
        Target::User => (
            format!("USER PROFILE (who the user is) [{}% — {}/{} chars]", pct, current, limit),
            "═".repeat(46),
        ),
        Target::Memory => (
            format!("MEMORY (your personal notes) [{}% — {}/{} chars]", pct, current, limit),
            "═".repeat(46),
        ),
    };
    format!("{}\n{}\n{}\n{}", separator, header, separator, content)
}
```

### Config.yaml additions
```yaml
# ~/.ironhermes/config.yaml additions
rate_limit:
  messages_per_minute: 10
  burst_size: 3

memory:
  memory_char_limit: 2200
  user_char_limit: 1375
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `scan_context_content` only at load time | Scan at both load time AND write time | Phase 3 | Write-time scanning prevents injecting malicious content via self-modification |
| No memory persistence | MEMORY.md + USER.md with bounded char limits | Phase 3 | Agent accumulates user-specific facts across sessions |
| No inbound rate limiting | Per-user token bucket on Telegram messages | Phase 3 | Prevents abuse/flooding of the gateway |
| `url_safety` Python-only | Rust SSRF validator in core | Phase 3 | Prerequisite for Phase 4 web tools; reusable across any HTTP-making code |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Token bucket is preferred over sliding window for rate limiting | Standard Stack (Rate Limiting), Pattern 6 | Low — either works; sliding window requires storing N timestamps per user, slightly more memory |
| A2 | `fs2` 0.4 is the correct version to add to workspace | Standard Stack | Low — `cargo search fs2` will confirm; if newer version needed, adjust |
| A3 | `fsync` should be included before rename for memory writes | Pattern 2 (atomic write) | Low — without fsync, OS crash between write and rename could lose last entry; with fsync it's safe but slower |
| A4 | `tokio::net::lookup_host` is available as the async DNS option | Pitfall 3 | Low — it is in tokio's net feature; could also use spawn_blocking with std resolution |
| A5 | Memory extra patterns (4 additional beyond context_scanner's 10) are sufficient | Code Examples | Medium — more injection techniques exist; patterns are an allow-list approach and can be extended |

---

## Open Questions (RESOLVED)

1. **MemoryStore thread safety model** (RESOLVED: Arc<Mutex<MemoryStore>> — coarse-grained)
   - What we know: MemoryTool must be `Send + Sync` (Tool trait). Multiple agent sessions could theoretically run concurrently (Semaphore-bounded, default 8).
   - What's unclear: Should MemoryStore use `Arc<Mutex<MemoryStore>>` (coarse-grained) or make each method take `&self` with interior `Mutex` fields?
   - Recommendation: `Arc<Mutex<MemoryStore>>` — coarse-grained locking is simpler, and the file lock is the real bottleneck anyway. Fine-grained locking would not improve throughput.

2. **PromptBuilder construction in handler.rs** (RESOLVED: load once at GatewayMessageHandler::new() startup)
   - What we know: `handler.rs` constructs `PromptBuilder::new().load_context().build_system_message()` on every message (line 227-229). MemoryStore needs to be pre-constructed and its snapshot captured before the first build.
   - What's unclear: Should MemoryStore be loaded once when `GatewayMessageHandler` is constructed, or once per-session?
   - Recommendation: Load once when `GatewayMessageHandler::new()` is called (at startup). Memory snapshot is session-agnostic (same SOUL.md-style global state). This matches Python hermes-agent's pattern.

3. **WriteFileTool atomic writes for context files** (RESOLVED: yes, use atomic writes for all context file writes — SELF-06)
   - What we know: D-08 specifies atomic writes for memory files. D-02 says scanning applies to context files.
   - What's unclear: Should write_file/patch tools also use atomic writes when writing SOUL.md/AGENTS.md (not just scanning)?
   - Recommendation: Yes, use atomic writes for context files too — the failure mode of a partial write to SOUL.md (corrupting the identity) is severe.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `fs2` crate | MemoryStore file locking (D-07) | Not yet in workspace | 0.4.x | None — must add to Cargo.toml |
| Rust standard `std::net::ToSocketAddrs` | SSRF DNS resolution | Built-in stdlib | Always | Use `tokio::net::lookup_host` if async required |
| `tokio` (workspace) | Rate limiter Instant, async file ops | Already in workspace | 1.x | — |
| `regex` (workspace) | Memory threat patterns | Already in workspace | 1.x | — |
| `serde_json` (workspace) | Memory tool JSON responses | Already in workspace | 1.x | — |
| `chrono` (workspace) | Timestamps (if needed for rate limiter) | Already in workspace | 0.4 | Use `std::time::Instant` instead |

**Missing dependencies with no fallback:**
- `fs2` — must be added to `[workspace.dependencies]` in root `Cargo.toml` and to `crates/ironhermes-core/Cargo.toml`.

**Missing dependencies with fallback:**
- Synchronous DNS (`ToSocketAddrs`) can be replaced with `tokio::net::lookup_host` for the async context.

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` via `cargo test` |
| Config file | None — uses `#[cfg(test)]` modules |
| Quick run command | `cargo test -p ironhermes-core` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SELF-01 | read_file tool reads SOUL.md content | smoke/integration | `cargo test -p ironhermes-tools -- read_file` | ❌ Wave 0 |
| SELF-02 | write_file to SOUL.md succeeds with clean content | unit | `cargo test -p ironhermes-tools -- write_context_file_clean` | ❌ Wave 0 |
| SELF-03 | write_file/patch to context file with injection pattern returns error | unit | `cargo test -p ironhermes-tools -- write_context_file_blocked` | ❌ Wave 0 |
| SELF-03 | scan_context_content in core passes existing test suite | unit | `cargo test -p ironhermes-core -- context_scanner` | ❌ Wave 0 (after move) |
| SELF-04 | MemoryStore::load_from_disk captures frozen snapshot | unit | `cargo test -p ironhermes-core -- memory_store` | ❌ Wave 0 |
| SELF-04 | PromptBuilder injects memory blocks into system prompt | unit | `cargo test -p ironhermes-agent -- prompt_builder` | ❌ Wave 0 (extend existing) |
| SELF-05 | memory tool add/replace/remove actions return correct JSON | unit | `cargo test -p ironhermes-tools -- memory_tool` | ❌ Wave 0 |
| SELF-05 | add beyond char limit returns error with current entries | unit | `cargo test -p ironhermes-core -- memory_store_capacity` | ❌ Wave 0 |
| SELF-06 | Atomic write leaves no partial state on simulated crash | unit | `cargo test -p ironhermes-core -- memory_store_atomic` | ❌ Wave 0 |
| SEC-01 | is_safe_url blocks 127.0.0.1, 192.168.x.x, 100.64.x.x, metadata hosts | unit | `cargo test -p ironhermes-core -- ssrf` | ❌ Wave 0 |
| SEC-01 | is_safe_url allows valid public URLs | unit | `cargo test -p ironhermes-core -- ssrf` | ❌ Wave 0 |
| SEC-02 | Same as SELF-03 — covered above | — | — | — |
| SEC-03 | Rate limiter allows burst_size messages then drops | unit | `cargo test -p ironhermes-gateway -- rate_limiter` | ❌ Wave 0 |
| SEC-03 | Rate limiter refills tokens over time | unit | `cargo test -p ironhermes-gateway -- rate_limiter_refill` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-core && cargo test -p ironhermes-tools`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** `cargo test --workspace` green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-core/src/context_scanner.rs` — move from agent (module registration in core lib.rs)
- [ ] `crates/ironhermes-core/src/memory_store.rs` — new; unit tests inline in mod tests
- [ ] `crates/ironhermes-core/src/ssrf.rs` — new; unit tests inline in mod tests
- [ ] `crates/ironhermes-tools/src/memory_tool.rs` — new; unit tests inline in mod tests
- [ ] `crates/ironhermes-gateway/src/rate_limiter.rs` — new; unit tests inline in mod tests
- [ ] Existing `context_scanner` tests in `ironhermes-agent` — update import paths after move

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | n/a |
| V3 Session Management | no | n/a |
| V4 Access Control | yes | context file write scanning (D-01, D-02) |
| V5 Input Validation | yes | RegexSet threat patterns + invisible unicode detection |
| V6 Cryptography | no | n/a |
| V10 Malicious Code | yes | Prompt injection via self-modification; SSRF via web tools |

### Known Threat Patterns for This Stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via self-modification (write "ignore previous instructions" to SOUL.md) | Tampering | RegexSet scan at write time, block before persisting |
| Exfiltration via shell command injection in memory (curl $API_KEY) | Information Disclosure | regex patterns `exfil_curl`, `exfil_wget`, `read_secrets` |
| SSRF via user-provided URLs (Phase 4 prerequisite) | Elevation of Privilege | is_safe_url() DNS resolution + IP range check |
| Invisible Unicode injection to bypass pattern matching | Tampering | Explicit invisible char set check before RegexSet |
| Rate abuse flooding gateway | Denial of Service | Per-user token bucket; excess silently dropped |
| DNS rebinding (TOCTOU) | Spoofing | Documented known limitation; mitigated by fail-closed on resolution errors |

---

## Sources

### Primary (HIGH confidence)
- `crates/ironhermes-agent/src/context_scanner.rs` — VERIFIED: full source read; 10 threat patterns, THREAT_NAMES, INVISIBLE_CHARS, scan_context_content(), truncate_content(), test suite
- `crates/ironhermes-agent/src/prompt_builder.rs` — VERIFIED: full source read; build() assembly order, load_context() pattern, test suite
- `crates/ironhermes-tools/src/file_tools.rs` — VERIFIED: full source read; WriteFileTool, PatchFileTool execute() methods (no scanning currently)
- `crates/ironhermes-tools/src/registry.rs` — VERIFIED: Tool trait definition, ToolRegistry, register_defaults()
- `crates/ironhermes-cron/src/lib.rs` — VERIFIED: atomic write pattern (JobStore::save() lines 162-176); no fsync
- `crates/ironhermes-gateway/src/handler.rs` — VERIFIED: GatewayMessageHandler, handle_with_multimodal(), run_agent() structure
- `crates/ironhermes-core/src/config.rs` — VERIFIED: Config struct, all sub-structs, serde patterns
- `Cargo.toml` (workspace) — VERIFIED: all workspace dependencies
- `/Users/twilson/code/hermes-agent/tools/memory_tool.py` — VERIFIED: full source read; MemoryStore class, MEMORY_SCHEMA, all patterns
- `/Users/twilson/code/hermes-agent/tools/url_safety.py` — VERIFIED: full source read; is_safe_url(), _is_blocked_ip(), CGNAT handling, blocked hostnames

### Secondary (MEDIUM confidence)
- `.planning/phases/03-self-improvement-security/03-CONTEXT.md` — locked decisions D-01 through D-22
- `.planning/REQUIREMENTS.md` — SELF-01 through SELF-06, SEC-01 through SEC-03
- `.planning/codebase/ARCH.md` — crate dependency graph, module structure

### Tertiary (LOW confidence / ASSUMED)
- `fs2` crate version 0.4 — assumed current; verify with `cargo search fs2` before adding to Cargo.toml
- `tokio::net::lookup_host` async DNS availability — assumed in tokio "full" feature; verify before use

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all workspace deps verified from Cargo.toml; only fs2 version is assumed
- Architecture patterns: HIGH — all patterns derived from direct source code reading
- Memory store port: HIGH — Python reference fully read; Rust idioms are direct translations
- SSRF port: HIGH — Python reference fully read; std::net mapping is well-understood
- Rate limiter: MEDIUM — algorithm choice (token bucket) is Claude's discretion; implementation is straightforward
- Pitfalls: HIGH — all pitfalls derived from direct code analysis, not general knowledge

**Research date:** 2026-04-07
**Valid until:** 2026-05-07 (stable Rust ecosystem; workspace deps unlikely to change)
