# Phase 6: Event Hooks - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-08
**Phase:** 06-event-hooks
**Areas discussed:** Hook event model, Guardrail design, Webhook delivery, Hook configuration

---

## Hook Event Model

| Option | Description | Selected |
|--------|-------------|----------|
| Core three only | message_received, tool_called, response_sent — matches HOOK-01 exactly | |
| Core three + tool_completed | Add tool_completed for tool outcome observability | ✓ |
| Broad lifecycle | Core three + tool_completed + session_started + session_ended + error_occurred | |

**User's choice:** Core three + tool_completed
**Notes:** Four events total — enough for observability without excessive surface area.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal metadata | Event type, timestamp, session/chat ID, type-specific fields | |
| Rich payload | Minimal metadata + full content | |
| Configurable verbosity | Always include metadata. Full content opt-in per hook registration | ✓ |

**User's choice:** Configurable verbosity
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Yes — request_id per turn | UUID per incoming message, threaded through all events | ✓ |
| No — session_id is enough | Chat/session ID already groups events | |

**User's choice:** Yes — request_id per turn
**Notes:** Essential for log correlation across full request lifecycle.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Tracing spans + events | Emit as tracing structured events, reuse existing infra | |
| Dedicated hook log file | JSONL file at ~/.ironhermes/hooks/events.jsonl | ✓ |
| Both | Tracing + JSONL file | |

**User's choice:** Dedicated hook log file
**Notes:** Isolated from app logs, easy to tail/parse.

---

## Guardrail Design

| Option | Description | Selected |
|--------|-------------|----------|
| Config-driven blocklist | TOML/JSON config lists blocked tools by name/pattern | |
| Trait-based hook plugins | GuardrailHook trait with async check() method | |
| Both layers | Config blocklist + trait for complex logic | ✓ |

**User's choice:** Both layers
**Notes:** Config checked first, trait hooks second.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Clear error with reason | Error string like "Tool 'terminal' blocked by guardrail: untrusted context" | |
| Generic blocked message | "Tool call blocked by security policy." No details | |
| Configurable detail level | Default clear error, config option for generic | ✓ |

**User's choice:** Configurable detail level
**Notes:** Default shows reason, reducible for high-security deployments.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Block only | Binary allow/block decision | |
| Block + warn | Three outcomes: allow, warn (log but proceed), block | ✓ |

**User's choice:** Block + warn
**Notes:** Warn mode enables monitoring before enforcing.

---

## Webhook Delivery

| Option | Description | Selected |
|--------|-------------|----------|
| Configurable Authorization header | Static header value per endpoint | |
| HMAC-SHA256 signatures | Sign payload with shared secret | |
| Both options | Static header + HMAC signing, configured per endpoint | ✓ |

**User's choice:** Both options
**Notes:** Simple for basic setups, HMAC for security-conscious deployments.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Fire-and-forget | Send once, log failures | |
| Simple retry with backoff | Up to 3 retries with exponential backoff | |
| Persistent retry queue | Failed deliveries queued to disk, retried on schedule | ✓ |

**User's choice:** Persistent retry queue with configuration
**Notes:** User specified "with configuration" — sensible defaults with max_retries and queue_ttl overrides.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Sensible defaults only | Max 5 retries, exponential backoff, discard after 24h | |
| Full configuration | All parameters configurable | |
| Defaults + key overrides | Defaults with configurable max_retries and queue_ttl | ✓ |

**User's choice:** Defaults + key overrides
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| All events by default | Every webhook gets all events | |
| Per-endpoint event filter | Each webhook specifies subscribed event types | ✓ |

**User's choice:** Per-endpoint event filter
**Notes:** Reduces noise and bandwidth.

---

## Hook Configuration

| Option | Description | Selected |
|--------|-------------|----------|
| Main config file | Add [hooks] section to existing TOML config | |
| Separate hooks config | Dedicated ~/.ironhermes/hooks.toml | ✓ |
| Config + code registration | Config for declarative, code for trait-based | |

**User's choice:** Separate hooks config
**Notes:** Keeps hook config isolated from main config.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Startup only | Load once, changes require restart | |
| Hot-reload on file change | Watch hooks.toml, reload without restart | ✓ |
| Startup + API reload | Load at startup, explicit reload command | |

**User's choice:** Hot-reload on file change
**Notes:** Better DX for iterating on hook rules.

---

| Option | Description | Selected |
|--------|-------------|----------|
| No reference — new design | Design from scratch | |
| Loosely inspired by hermes-agent | Check for patterns worth borrowing | ✓ |

**User's choice:** Loosely inspired by hermes-agent
**Notes:** Not a strict port.

---

## Claude's Discretion

- Internal HookEvent enum design and serialization format
- JSONL rotation/size-limiting strategy
- File watcher implementation details
- Retry queue file format and location
- GuardrailHook trait exact method signatures
- Trait-based guardrail hook registration pattern

## Deferred Ideas

None — discussion stayed within phase scope
