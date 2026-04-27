# Phase 23: Configuration CLI and Setup Wizard - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-27
**Phase:** 23-configuration-cli-and-setup-wizard
**Areas discussed:** Wizard rendering style, Wizard scope, First-run trigger, Cache-break UX on `config set`, Wizard sections list, `config set` key syntax, `config show` redaction policy, `config migrate` trigger

---

## Round 1 — Domain framing

### Wizard rendering style

| Option | Description | Selected |
|--------|-------------|----------|
| Inline rustyline prompts | Sequential question-by-question prompts via rustyline (Phase 22.3 infra). Lightweight, scriptable, matches existing UX. | ✓ |
| Ratatui form (in-place) | Multi-field form via ratatui (Phase 22.4 infra). User tabs between fields, sees defaults inline, validates as they go. | |
| Hybrid: rustyline by default, ratatui for `hermes setup` only | Lightweight inline for auto-launch; rich ratatui for explicit setup. | |
| External editor (open config.yaml) | Generate a commented config.yaml template, open $EDITOR. | |

**User's choice:** Inline rustyline prompts
**Notes:** Aligns with the lightweight, scriptable UX. Reuses Phase 22.3's rustyline 15 wiring. Wizard prompts run without history persistence so wizard answers don't bleed into chat history.

### Wizard scope

| Option | Description | Selected |
|--------|-------------|----------|
| Minimum viable: provider + API key + model only | 3 questions on first run, defaults for everything else. | |
| Section-based: subcommand routing per section | `hermes setup` runs MV; `hermes setup model\|memory\|gateway\|tools\|agent` configures sections. Mirrors hermes-agent. | ✓ |
| Comprehensive single-flow | One long wizard covering every section on first run. | |

**User's choice:** Section-based subcommand routing
**Notes:** Mirrors canonical hermes-agent design. Phase 25 (Toolset Management) and Phase 26 (Provider Polish) MAY plug additional questions into their own setup sections later — Phase 23 establishes the dispatch surface.

### First-run trigger

| Option | Description | Selected |
|--------|-------------|----------|
| Auto-launch on missing config.yaml | Run `hermes` with no config.yaml triggers wizard. | |
| Auto-launch on missing OR invalid config | Same plus `Config::validate()` failure repairs broken sections. | ✓ |
| Explicit only — require `hermes setup` | No auto-launch. | |

**User's choice:** Auto-launch on missing OR invalid config
**Notes:** Most robust. Wizard runs in fix-mode for invalid config, preserving valid sections. Distinct from explicit `hermes setup` which always runs full minimum-viable.

### Cache-break UX on `hermes config set`

| Option | Description | Selected |
|--------|-------------|----------|
| Warn and persist (recommended) | Inline warning + change always lands. Aligns with v2.1 architectural principle #2. | ✓ |
| Block when active session detected | Refuse with --force escape if running gateway/CLI session. | |
| Annotate in ConfigField schema, no per-key warnings | Generic warning when any cache_breaking field changes. | |
| Silent persist | No warning; relies on PRMT-06 frozen-at-session-start property. | |

**User's choice:** Warn and persist
**Notes:** Informational warning + change persists. PRMT-06 already prevents mid-session prompt mutation. Phase 23 introduces a `cache_breaking: bool` flag on ConfigField; Phase 27 (Prompt Caching) MAY refine the field-tagging list.

---

## Round 2 — Implementation specifics

### Wizard sections list

| Option | Description | Selected |
|--------|-------------|----------|
| model | Provider, API key, default model, base_url. Required for first-run minimum viable. | ✓ |
| memory | Memory backend (file/sqlite/grafeo/duckdb), HERMES_HOME, memory_enabled toggles. | ✓ |
| gateway | Telegram bot token, allowed chat IDs, port. Skip on first run unless wiring up Telegram. | ✓ |
| tools | Toolset enable/disable. Coordinates with Phase 25. | ✓ |

**User's choice:** all 4 (model, memory, gateway, tools)
**Notes:** `agent` section deferred to Phase 26 (Provider Polish — owns BudgetHandle / PROV config schemas). `skills` section deferred to Phase 28 (Skills Trust Tiers — owns SKILL-09 trust-tier additions).

### `config set` key syntax

| Option | Description | Selected |
|--------|-------------|----------|
| Dotted path (recommended) | `config set model.default <val>` — matches YAML structure, mirrors git/npm/cargo. | ✓ |
| Hyphenated flat | `config set model-default <val>` | |
| Section + key | `config set --section model default <val>` | |
| Dotted + section subcommand alias | `config set model.default <val>` + `config model set default <val>` ergonomic alias. | |

**User's choice:** Dotted path
**Notes:** Matches YAML, mirrors common CLI conventions, no special parsing required.

### `config show` secret redaction

| Option | Description | Selected |
|--------|-------------|----------|
| Mask with prefix preserved (recommended) | `sk-abc***` — first 4–6 chars + asterisks. Helps verify right key loaded. | ✓ |
| Hide entirely with placeholder | `<redacted>` for any secret field. Most conservative. | |
| Show in full to TTY only | If stdout is real TTY, show; if piped, redact. Pragmatic but risky. | |
| User-controlled flag | Default redact; `--reveal-secrets` shows full. Matches `git config --list`. | |

**User's choice:** Mask with prefix preserved
**Notes:** Phase 23 adds `secret: bool` to ConfigField schema. `.env` values not inlined into `config show` — surfaced via separate `hermes config env-path` (path only).

### `config migrate` trigger

| Option | Description | Selected |
|--------|-------------|----------|
| Manual only — explicit `hermes config migrate` | User runs the command. No surprise behavior. | ✓ |
| Manual + post-install hook for `hermes skills install` | Manual works; auto-runs after every skills install. | |
| Manual + on first run when skills directory has changed | Manual works; auto on `hermes` start if skills/ mtime changed. | |
| All triggers (manual + post-install + start-up) | Highest UX coverage; more code paths. | |

**User's choice:** Manual only
**Notes:** Surprise behavior is a UX hazard. Users discover gaps via skill failure messages or `hermes doctor`. Phase 28 MAY add hooks if skill-install UX gaps surface.

---

## Claude's Discretion

- Wizard question phrasing and exact validation error messages — planner picks reasonable phrasing.
- Order within each section's question list — planner picks; document in plan if non-obvious.
- Whether `hermes config show --section <X>` filter lands in v2.1 or defers — small ergonomic; planner picks if budget allows.
- Whether `hermes config get` returns YAML, raw scalar, or both — default raw scalar; `--json` for future polish.
- Whether `hermes config edit` (open `$EDITOR`) lands in v2.1 or v2.2 — planner picks.

## Deferred Ideas

- `hermes setup agent` — Phase 26 (Provider Polish)
- `hermes setup skills` — Phase 28 (Skills Trust Tiers)
- `hermes setup voice` / STT / TTS — VOICE-01..N parked in Future Requirements; defer to v2.2+
- `hermes config show --json` — v2.2 polish
- `hermes doctor --fix` integration — v2.2 Production Polish reservation; Phase 23's fix-mode is partial overlap but scoped narrower
- Auto-trigger `config migrate` on skill install — explicitly rejected for v2.1 (D-11); reconsider in Phase 28
