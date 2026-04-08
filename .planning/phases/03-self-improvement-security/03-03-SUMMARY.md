---
phase: 03-self-improvement-security
plan: 03
subsystem: security
tags: [ssrf, rate-limiting, security, gateway]
dependency_graph:
  requires: [03-01]
  provides: [ssrf-validator, rate-limiter]
  affects: [ironhermes-core, ironhermes-gateway]
tech_stack:
  added: [url-crate]
  patterns: [token-bucket, fail-closed-validation]
key_files:
  created:
    - crates/ironhermes-core/src/ssrf.rs
    - crates/ironhermes-gateway/src/rate_limiter.rs
  modified:
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/Cargo.toml
    - crates/ironhermes-gateway/src/lib.rs
    - crates/ironhermes-gateway/src/handler.rs
    - Cargo.toml
decisions:
  - "DNS-dependent SSRF tests marked #[ignore] for CI reliability"
  - "Rate limiter uses sync Mutex (sufficient for single-threaded check_and_consume under tokio lock)"
  - "Token bucket refill test uses high rate (600/min) with 150ms sleep for deterministic timing"
metrics:
  duration: 250s
  completed: 2026-04-08T02:13:56Z
  tasks: 2/2
  files: 8
requirements: [SEC-01, SEC-03]
---

# Phase 03 Plan 03: SSRF Validation and Rate Limiting Summary

SSRF URL validator ported from Python url_safety.py blocking private IPs, loopback, link-local, CGNAT, and metadata hostnames with fail-closed semantics; per-user token bucket rate limiter in gateway silently dropping excess messages at 10 msg/min with burst of 3.

## Task Results

| Task | Name | Commit | Tests |
|------|------|--------|-------|
| 1 | SSRF validator in ironhermes-core | 9c8e6b6 | 16 pass, 3 ignored (DNS) |
| 2 | Per-user rate limiter in gateway | a2bc603 | 5 pass |

## Implementation Details

### Task 1: SSRF Validator

Created `crates/ironhermes-core/src/ssrf.rs` with:
- `is_safe_url(url_str: &str) -> bool` -- public entry point
- `is_blocked_ip(ip: IpAddr) -> bool` -- checks private, loopback, link-local, broadcast, multicast, unspecified, CGNAT
- `is_cgnat(ip: Ipv4Addr) -> bool` -- checks 100.64.0.0/10 range
- `BLOCKED_HOSTNAMES` -- metadata.google.internal, metadata.goog
- Fail-closed on parse errors, missing hosts, DNS resolution failures
- Doc comments noting DNS rebinding TOCTOU limitation (D-17) and async caller guidance

### Task 2: Per-User Rate Limiter

Created `crates/ironhermes-gateway/src/rate_limiter.rs` with:
- `PerUserRateLimiter` struct with token bucket algorithm
- `check_and_consume(user_id: &str) -> bool` -- per-sender_id rate checking
- Added `RateLimitConfig` to `Config` struct (messages_per_minute=10, burst_size=3)
- Integrated into `GatewayMessageHandler` -- rate check at start of `handle_with_multimodal`
- Silent drop on excess (returns `Ok(())`)

## Deviations from Plan

None -- plan executed exactly as written.

## Verification Results

1. `cargo test -p ironhermes-core ssrf` -- 16 passed, 3 ignored
2. `cargo test -p ironhermes-gateway rate_limiter` -- 5 passed
3. `cargo check --workspace` -- clean compilation, no errors
