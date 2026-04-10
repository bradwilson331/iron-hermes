---
phase: 10
slug: batch-processing
status: verified
threats_open: 0
asvs_level: 1
created: 2026-04-10
---

# Phase 10 — Security

> Per-phase security contract: threat register, accepted risks, and audit trail.

---

## Trust Boundaries

| Boundary | Description | Data Crossing |
|----------|-------------|---------------|
| JSONL input -> AgentLoop | Untrusted prompts from input file cross into agent execution | User-authored prompts |
| AgentLoop output -> JSONL file | Tool results may contain secrets that leak to output | API keys, tokens, PEM blocks |
| Checkpoint file -> resume logic | Corrupted checkpoint could cause skipped or re-run entries | Content hashes, completion state |
| Cancel sentinel file | Filesystem-based IPC between cmd_run and cmd_cancel | Presence/mtime signals |
| Tool names in messages -> registry | Hallucinated names indicate unreliable trajectory | Tool call metadata |
| Filter pipeline | Quality gate for training data output | Trajectory pass/reject decisions |

---

## Threat Register

| Threat ID | Category | Component | Disposition | Mitigation | Status |
|-----------|----------|-----------|-------------|------------|--------|
| T-10-01 | Information Disclosure | batch/filters.rs | mitigate | SECRET_PATTERNS RegexSet (Stripe, JWT, GitHub PAT, Slack, AWS AKIA, PEM) scans Role::Tool + Role::Assistant; flagged trajectories routed to reject file | closed |
| T-10-02 | Tampering | batch/checkpoint.rs | mitigate | Atomic write (tmp + rename) in save_checkpoint; load_checkpoint returns empty HashMap for missing/empty file | closed |
| T-10-03 | Denial of Service | batch/runner.rs | mitigate | Semaphore bounds concurrent tasks; input streamed line-by-line via BufReader (not loaded into memory) | closed |
| T-10-04 | Spoofing | JSONL input prompts | accept | Input is local file from user's filesystem; prompt injection is user's own risk in batch mode | closed |
| T-10-05 | Repudiation | batch/runner.rs | mitigate | Rejected trajectories written to separate _rejected.jsonl with rejection_reason field; nothing silently discarded | closed |
| T-10-06 | Elevation of Privilege | batch/filters.rs | accept | False negatives possible (novel secret formats); regex covers common patterns; users can manually review output | closed |
| T-10-06b | Denial of Service | batch/runner.rs | mitigate | Stale cancel sentinel cleaned via mtime check (clean_stale_sentinel); tokio::select! with 500ms poll detects cancel during semaphore contention | closed |
| T-10-07 | Tampering | cancel sentinel mtime | accept | Low risk — sentinel is in IRONHERMES_HOME, user-controlled directory | closed |
| T-10-08 | Denial of Service | 500ms polling interval | accept | Negligible overhead; cancel latency bounded to 500ms worst case | closed |

*Status: open / closed*
*Disposition: mitigate (implementation required) / accept (documented risk) / transfer (third-party)*

---

## Accepted Risks Log

| Risk ID | Threat Ref | Rationale | Accepted By | Date |
|---------|------------|-----------|-------------|------|
| AR-01 | T-10-04 | Input from user's own filesystem; prompt injection is user's own risk (same as interactive mode) | gsd-security-auditor | 2026-04-10 |
| AR-02 | T-10-06 | SECRET_PATTERNS covers common formats; novel formats may slip through; users can manually review | gsd-security-auditor | 2026-04-10 |
| AR-03 | T-10-07 | Cancel sentinel in IRONHERMES_HOME; attacker with fs write access has broader concerns | gsd-security-auditor | 2026-04-10 |
| AR-04 | T-10-08 | 500ms polling adds negligible overhead; acceptable UX tradeoff | gsd-security-auditor | 2026-04-10 |

---

## Security Audit Trail

| Audit Date | Threats Total | Closed | Open | Run By |
|------------|---------------|--------|------|--------|
| 2026-04-10 | 9 | 9 | 0 | gsd-security-auditor (sonnet) |

---

## Sign-Off

- [x] All threats have a disposition (mitigate / accept / transfer)
- [x] Accepted risks documented in Accepted Risks Log
- [x] `threats_open: 0` confirmed
- [x] `status: verified` set in frontmatter

**Approval:** verified 2026-04-10
