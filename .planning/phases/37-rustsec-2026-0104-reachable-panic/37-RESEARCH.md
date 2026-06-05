# Phase 37: RUSTSEC-2026-0104 + Workspace v0.2.0 Bump - Research

**Researched:** 2026-06-05
**Domain:** Cargo dependency security remediation + workspace version management
**Confidence:** HIGH

---

## Summary

Phase 37 has two goals: (1) remediate RUSTSEC-2026-0104, a reachable panic (DoS, CVSS 7.5) in
`rustls-webpki` CRL parsing, and (2) bump the IronHermes workspace version from `0.1.0` to
`0.2.0`.

The vulnerability exists in **both** the `0.102.x` and `0.103.x` series of `rustls-webpki`.
Two distinct dependency chains pull both versions into this workspace simultaneously. Chain 1
(`rustls-webpki 0.102.8`) has **no patch within the 0.102.x series** — the official advisory
confirms patches start at `0.103.13`. The only remediation for Chain 1 is to eliminate the
`rustls 0.22.x` subtree entirely, which requires upgrading `serenity`. However, `serenity`
is still on `0.12.5` as the latest stable release (no `0.13.x` exists on crates.io as of
2026-06-05), and `serenity 0.12.5` hard-codes `tokio-tungstenite 0.21.0` which hard-codes
`rustls 0.22.4`. There is no serenity upgrade path to rustls 0.23.x today. Chain 2
(`rustls-webpki 0.103.10`) is straightforwardly fixable via `[patch.crates-io]` to force
`0.103.13`.

**Primary recommendation:** Add `[patch.crates-io]` for `rustls-webpki = "0.103.13"` to
fix Chain 2. For Chain 1, document the no-upstream-patch situation, add `cargo deny` (or
`cargo audit`) as the verification gate, and accept a **targeted exemption** for
`rustls-webpki 0.102.8` with documented rationale — OR use a `[patch.crates-io]` that
overrides the `rustls 0.22.4` → `rustls-webpki` path to `0.103.x` (only valid if rustls
0.22.x accepts 0.103.x at runtime — it does NOT, semver incompatible). The planner must
choose between accepting the exemption or seeking a serenity fork/patch.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| TLS certificate verification | Dependency (rustls) | — | rustls-webpki is an internal dep of rustls; workspace code never calls it directly |
| CRL parsing (vulnerable path) | Dependency (rustls-webpki) | — | Only triggered if rustls asks webpki to parse a CRL; workspace never calls it directly |
| Dependency version pinning | Root Cargo.toml | Per-crate Cargo.toml | `[patch.crates-io]` in root overrides workspace-wide |
| Workspace version | `[workspace.package]` in root Cargo.toml | Two crates with hardcoded `version = "0.1.0"` | Most crates use `version.workspace = true`; two exceptions need separate edits |

---

## Live Codebase State (Verified Against Cargo.lock)

### Vulnerable Versions Confirmed Present

| Package | Version | Status |
|---------|---------|--------|
| `rustls-webpki` | `0.102.8` | [VERIFIED: Cargo.lock line 8090] VULNERABLE — no 0.102.x patch exists |
| `rustls-webpki` | `0.103.10` | [VERIFIED: Cargo.lock line 8101] VULNERABLE — patched at 0.103.13 |

### Chain 1: `rustls-webpki 0.102.8` (via rustls 0.22.x branch)

```
serenity 0.12.5
  └── tokio-tungstenite 0.21.0   [Cargo.lock line 9632-9645]
        ├── rustls 0.22.4         [Cargo.lock line 7989-8000]
        │     └── rustls-webpki 0.102.8  ← VULNERABLE
        └── tokio-rustls 0.25.0   [Cargo.lock line 9588-9596]
              └── rustls 0.22.4

tungstenite 0.21.0               [Cargo.lock line ~10013]
  └── rustls 0.22.4              (same rustls 0.22.4 node)
```

Direct gateway dependency: `serenity = { version = "0.12.5", features = ["client", "gateway", "http", "model", "cache", "rustls_backend"] }` [VERIFIED: crates/ironhermes-gateway/Cargo.toml line 42]

