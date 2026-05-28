---
slug: cron-allowlist-drops-token
status: resolved
trigger: cron delivery silently drops user's own Telegram token; user receives no scheduled-job output
created: 2026-05-28
updated: 2026-05-28
---

# Cron Delivery Allowlist Drops User Token 7018949547

## Symptoms

**Expected behavior:**
Cron jobs (Daily Caribbean Weather Forecast, Daily Weather Briefing) run on schedule and deliver their output as Telegram messages to the user (chat_id `7018949547`).

**Actual behavior:**
Cron jobs run successfully (`success=true` in logs) but deliveries are silently skipped. The user never receives the scheduled-job output in Telegram.

**Error message:**
```
2026-05-28T12:05:52.747833Z  WARN deliver token not in allowlist — skipping token=7018949547
2026-05-28T12:06:25.082695Z  WARN deliver token not in allowlist — skipping token=7018949547
2026-05-28T12:06:25.082751Z  WARN deliver token not in allowlist — skipping token=7018949547
```

Source: `crates/ironhermes-cron/src/delivery.rs:221` — `tracing::warn!(token=%token, "deliver token not in allowlist — skipping");`

**Timeline:**
Surfaced in the post-36.17.2 UAT log (2026-05-28). Unrelated to phase 36.17.2 — phase 36.17.2 only touched the per-chat session-queue and slash-command fast-path on the *inbound* path; the cron *outbound* delivery filter has been in place separately. Likely pre-existing; the user has only just now exercised cron deliveries while watching gateway logs.

**Reproduction:**
1. Have at least one configured cron job that targets chat_id `7018949547` (Daily Caribbean Weather Forecast, Daily Weather Briefing both qualify).
2. Wait for the cron tick (60s loop) or trigger manually.
3. Job runs, agent loop completes, then the delivery layer in `ironhermes-cron/src/delivery.rs` fires the allowlist warning and drops the outbound message.
4. User receives nothing in Telegram.

## Asymmetry observation

The same chat_id `7018949547` is happily accepted by the gateway on the inbound path (see same log: `Received message from dispatch channel chat_id=7018949547 sender_id=7018949547 content=/queue 999`). Only the *outbound* cron delivery filter rejects it.

This implies the cron-delivery allowlist is a separate config from the inbound gateway's filters — probably a deliberate second gate, but its source/population is unclear and the user's own token isn't on it.

## Current Focus

**hypothesis:** CONFIRMED — the `deliver` field on three jobs is set to a bare Telegram chat_id `7018949547` instead of the required `telegram:7018949547` format. The delivery code treats the deliver string as a platform name, not a chat_id.
**test:** Read jobs.json and delivery.rs — both confirmed.
**root_cause:** Data: three jobs have `"deliver": "7018949547"` (bare integer). Code: `expand_routing_token` at delivery.rs:163 checks if the token (after failing colon-split) is in `KNOWN_DELIVERY_PLATFORMS` = `["telegram", "discord", ...]`. `"7018949547"` is not a platform name, so it warns and returns empty. Schema: the `deliver` parameter description in both copies of `cronjob_tool.rs` is too vague (`"Delivery target. Default: 'local'."`) — no format guidance — causing the LLM to pass a bare chat_id.
**next_action:** Fix applied (see Resolution).

## Evidence

- timestamp: 2026-05-28T00:00:00Z
  file: /Users/twilson/.ironhermes/cron/jobs.json
  finding: Three jobs have `"deliver": "7018949547"` — a bare Telegram chat_id, not a platform token. Affected jobs: "Daily Weather Briefing" (id: 263d4ea1), "Miami Weather 9:18 EST" (id: b4ebf964), "Miami Weather Test - 3 mins" (id: a42ef03f). Other jobs use `"deliver": "origin"` or `"deliver": "local"` which are valid tokens.

- timestamp: 2026-05-28T00:01:00Z
  file: /Users/twilson/code/ironhermes/crates/ironhermes-cron/src/delivery.rs:200-230
  finding: `expand_routing_token` at line 163 processes the deliver string. Token `7018949547` does not contain `:` so it skips the `platform:chat_id` branch. It falls through to the bare-platform branch at line 220: `if !KNOWN_DELIVERY_PLATFORMS.contains(&token)` — since `7018949547` is not in `["telegram","discord","slack","matrix","whatsapp","webhook","qq"]`, it warns and returns empty.

- timestamp: 2026-05-28T00:02:00Z
  file: /Users/twilson/code/ironhermes/crates/ironhermes-tools/src/cronjob_tool.rs:382-385
  finding: Schema description for `deliver` is `"Delivery target. Default: 'local'."` — no examples, no mention of required `platform:chat_id` format. Identical in `crates/ironagent-tools-api/src/cronjob_tool.rs:382-385`. This is why the creating LLM passed a bare chat_id.

- timestamp: 2026-05-28T00:03:00Z
  file: /Users/twilson/.ironhermes/config.yaml
  finding: `gateway.platforms.telegram.whitelist: [7018949547]` — the user's chat_id IS in the inbound whitelist, confirming the asymmetry: inbound gate uses config whitelist, outbound cron delivery uses the deliver field's platform-name allowlist. These are different mechanisms.

## Eliminated

- **H1: cron delivery allowlist is a config-driven list separate from inbound whitelist** — ELIMINATED. There is no separate allowlist config for cron delivery. The `KNOWN_DELIVERY_PLATFORMS` constant is a security gate to prevent env-var enumeration (prevents crafted tokens like `stripe_secret` from reading `STRIPE_SECRET_HOME_CHANNEL`). It is NOT a per-user authorization filter. The bug is the deliver field format, not a missing allowlist entry.

- **H2: TELEGRAM_HOME_CHANNEL env var missing** — ELIMINATED. The affected jobs use `deliver = "7018949547"` which doesn't reach the home-channel lookup at all; it fails the platform-name check before that.

- **H3: Code regression from phase 36.17.2** — ELIMINATED. delivery.rs was not modified in 36.17.2. The warn path at line 221 has been there since before this phase.

## Resolution

**root_cause:** Three cron jobs were created with `"deliver": "7018949547"` — a bare Telegram chat_id. The delivery layer treats the deliver string as a platform name (e.g. `"telegram"`), not a chat_id. Platform names must be in `KNOWN_DELIVERY_PLATFORMS`; a numeric chat_id never is. The correct form is `"telegram:7018949547"` (platform colon chat_id) which routes to the `platform:chat_id` branch in `expand_routing_token` and resolves correctly.

**fix:**
1. Data fix: updated `~/.ironhermes/cron/jobs.json` — changed `deliver` from `"7018949547"` to `"telegram:7018949547"` for jobs 263d4ea1, b4ebf964, a42ef03f.
2. Schema fix: improved `deliver` parameter description in both `crates/ironhermes-tools/src/cronjob_tool.rs` and `crates/ironagent-tools-api/src/cronjob_tool.rs` with explicit format examples to prevent recurrence.

**verification:** Restart gateway; next cron tick for any of the three affected jobs should deliver to Telegram without the warning.
