# Phase 24: Profile Isolation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-28
**Phase:** 24-profile-isolation
**Areas discussed:** Profile resolution mechanism, Default profile + first-use, Gateway PID file design, CLI surface scope

---

## Profile Resolution Mechanism

### Q1: How should --profile map to a HERMES_HOME path at runtime?

| Option | Description | Selected |
|--------|-------------|----------|
| CLI sets IRONHERMES_HOME early | Before any get_hermes_home() call, main.rs computes the path from --profile and sets IRONHERMES_HOME via env::set_var. Zero changes to ironhermes-core. Every consumer gets isolated for free. | ✓ |
| Add precedence chain inside get_hermes_home() | Modify constants.rs to check IRONHERMES_PROFILE first, then IRONHERMES_HOME, then default. More explicit but expands core surface. | |
| Pass a Profile context object through call sites | Thread a Profile struct through every get_hermes_home() consumer. Cleanest in theory but a 7-crate refactor. | |
| Let Claude decide | Pick the approach. | |

**User's choice:** CLI sets IRONHERMES_HOME early (Recommended)
**Notes:** Locked the env-var-injection-at-entry-point pattern. Core stays untouched.

### Q2: What's the precedence when both --profile and IRONHERMES_HOME env are set?

| Option | Description | Selected |
|--------|-------------|----------|
| --profile wins, overwrites env | Silent override. Simplest mental model. | ✓ |
| --profile wins but warns to stderr | Override with explicit ⚠ to stderr. | |
| Refuse with error | Force the user to pick one. Most defensive. | |
| IRONHERMES_HOME wins | Inverts flag-beats-env convention. | |

**User's choice:** --profile wins, overwrites env (Recommended)

### Q3: What profile name format/validation should we enforce?

| Option | Description | Selected |
|--------|-------------|----------|
| Slug-style: [a-z0-9][a-z0-9-]* | Lowercase alphanumeric + dashes, must start with letter/digit. Matches Phase 21.8 skill slug pattern. | ✓ |
| Filesystem-safe (broader) | Allow alphanumeric + `-_.`. Adds case-sensitivity issues on macOS. | |
| Reserved-name guard added on top | Reject `default`, `current`, `none`, leading `_`. | |
| Let Claude decide | | |

**User's choice:** Slug-style: [a-z0-9][a-z0-9-]* (Recommended)
**Notes:** Reserved-token guard from option C folded in by Claude (D-03 reserves `default`/`current`/`none`/leading-underscore).

### Q4: When --profile is used, where do profile directories actually sit on disk?

| Option | Description | Selected |
|--------|-------------|----------|
| Always under default root | --profile work always resolves to <home>/.ironhermes/profiles/work/, regardless of pre-set IRONHERMES_HOME. | ✓ |
| Under user-set root if any | If IRONHERMES_HOME=/custom, --profile work resolves to /custom/profiles/work. Complicates precedence. | |
| Hardcoded constant `profiles/{name}` subpath | Lock the literal `profiles/` in const PROFILES_SUBDIR. | |
| Let Claude decide | | |

**User's choice:** Always under default root (Recommended)
**Notes:** PROFILES_SUBDIR const from option C folded in by Claude (D-04).

---

## Default Profile + First-Use

### Q1: When a user runs bare `hermes` (no --profile) on an existing install, where should HERMES_HOME point?

| Option | Description | Selected |
|--------|-------------|----------|
| Existing `~/.ironhermes/` stays the implicit default | Zero migration. Bare hermes keeps using `~/.ironhermes/` exactly as before. | ✓ |
| Migrate existing tree into `profiles/default/` | Cleaner mental model long-term but breaks every existing user's home dir. | |
| Two roots coexist forever | Permanent dual layout. Simplest in code but confusing in `tree`. | |
| Let Claude decide | | |

**User's choice:** Existing `~/.ironhermes/` stays the implicit default (Recommended)

### Q2: On first `hermes --profile NEW chat`, what should happen?

| Option | Description | Selected |
|--------|-------------|----------|
| Auto-scaffold + auto-launch Phase 23 wizard | Same UX as a fresh install but inside the new profile dir. | ✓ |
| Auto-scaffold but skip the wizard | Loses the Learning Loop opt-in framing — reproduces the canonical hermes-agent gotcha per profile. | |
| Refuse with explicit setup hint | Forces explicit operator intent. Discoverable but adds friction. | |
| Let Claude decide | | |

**User's choice:** Auto-scaffold + auto-launch Phase 23 wizard (Recommended)

### Q3: Where on the CLI should `--profile <name>` be accepted?

| Option | Description | Selected |
|--------|-------------|----------|
| Global flag on every subcommand | Top-level Cli struct flag. Matches `git -C <dir>` convention. | ✓ |
| Only on entry-point subcommands | chat/gateway/setup/single-shot only. Cripples cross-profile admin. | |
| Global flag + IRONHERMES_PROFILE env fallback | Adds env-var fallback for sustained per-profile work. | |
| Let Claude decide | | |

**User's choice:** Global flag on every subcommand (Recommended)
**Notes:** IRONHERMES_PROFILE env fallback explicitly deferred to v2.2 (per Deferred Ideas in CONTEXT.md).

### Q4: Should there be a visible signal about which profile is active?

| Option | Description | Selected |
|--------|-------------|----------|
| One-line stderr banner when --profile is set | `[profile: work] HERMES_HOME=...` to stderr. Bare hermes silent. Stdout untouched. | ✓ |
| Status bar / TUI marker only | Invisible in single-shot and gateway logs. | |
| Banner + persistent prompt prefix | Adds `work │ >` rustyline prompt. Strongest reinforcement. | |
| No visible indicator | Cheapest but error-prone in long sessions. | |