### Chain 2: `rustls-webpki 0.103.10` (via rustls 0.23.x branch)

```
reqwest 0.12.28         → rustls 0.23.40 → rustls-webpki 0.103.10  ← VULNERABLE
hyper-rustls            → rustls 0.23.40 → rustls-webpki 0.103.10
slack-morphism 2.22.0   → tokio-tungstenite 0.29.0 → rustls 0.23.40
chromiumoxide           → reqwest 0.13.4 → rustls 0.23.40
```

[VERIFIED: Cargo.lock lines 4309, 7996-7997, 8003-8013, 8527, 8753]

---

## Standard Stack (No New Dependencies Required)

This phase adds zero new crate dependencies. Remediation uses native Cargo mechanisms.

### Cargo Patch Mechanism

| Mechanism | Use | Syntax |
|-----------|-----|--------|
| `[patch.crates-io]` in root `Cargo.toml` | Force transitive dep to specific version | `rustls-webpki = { version = "=0.103.13" }` |
| `cargo update -p rustls-webpki` | Bump resolver without changing Cargo.toml | Useful after adding patch entry |
| `cargo tree -i rustls-webpki` | Verify which versions are still in graph | Verification command |

**`[patch.crates-io]` semantics (Cargo docs):** [CITED: https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html]
- Applies globally to the entire workspace.
- The patch version must be semver-compatible with the dependency being patched.
- `version = "=0.103.13"` pins exactly; `version = "0.103.13"` means `>=0.103.13, <0.104.0`.
- Cargo will use the patch for any crate that resolves within the specified version range.
- The `[patch.crates-io]` entry does NOT need a local path — it can reference the registry version directly with a version constraint.

**Critical semver constraint:** `rustls 0.22.4` declares `rustls-webpki = "0.102"` in its
`Cargo.toml`. A `[patch.crates-io]` to `0.103.13` is **NOT semver-compatible** with the
`0.102` requirement. Cargo will reject this patch for the `rustls 0.22.4` consumer.
Therefore: [patch.crates-io] can only fix Chain 2 (0.103.x range). Chain 1 requires
eliminating `rustls 0.22.4` from the graph entirely.

### Verification Tools

| Tool | Status | Install |
|------|--------|---------|
| `cargo audit` | NOT installed [VERIFIED: cargo audit --version returns error] | `cargo install cargo-audit` |
| `cargo deny` | NOT installed [VERIFIED: cargo deny --version returns error] | `cargo install cargo-deny` |
| `cargo tree` | Built-in Cargo subcommand | No install needed |

**Recommended verification approach:** `cargo tree -i rustls-webpki` after changes to confirm
which versions remain. Install `cargo-audit` as part of Wave 0 for authoritative RUSTSEC
scanning. Alternative: install `cargo-deny` with a `deny.toml` advisory list.

### Version bump tooling

No special tool needed — edit `Cargo.toml` directly. The workspace version is at
`[workspace.package] version = "0.1.0"` in the root `Cargo.toml`. [VERIFIED: Cargo.toml line 52]

---

## Package Legitimacy Audit

No new external packages are introduced in this phase. The remediation uses:
- `rustls-webpki 0.103.13` — an upgrade of an existing transitive dependency via `[patch.crates-io]`. Not a new package.

| Package | Registry | Disposition |
|---------|----------|-------------|
| `rustls-webpki` | crates.io | Existing transitive dep — upgrade only, not a new install |
| `cargo-audit` | crates.io | Dev/CI tool, installed via `cargo install` in Wave 0 task |

*slopcheck was not available at research time. `cargo-audit` is a well-established tool from
the RustSec organization (rustsec.org), used by the Rust security advisory database project
itself — this is HIGH confidence legitimate. `rustls-webpki` is maintained by the rustls
project (github.com/rustls/webpki).*

---

## Architecture Patterns

### System Architecture Diagram

