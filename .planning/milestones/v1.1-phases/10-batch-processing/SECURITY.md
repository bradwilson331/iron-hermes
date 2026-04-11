# Security Audit — Phase 10: Batch Processing

**Audit Date:** 2026-04-10
**ASVS Level:** 1
**Auditor:** gsd-security-auditor (claude-sonnet-4-6)
**Plans Audited:** 10-01, 10-02, 10-03, 10-04

---

## Threat Verification

| Threat ID | Category | Disposition | Status | Evidence |
|-----------|----------|-------------|--------|----------|
| T-10-01 | Information Disclosure | mitigate | CLOSED | `filters.rs:11-29` (SECRET_PATTERNS RegexSet); `filters.rs:112-123` (Role::Tool \|\| Role::Assistant scan); `runner.rs:170-180` (reject file routing) |
| T-10-02 | Tampering | mitigate | CLOSED | `checkpoint.rs:30-33` (tmp + rename atomic write); `checkpoint.rs:16-23` (empty/missing graceful load) |
| T-10-03 | Denial of Service | mitigate | CLOSED | `runner.rs:73-89` (BufReader line-by-line streaming); `runner.rs:201` (Semaphore::new(worker_count)); `runner.rs:220-236` (select!-bounded acquire) |
| T-10-05 | Repudiation | mitigate | CLOSED | `runner.rs:44-54` (reject_path derivation); `runner.rs:261-265` (rejection_reason field); `runner.rs:170-180` (reject file write) |
| T-10-06 (cancel) | Denial of Service | mitigate | CLOSED | `runner.rs:122-123` (clean_stale_sentinel call); `runner.rs:441-456` (mtime < run_start guard); `runner.rs:220-236` (500ms select! poll) |
| T-10-04 | Spoofing | accept | CLOSED | See accepted risks log below |
| T-10-06 (patterns) | Elevation of Privilege | accept | CLOSED | See accepted risks log below |
| T-10-07 | Tampering | accept | CLOSED | See accepted risks log below |
| T-10-08 | Denial of Service | accept | CLOSED | See accepted risks log below |

---

## Accepted Risks Log

| Threat ID | Category | Rationale | Owner |
|-----------|----------|-----------|-------|
| T-10-04 | Spoofing — prompt injection via JSONL input | Input is a local file from the user's own filesystem. Prompt injection into the agent is the user's own risk in batch mode, the same exposure as interactive mode. No additional mitigation warranted. | User |
| T-10-06 (patterns) | EoP — false negatives in SECRET_PATTERNS | Regex set covers common patterns (Stripe, JWT, GitHub PAT, Slack, AWS AKIA, PEM). Novel or custom secret formats may evade detection. Users should manually review output files when handling highly sensitive credentials. | User |
| T-10-07 | Tampering — cancel sentinel mtime in user-controlled directory | The sentinel file resides in IRONHERMES_HOME, a user-controlled directory. An attacker with filesystem write access to that directory already has broader system access. Risk is low; the timestamp-guarded cleanup (Plan 04) further reduces the attack surface. | User |
| T-10-08 | Denial of Service — 500ms cancel polling interval | Polling at 500ms adds negligible CPU/IO overhead. Maximum cancel latency of 500ms is acceptable UX for a batch operation. | User |

---

## Unregistered Threat Flags

None. All threat flags reported in SUMMARY.md files for Plans 10-01 through 10-04 map to registered threat IDs or are explicitly noted as "None".

---

## Notes

- Agent-level errors (API failures, panics) during batch execution are logged to stderr (`runner.rs:282-284`) but do not produce TrajectoryLine records. These infrastructure failures are not trajectories and fall outside the repudiation threat scope (T-10-05), which covers completed trajectory routing.
- The `all_entries` Vec in `runner.rs:79` collects prompt strings (not trajectory data) in memory for checkpoint skip filtering. This is consistent with the T-10-03 mitigation intent: large concurrent agent result data is never held simultaneously; only lightweight prompt hashes accumulate.
