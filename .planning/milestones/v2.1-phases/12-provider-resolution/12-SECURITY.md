---
phase: 12
slug: provider-resolution
status: verified
threats_open: 0
asvs_level: 1
created: 2026-04-11
---

# Phase 12 — Security

> Per-phase security contract: threat register, accepted risks, and audit trail.

---

## Trust Boundaries

| Boundary | Description | Data Crossing |
|----------|-------------|---------------|
| Config → ProviderResolver | Credentials flow from config/env into resolver | API keys (sensitive) |
| ProviderResolver → LLM Clients | Resolver passes scoped credentials to client constructors | API key per provider |
| AnthropicClient → Anthropic API | HTTP requests carrying API key to external service | API key via x-api-key header |
| Credential file → AnthropicClient | Local file with OAuth token read at startup | OAuth access token |
| LLM response → adapter | Untrusted JSON parsed into ChatMessage | Model output |
| Budget counter → system prompt | Threshold messages injected into agent context | Hardcoded strings |

---

## Threat Register

| Threat ID | Category | Component | Disposition | Mitigation | Status |
|-----------|----------|-----------|-------------|------------|--------|
| T-12-01 | Info Disclosure | ResolvedEndpoint | mitigate | Custom Debug impl redacts api_key → `[REDACTED]` (provider.rs:24-34) | closed |
| T-12-02 | Tampering | ProviderConfig.base_url | mitigate | `is_provider_url_safe()` rejects non-https unless localhost/127.0.0.1 (provider.rs:47-56, invoked at 142) | closed |
| T-12-03 | Tampering | fallback_providers | mitigate | Post-build validation rejects unknown fallback provider names (provider.rs:186-196) | closed |
| T-12-04 | Info Disclosure | AnthropicClient | mitigate | Custom Debug redacts api_key (anthropic_client.rs:463-469). Key sent only via x-api-key header to self.base_url | closed |
| T-12-05 | Tampering | adapt_messages | mitigate | Takes &[ChatMessage] from trusted agent loop. No user-controlled format strings | closed |
| T-12-06 | Info Disclosure | discover_anthropic_credential | mitigate | Hardcoded path ~/.claude/credentials.json. Token only passed to AnthropicClient constructor | closed |
| T-12-07 | Spoofing | CodexResponses | accept | Returns Err at construction. No API calls made. Stub only | closed |
| T-12-08 | DoS | Budget counter | mitigate | Hard stop at budget >= max_iterations (agent_loop.rs:251-256). Config-bounded | closed |
| T-12-09 | Tampering | Budget threshold injection | accept | Threshold messages are &'static str constants. No user input in injection path | closed |
| T-12-10 | DoS | Fallback retry loop | mitigate | MAX_RETRIES=3, one-shot via .take() (agent_loop.rs:316), exponential backoff | closed |
| T-12-11 | Info Disclosure | Old resolve methods | mitigate | resolve_base_url/resolve_api_key deleted from Config. Zero matches in codebase | closed |
| T-12-12 | Tampering | Resolver immutability | mitigate | ProviderResolver: Clone, private fields, all methods &self. No &mut self after build() | closed |
| T-12-13 | Info Disclosure | Debug logging | mitigate | ResolvedEndpoint Debug redacts api_key. Zero tracing calls log API keys | closed |

---

## Accepted Risks Log

| Risk ID | Threat Ref | Rationale | Accepted By | Date |
|---------|------------|-----------|-------------|------|
| AR-12-01 | T-12-07 | CodexResponses is a stub that errors at construction. No attack surface | gsd-security-auditor | 2026-04-11 |
| AR-12-02 | T-12-09 | Budget threshold strings are hardcoded constants, not user-controllable | gsd-security-auditor | 2026-04-11 |

---

## Security Audit Trail

| Audit Date | Threats Total | Closed | Open | Run By |
|------------|---------------|--------|------|--------|
| 2026-04-11 | 13 | 13 | 0 | gsd-security-auditor (sonnet) |

---

## Sign-Off

- [x] All threats have a disposition (mitigate / accept / transfer)
- [x] Accepted risks documented in Accepted Risks Log
- [x] `threats_open: 0` confirmed
- [x] `status: verified` set in frontmatter

**Approval:** verified 2026-04-11