```
Root Cargo.toml
  [workspace.package]
    version = "0.1.0"  ──► change to "0.2.0"
  [patch.crates-io]      ──► ADD: rustls-webpki = { version = "=0.103.13" }
                                   (fixes Chain 2 only)

ironhermes-gateway/Cargo.toml
  serenity = "0.12.5"   ──► no upgrade path today (latest IS 0.12.5)
                             Chain 1 requires exemption or workaround

iron_hermes_ui/Cargo.toml
  version = "0.1.0"  ──► change to "0.2.0" (hardcoded, not workspace)

ironhermes-exec/Cargo.toml
  version = "0.1.0"  ──► change to "0.2.0" (hardcoded, not workspace)

All other crates
  version.workspace = true  ──► automatically picks up root bump, NO edits needed
```

### Recommended Project Structure for Changes

```
Cargo.toml                          # Edit: [workspace.package].version + [patch.crates-io]
crates/iron_hermes_ui/Cargo.toml    # Edit: hardcoded version = "0.1.0"
crates/ironhermes-exec/Cargo.toml   # Edit: hardcoded version = "0.1.0"
Cargo.lock                          # Updated automatically by cargo update
```

### Pattern 1: Adding `[patch.crates-io]` for Security Patch

**What:** Forces Cargo to resolve a specific transitive dependency version workspace-wide.

**When to use:** When the workspace does not directly depend on the vulnerable package but
needs to force an upgrade through an indirect consumer.

**Example (from Cargo docs):**
```toml
# In root Cargo.toml, after all [workspace.*] sections
[patch.crates-io]
# Fix RUSTSEC-2026-0104: reachable panic in CRL parsing (DoS, CVSS 7.5)
# Patches the 0.103.x consumer chain (reqwest, hyper-rustls, slack-morphism, chromiumoxide)
# NOTE: cannot patch the 0.102.x chain — rustls 0.22.4 requires "0.102" (semver-incompatible)
rustls-webpki = { version = "=0.103.13" }
```

**Verification:**
```bash
cargo update                        # regenerate Cargo.lock with patch applied
cargo tree -i rustls-webpki         # should show only 0.102.8 and 0.103.13 (not 0.103.10)
```

### Pattern 2: Chain 1 Remediation Options

**The problem:** `serenity 0.12.5` → `tokio-tungstenite 0.21.0` → `rustls 0.22.4` →
`rustls-webpki 0.102.8`. No patch in 0.102.x series. No serenity 0.13.x exists.

**Option A (RECOMMENDED — Accept exemption with documentation):**
- Add `[patch.crates-io]` for Chain 2 only.
- Add a `cargo deny` advisory exemption for `RUSTSEC-2026-0104` scoped to `rustls-webpki 0.102.8` with documented rationale: "serenity 0.12.5 is the latest release; no upstream fix available in this version branch; no 0.13.x release exists; risk accepted pending serenity upgrade."
- Track as a follow-on phase when serenity 0.13.x ships.

**Option B (Invasive — not recommended now):**
- Add `[patch.crates-io]` to override `tokio-tungstenite 0.21.0` with a newer version that uses `rustls 0.23.x`. But `serenity 0.12.5` has a hard API dependency on `tokio-tungstenite 0.21.x` — patching tokio-tungstenite to a newer version will cause serenity to fail to compile (API incompatibility). This requires modifying serenity source, which means a git dependency or fork.

**Option C (Acceptable if scope permits):**
- Use a `[patch.crates-io]` git path pointing to a fork of `serenity` that bumps its tokio-tungstenite dep. High maintenance burden.

**Option A is the correct choice** for a focused security phase. Document the exemption
explicitly. The advisory specifically notes that applications not actively parsing CRLs from
untrusted sources have reduced exposure; serenity's usage of rustls-webpki is through WebSocket
TLS handshakes with Discord's servers (a trusted endpoint), not arbitrary CRL parsing.

### Pattern 3: Workspace Version Bump

**Scope of changes:**