**User's choice:** One-line stderr banner when --profile is set (Recommended)

---

## Gateway PID File Design

### Q1: Where should the gateway PID file live?

| Option | Description | Selected |
|--------|-------------|----------|
| $HERMES_HOME/gateway.pid | Profile isolation falls out from get_hermes_home() resolution. Single canonical location. | ✓ |
| $HERMES_HOME/run/gateway.pid | FHS-style separation of runtime state. Adds run/ to ensure_home_dirs(). | |
| $XDG_RUNTIME_DIR/ironhermes-{profile}.pid | Cross-platform headache (macOS doesn't define XDG_RUNTIME_DIR). | |
| Let Claude decide | | |

**User's choice:** $HERMES_HOME/gateway.pid (Recommended)

### Q2: What should gateway.pid contain?

| Option | Description | Selected |
|--------|-------------|----------|
| PID + start_time + profile_name (YAML) | Three-line YAML: pid, started_at, profile. Self-describing. | ✓ |
| PID only (single line) | Matches sshd/postgresql/nginx convention. Loses self-description. | |
| Full process snapshot (JSON) | Diagnostic-rich but every field becomes a contract. | |
| Let Claude decide | | |

**User's choice:** PID + start_time + profile_name (YAML) (Recommended)

### Q3: How should staleness be detected?

| Option | Description | Selected |
|--------|-------------|----------|
| kill(pid, 0) probe + delete on stale | Standard POSIX pattern. ESRCH→stale, EPERM→live conflict. | ✓ |
| Time-based heuristic | Unreliable; long-running gateway looks dead. | |
| Lock file via fcntl/flock | Robust but platform-specific; outside current crate set. | |
| Let Claude decide | | |

**User's choice:** kill(pid, 0) probe + delete on stale (Recommended)

### Q4: What happens when a SECOND `hermes gateway run` starts in the same profile while one is live?

| Option | Description | Selected |
|--------|-------------|----------|
| Refuse with explicit error | Print pid+started_at+stop hint, exit non-zero. Matches systemd/nginx. | ✓ |
| Refuse + offer --force flag | Same as A by default; --force kills + takes over. | |
| Take over silently | SIGTERM old pid, start fresh. Loses session without consent. | |
| Let Claude decide | | |

**User's choice:** Refuse with explicit error (Recommended)
**Notes:** --force flag explicitly deferred to v2.2 if real-world ops needs surface it.

---

## CLI Surface Scope

### Q1: Beyond the global --profile flag, should Phase 24 ship a `hermes profile` subcommand surface?

| Option | Description | Selected |
|--------|-------------|----------|
| Strict minimum: just --profile flag | No `hermes profile` namespace. Matches REQUIREMENTS.md PROF-01..N → v2.2 deferral. | ✓ |
| Minimal trio: profile list/show/current | Read-only commands for discoverability. | |
| Add `profile create <name>` too | Slight overlap with --profile auto-scaffolding. | |
| Full lifecycle (delete/rename/alias) | Contradicts REQUIREMENTS.md deferral note. | |

**User's choice:** Strict minimum: just --profile flag

### Q2: Without `hermes profile list`, how do users discover what profiles exist?

| Option | Description | Selected |
|--------|-------------|----------|
| `hermes status` gains a Profile section | Reuses existing surface from Phase 21.7. | ✓ |
| `hermes doctor` lists profiles | Lower discoverability than status. | |
| Document only (no surface) | Cheapest but worst UX. | |
| Let Claude decide | | |

**User's choice:** `hermes status` gains a Profile section (Recommended)

### Q3: Should `hermes config show` surface the active profile name?

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, prepend `Profile: work` line | Always-on. Sits above Phase 23 D-17 banner. One small edit to cmd_config_show. | ✓ |
| Only when --profile is explicitly set | Bare hermes shows nothing. | |
| No change to config show | User can run `hermes config path`. | |
| Let Claude decide | | |

**User's choice:** Yes, prepend `Profile: work` line (Recommended)
**Notes:** Bare hermes shows literal `Profile: default` (matches D-03 reserved-name guard).

### Q4: What should `hermes doctor` check related to profiles?

| Option | Description | Selected |
|--------|-------------|----------|
| Active-profile checks only | Whichever profile this invocation resolved. Adds gateway.pid liveness check. | ✓ |
| All profiles with --all flag | Default A; add --all for cross-profile sweep. | |
| No profile awareness | doctor stays exactly as it is. | |
| Let Claude decide | | |

**User's choice:** Active-profile checks only (Recommended)

---

## Claude's Discretion

- Exact wording of D-08 stderr banner, D-12 conflict error message, D-14 Profile section header
- `nix` crate vs `libc` directly for `kill(pid, 0)` — picker checks existing dep tree
- `ProfileName` as tuple-struct newtype vs plain validator function
- `tempfile::NamedTempFile::persist()` vs hand-rolled `write_then_rename` for atomic gateway.pid write
- Whether D-11's EPERM branch logs differently than ESRCH (edge case: gateway started by root while operator runs as user)

## Deferred Ideas

- `hermes profile list/create/show/current/use/delete/rename/alias/export/import` — full PROF-01..N lifecycle → v2.2 Production Polish
- `hermes gateway run --force` (force-kill stale gateway) → v2.2 if ops needs surface it
- `hermes doctor --all` (cross-profile sweep) → later if needed
- `IRONHERMES_PROFILE` env var as flag fallback → v2.2 alongside `hermes profile use <name>`
- TUI prompt prefix (`work │ >` style) → future TUI polish phase
- Profile templates / "copy from existing profile" → falls out of v2.2 PROF-01..N
- Windows `OpenProcess`-based PID liveness → re-open when Windows gateway support becomes a deliverable