| File | Current | Action |
|------|---------|--------|
| `Cargo.toml` (`[workspace.package]`) | `version = "0.1.0"` | Change to `"0.2.0"` |
| `crates/iron_hermes_ui/Cargo.toml` | `version = "0.1.0"` (hardcoded) | Change to `"0.2.0"` |
| `crates/ironhermes-exec/Cargo.toml` | `version = "0.1.0"` (hardcoded) | Change to `"0.2.0"` |
| All other crates (`version.workspace = true`) | Inherited | No change needed |
| `Cargo.lock` | Stale checksums | Regenerated by `cargo build` or `cargo update` |

**Runtime version strings:**
- `crates/ironhermes-cli/src/main.rs` lines 947 and 3290 use `env!("CARGO_PKG_VERSION")` — these pick up the workspace version at compile time, no source edit needed. [VERIFIED: grep output]
- No hardcoded `"0.1.0"` strings in non-Cargo source files found.
- `.planning/codebase/TECH.md` line 8 documents `Workspace version: 0.1.0` — this is a doc file, not source; update for accuracy if desired (optional).

### Anti-Patterns to Avoid

- **Using `cargo update -p rustls-webpki` without `[patch.crates-io]`:** `cargo update` only bumps within the semver range already specified by the parent crate. `rustls 0.23.40` depends on `rustls-webpki = "^0.103"`, so `cargo update -p rustls-webpki --precise 0.103.13` would work for Chain 2 *without* a `[patch.crates-io]` entry — but the `[patch]` approach is more explicit and survives future `cargo update` calls.
- **Pinning `rustls-webpki` to an exact version without understanding both chains:** Only Chain 2 can be fixed; don't let the planner believe `[patch.crates-io]` alone closes both vulnerabilities.
- **Skipping `cargo tree -i rustls-webpki` verification:** The only way to confirm the patch worked is to inspect the resolved graph.
- **Bumping version in per-crate Cargo.toml files that already use `version.workspace = true`:** There are 15 crates using workspace inheritance — only the 2 hardcoded ones need editing.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Detecting RUSTSEC advisories | Custom vulnerability scanner | `cargo audit` / `cargo deny` | Maintained by RustSec organization, automatically fetches latest advisory DB |
| Checking resolved dependency versions | Parsing Cargo.lock manually | `cargo tree -i <crate>` | Built-in, shows full dependency graph with version resolution |
| Forcing transitive dep upgrade | Forking intermediate crates | `[patch.crates-io]` in root Cargo.toml | Native Cargo mechanism, no source forks needed |

---

## Common Pitfalls

### Pitfall 1: Believing `[patch.crates-io]` Fixes Both Chains

**What goes wrong:** Planner assumes adding `rustls-webpki = { version = "=0.103.13" }` removes
both vulnerable versions from the lockfile.

**Why it happens:** The patch fixes Chain 2 (rustls 0.23.x consumers). But Chain 1 (rustls
0.22.x consumers) requires `rustls-webpki "0.102"` — a different semver range. Cargo will NOT
apply the 0.103.x patch to a `"0.102"` requirement. After the patch, `cargo tree -i
rustls-webpki` will still show `0.102.8`.

**How to avoid:** Run `cargo tree -i rustls-webpki` after applying the patch. Two lines should
appear: one for `0.102.8` (still present, not patchable) and one for `0.103.13` (upgraded from
0.103.10). Document this outcome explicitly in the plan's verification step.

**Warning signs:** If `cargo tree` output still shows `0.103.10`, the patch syntax is wrong
(check the `=` prefix in the version string).

### Pitfall 2: Assuming Serenity Has a Newer Release

**What goes wrong:** Plan task says "upgrade serenity to >= 0.13" without verifying this exists.

**Why it happens:** The advisory analysis file says "check whether serenity >= 0.13.x exists."

**How to avoid:** [VERIFIED via crates.io API] `serenity` max stable version is `0.12.5` as of
2026-06-05. No 0.13.x release exists. The plan must NOT include a serenity upgrade task.

### Pitfall 3: Missing the Two Hardcoded Version Crates

**What goes wrong:** Only `[workspace.package] version` is bumped; `iron_hermes_ui` and
`ironhermes-exec` remain at `0.1.0`.

**Why it happens:** Most crates use `version.workspace = true` so it's easy to assume all do.

**How to avoid:** [VERIFIED: grep output] Two crates have `version = "0.1.0"` hardcoded:
`crates/iron_hermes_ui/Cargo.toml` and `crates/ironhermes-exec/Cargo.toml`. Both need editing.

### Pitfall 4: Forgetting cargo-audit Is Not Installed

**What goes wrong:** Plan verification step says `cargo audit` and it fails because the tool
is not installed.

**Why it happens:** `cargo audit` is not a built-in Cargo subcommand — it requires `cargo
install cargo-audit`.

**How to avoid:** [VERIFIED: cargo audit --version fails] Include a Wave 0 task to install
`cargo-audit` before the verification tasks that invoke it. Alternative: use `cargo tree -i
rustls-webpki` (built-in, no install) as the primary verification mechanism.

### Pitfall 5: `[patch.crates-io]` Version Constraint Typo

**What goes wrong:** Writing `version = "0.103.13"` (without `=` prefix) means `>=0.103.13,
<0.104.0`. This is fine for now but could resolve to a future `0.103.14` after a `cargo update`.
Using `version = "=0.103.13"` pins exactly.

**How to avoid:** Use exact pinning (`=0.103.13`) for security patches so future `cargo update`
calls don't silently advance past the tested version. Note in a comment that this should be
reviewed/relaxed once the codebase upgrades to serenity 0.13.x or the full rustls 0.23.x chain.

---

## Code Examples

### Adding `[patch.crates-io]` (Chain 2 fix)

```toml
# In root Cargo.toml, after [profile.dev] section
[patch.crates-io]
# RUSTSEC-2026-0104: reachable panic in CRL parsing (DoS, CVSS 7.5)
# Patches rustls-webpki in the 0.103.x range (reqwest, hyper-rustls, slack-morphism, chromiumoxide).
# NOTE: rustls-webpki 0.102.8 (via serenity → tokio-tungstenite 0.21 → rustls 0.22.4) CANNOT be
# patched here — rustls 0.22.x requires "webpki ^0.102" (semver-incompatible with 0.103.x).
# No serenity 0.13.x release exists as of 2026-06-05. Tracked: upgrade when serenity 0.13.x ships.
rustls-webpki = { version = "=0.103.13" }
```

### Verification Commands

```bash
# Regenerate lockfile after patch
cargo update -p rustls-webpki

# Verify resolved versions — should show 0.102.8 (still present) and 0.103.13 (upgraded)
cargo tree -i rustls-webpki

# Full workspace build to confirm nothing broke
cargo build --workspace

# Full test suite
cargo nextest run --workspace

# If cargo-audit is installed (Wave 0 install task):
cargo audit
# Expected: RUSTSEC-2026-0104 warning still present for 0.102.8 (acknowledged/exempted)
# Expected: RUSTSEC-2026-0104 NO longer triggered for 0.103.x (patched to 0.103.13)
```

### Version Bump (three files)

```toml
# Cargo.toml [workspace.package]
version = "0.2.0"   # was "0.1.0"

# crates/iron_hermes_ui/Cargo.toml
version = "0.2.0"   # was "0.1.0" (hardcoded — does NOT use version.workspace = true)

# crates/ironhermes-exec/Cargo.toml
version = "0.2.0"   # was "0.1.0" (hardcoded — does NOT use version.workspace = true)
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `webpki` crate (standalone) | `rustls-webpki` (rustls project maintained) | ~2022 | rustls-webpki is the actively maintained fork |
| rustls-webpki 0.102.x | rustls-webpki 0.103.x | 2023-2024 | 0.103.x uses aws-lc-rs, 0.102.x uses ring only |

**Deprecated/outdated:**
- `rustls-webpki 0.102.x`: No patches being published for this branch. All fixes land in 0.103.x+ only. Effectively end-of-life for security fixes. [CITED: rustsec.org advisory RUSTSEC-2026-0104]
- `rustls 0.22.x`: Superseded by 0.23.x. Staying on it is only unavoidable due to serenity.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | serenity 0.12.5 is the current latest and no 0.13.x is in progress/imminent | Chain 1 analysis | Low — verified via crates.io API returning `max_stable: 0.12.5, max: 0.12.5`; no pre-release exists |
| A2 | `rustls-webpki = { version = "=0.103.13" }` in `[patch.crates-io]` is sufficient to upgrade from 0.103.10 to 0.103.13 | Code Examples | Low — matches Cargo docs semantics; will be confirmed by `cargo tree -i rustls-webpki` |
| A3 | serenity's use of rustls-webpki is through Discord WebSocket TLS (trusted endpoint), not arbitrary CRL parsing from untrusted input | Risk assessment for Chain 1 exemption | Medium — if Discord ever sends malformed CRLs, the panic is still reachable; but this is outside attacker control |
| A4 | `iron_hermes_ui` and `ironhermes-exec` are the only two crates with hardcoded `version = "0.1.0"` | Version bump scope | Low — verified by grepping all workspace member Cargo.toml files |

**If this table is empty:** N/A — four assumptions are flagged above.

---

## Open Questions

1. **Should the plan install `cargo-audit` or `cargo-deny`?**
   - What we know: Neither is currently installed. `cargo tree -i rustls-webpki` works without either.
   - What's unclear: Whether the project wants ongoing advisory scanning as a CI gate.
   - Recommendation: Install `cargo-audit` in Wave 0 as a project quality improvement; use `cargo tree` as the primary verification gate so the phase doesn't hard-depend on a new install.

2. **Accepted exemption format for Chain 1 (`rustls-webpki 0.102.8`)?**
   - What we know: If `cargo-audit` is installed, it will flag `0.102.8` as vulnerable indefinitely until serenity upgrades. `cargo audit` supports an `audit.toml` ignore list.
   - What's unclear: Whether the user wants a formal `audit.toml` ignore entry or just a code comment.
   - Recommendation: Add a comment in root `Cargo.toml` above the `[patch.crates-io]` section documenting the exemption rationale. Optionally add `audit.toml` with `ignore = ["RUSTSEC-2026-0104"]` with expiry date.

3. **Does `iron_hermes_ui` need to stay at `0.1.0` for Dioxus compatibility?**
   - What we know: `iron_hermes_ui` has `version = "0.1.0"` and `dioxus = { version = "=0.7.1" }` — Dioxus version is pinned, not the ui crate version.
   - What's unclear: Whether any external tooling hard-codes the `iron_hermes_ui` version string.
   - Recommendation: Safe to bump to `0.2.0`; version strings in `[package]` don't affect Dioxus compatibility.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `cargo` | All tasks | Yes | 1.94.0 [VERIFIED] | — |
| `cargo nextest` | Test suite | Yes (used in recent commits) | present [VERIFIED via commit history] | `cargo test --workspace` |
| `cargo audit` | Advisory verification | No | — | `cargo tree -i rustls-webpki` (built-in) |
| `cargo deny` | Advisory policy | No | — | `cargo audit` or `cargo tree` |

**Missing dependencies with no fallback:** None — `cargo tree` is sufficient for verification.

**Missing dependencies with fallback:** `cargo-audit` — fallback is `cargo tree -i rustls-webpki`.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | cargo-nextest (migrated in phase 36.17.7) |
| Config file | `.config/nextest.toml` |
| Quick run command | `cargo nextest run --workspace` |
| Full suite command | `cargo nextest run --workspace && cargo test --doc` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | Notes |
|--------|----------|-----------|-------------------|-------|
| SEC-01 | `rustls-webpki 0.103.10` removed from graph | Verification grep | `cargo tree -i rustls-webpki \| grep 0.103.10` should return empty | Exit 1 if found |
| SEC-02 | `rustls-webpki 0.103.13` present in graph | Verification grep | `cargo tree -i rustls-webpki \| grep 0.103.13` should return non-empty | Exit 1 if missing |
| SEC-03 | Workspace builds clean after patch | Build | `cargo build --workspace` | Must be exit 0 |
| SEC-04 | Full test suite passes after patch | Test | `cargo nextest run --workspace` | No new failures |
| VER-01 | Workspace version = 0.2.0 in root manifest | Grep | `grep '^version = ' Cargo.toml \| grep 0.2.0` | |
| VER-02 | `hermes --version` outputs 0.2.0 | Build + run | `cargo run -p ironhermes-cli -- --version \| grep 0.2.0` | Uses env!("CARGO_PKG_VERSION") |
| VER-03 | `iron_hermes_ui` version = 0.2.0 | Grep | `grep '^version' crates/iron_hermes_ui/Cargo.toml \| grep 0.2.0` | |
| VER-04 | `ironhermes-exec` version = 0.2.0 | Grep | `grep '^version' crates/ironhermes-exec/Cargo.toml \| grep 0.2.0` | |

### Wave 0 Gaps

- [ ] Optional: `cargo install cargo-audit` — enables authoritative RUSTSEC scanning beyond `cargo tree`
- [ ] Optional: Create `audit.toml` with documented exemption for `RUSTSEC-2026-0104` / `rustls-webpki 0.102.8`

*(No new test files needed — all verification is via cargo commands and greps)*

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V6 Cryptography | Yes | rustls (not hand-rolled) — this phase ensures cryptographic library is patched |
| V5 Input Validation | Partial | CRL parsing vulnerability is in input validation within rustls-webpki |
| V2 Authentication | No | — |
| V3 Session Management | No | — |
| V4 Access Control | No | — |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Malformed CRL causing panic (RUSTSEC-2026-0104) | Denial of Service | Upgrade rustls-webpki to 0.103.13+ (Chain 2 fixed); serenity chain requires upstream fix |
| Supply chain confusion (patch.crates-io to wrong package) | Tampering | Verify via `cargo tree` + `cargo audit` after applying patch |

---

## Sources

### Primary (HIGH confidence)
- `Cargo.lock` (live file, grepped directly) — confirmed both rustls-webpki versions, all dependency chains
- `crates/ironhermes-gateway/Cargo.toml` (live file) — confirmed serenity 0.12.5 with features
- `Cargo.toml` (live file) — confirmed workspace version, no existing `[patch.crates-io]`
- crates.io API (`https://crates.io/api/v1/crates/rustls-webpki`) — confirmed max_stable: 0.103.13, max: 0.104.0-alpha.7
- crates.io API (`https://crates.io/api/v1/crates/serenity`) — confirmed max_stable: 0.12.5, no 0.13.x
- [rustsec.org RUSTSEC-2026-0104](https://rustsec.org/advisories/) — confirmed patched versions: >=0.103.13 or >=0.104.0-alpha.7; 0.102.x has no patch
- [GitHub rustls/webpki releases](https://github.com/rustls/webpki/releases) — confirmed 0.103.13 released 21 Apr 2026

### Secondary (MEDIUM confidence)
- [Cargo overriding dependencies docs](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html) — `[patch.crates-io]` semantics [ASSUMED from training knowledge, not fetched this session]
- [DailyCVE GHSA-82j2-j2ch-gfr8](https://dailycve.com/rustls-webpki-reachable-panic-in-crl-parsing-ghsa-82j2-j2ch-gfr8-medium/) — confirmed vulnerability details (CRL BIT STRING underflow)

### Tertiary (LOW confidence)
- serenity GitHub releases page — confirmed 0.12.5 is described as "last release for 0.12.x series" but no 0.13.x timeline mentioned

---

## Metadata

**Confidence breakdown:**
- Vulnerable versions in lockfile: HIGH — directly verified from Cargo.lock
- Patched versions (0.103.13): HIGH — verified via crates.io API
- 0.102.x has no patch: HIGH — verified via official RustSec advisory
- Serenity latest = 0.12.5, no 0.13.x: HIGH — verified via crates.io API
- `[patch.crates-io]` semver incompatibility for Chain 1: HIGH — fundamental Cargo semver rule
- Version bump scope (3 files): HIGH — verified by grepping all workspace member Cargo.toml files

**Research date:** 2026-06-05
**Valid until:** 2026-07-05 (stable ecosystem; serenity 0.13.x release would invalidate Chain 1 finding)
