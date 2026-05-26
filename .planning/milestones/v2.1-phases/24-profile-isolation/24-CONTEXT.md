# Phase 24: Profile Isolation - Context

**Gathered:** 2026-04-28
**Status:** Ready for planning

<domain>
## Phase Boundary

Each named profile gets its own isolated `HERMES_HOME` (config, memory stores, sessions DB, gateway PID file). Operator can switch via `hermes --profile work chat` without cross-contamination from `personal`. Phase 24 ships exactly one new operator-facing primitive — the `--profile <name>` global CLI flag — plus the supporting plumbing to make every existing consumer route through the profile-scoped home directory.

This phase covers **CFG-04** only. Phase 24 does NOT ship `hermes profile list/create/delete/rename/alias/export/import` — full profile lifecycle is `PROF-01..N` and stays deferred to **v2.2 Production Polish** per REQUIREMENTS.md. Gateway start/stop/status are existing surfaces that gain profile awareness through the same single resolution point used by every other consumer.

</domain>

<decisions>
## Implementation Decisions

### Profile Resolution

- **D-01: CLI sets `IRONHERMES_HOME` early in `main.rs` based on the `--profile` flag.** Before any `ironhermes_core::get_hermes_home()` call (and that means before `Cli::parse()` returns into any handler logic that touches state), the resolved profile path is written to `IRONHERMES_HOME` via `unsafe { std::env::set_var(...) }`. Zero changes to `crates/ironhermes-core/src/constants.rs` — every existing consumer (memory factory, state.db, skills, prompt_builder, models cache, config, .env) gets isolated for free because they already route through the same single resolution point. Single point of truth at the entry point.
- **D-02: `--profile` wins over a pre-set `IRONHERMES_HOME` env var, silently.** If `IRONHERMES_HOME=/custom/path hermes --profile work` is invoked, the resolved path is `<dirs::home_dir()>/.ironhermes/profiles/work/`, NOT `/custom/path`. Documented in `--help`. No warning emitted (no noise on the common case). Mirrors typical flag-beats-env precedence.
- **D-03: Profile name validation is slug-style `[a-z0-9][a-z0-9-]*`** — lowercase alphanumeric + dashes, must start with letter/digit. Matches the pattern Phase 21.8 uses for skill names (reuse `to_skill_slug` / `sanitize_name` if call surface fits, else hand-roll a tiny validator in `ironhermes-core`). Path-safe on every OS, no shell escaping headaches. **Reserved tokens rejected:** `default`, `current`, `none`, plus any name beginning with `_`. Reserved guard prevents collision with implicit `default` (the bare-`hermes` root).
- **D-04: Profile dirs always sit under the default root.** `--profile work` resolves to `<dirs::home_dir()>/.ironhermes/profiles/work/`, regardless of any pre-set `IRONHERMES_HOME`. The `IRONHERMES_HOME` env is ignored when `--profile` is set (per D-02). The literal `profiles/` subpath is locked in `pub const PROFILES_SUBDIR: &str = "profiles";` in `ironhermes-core::constants` — Phases 25/26 cannot relocate it. Backup tooling stays simple.

### Default Profile + First-Use

- **D-05: Bare `hermes` (no `--profile`) keeps using `~/.ironhermes/` exactly as before.** Zero migration. The `profiles/` subdirectory is created lazily on the first `--profile` use. Every existing user's working install survives the upgrade untouched. Phase 24 is purely additive — not a layout-changing migration.
- **D-06: First `hermes --profile NEW chat` (profile dir doesn't exist yet) auto-scaffolds AND auto-launches the Phase 23 setup wizard.** Sequence: validate name (D-03) → resolve path (D-04) → set `IRONHERMES_HOME` (D-01) → emit banner (D-08) → `ensure_home_dirs()` against the profile path → Phase 23's `preflight::run_preflight_check` triggers wizard for missing `config.yaml` → after wizard completes, drop into the originally-requested chat. Preserves the Learning Loop opt-in framing (Phase 23 D-16) at the per-profile level — closes the canonical hermes-agent gotcha for every new profile, not just the first install.
- **D-07: `--profile <name>` is a global clap flag** on the top-level `Cli` struct, available on every subcommand. Resolved before subcommand dispatch. Examples that all work: `hermes --profile work chat`, `hermes --profile work config show`, `hermes --profile work config set model.default openai/gpt-4o-mini`, `hermes --profile work setup`, `hermes --profile work gateway run`, `hermes --profile work skills install acme/foo`, `hermes --profile work doctor`. Matches `git -C <dir>` / `cargo --manifest-path` conventions.
- **D-08: One-line stderr banner when `--profile` is active.** Format: `[profile: work] HERMES_HOME=~/.ironhermes/profiles/work/` printed to stderr before any other output. Bare `hermes` (default profile) prints nothing — silent for backwards-compat with every existing pipe like `hermes -e "prompt" | jq`. Mirrors how the YOLO banner works (Phase 21.7 D-11/D-12). Stdout stays clean. Use the same `display_hermes_home()` helper (`ironhermes-core::constants:40`) for `~/`-relative rendering.

### Gateway PID File

- **D-09: PID file lives at `$HERMES_HOME/gateway.pid`** — at the root of the profile's home directory, alongside `config.yaml`, `state.db`, `.env`. Profile isolation falls out naturally because each profile's `get_hermes_home()` resolves to a different path. NOT in a `run/` subdirectory — keeps `ensure_home_dirs()` scaffolding unchanged from Phase 21.6 (no new `run/` entry needed). Single canonical location for `hermes status` / `hermes gateway stop` to find.
- **D-10: `gateway.pid` is YAML with three fields** — `pid: <integer>`, `started_at: <ISO8601 UTC string>`, `profile: <slug or "default">`. Self-describing — `cat gateway.pid` shows what's running. Lets `hermes status` cross-check the recorded `profile` against the resolved profile name (defense-in-depth against operator confusion when an orphan file lingers). **Atomic-write via `tempfile::NamedTempFile::persist()`** — write to `gateway.pid.tmpXXXX` first, fsync, then rename → atomic on POSIX. Avoids torn reads if `hermes status` reads concurrently with gateway startup. The YAML body is hand-rolled (3 lines) to avoid pulling serde_yaml into ironhermes-gateway just for the PID file.
- **D-11: Staleness detection via `kill(pid, 0)` probe with auto-delete on stale.** On `gateway run`: read `gateway.pid` if it exists; send signal 0 to the recorded pid via `nix::sys::signal::kill(Pid::from_raw(pid), None)` (or `libc::kill(pid, 0)` if nix isn't already in the dep tree). Three outcomes:
  - `Ok(())` → process is live and signalable by current user → live conflict, refuse per D-12
  - `Err(ESRCH)` (no such process) → stale, delete `gateway.pid` silently and proceed with startup
  - `Err(EPERM)` (process exists but owned by another user) → treat as live conflict, refuse per D-12 with note about ownership
  Cross-platform note: on Windows we'll need a `cfg!(unix)` guard and a fallback that uses `OpenProcess` — but Windows gateway support isn't a v2.1 deliverable, so a Unix-only path with `#[cfg(unix)]` is acceptable; non-Unix path can panic-with-clear-message until ACP/Phase 30 needs it.
- **D-12: Second `gateway run` in the same profile refuses with an explicit error** when staleness check (D-11) returns "live". Message format:
  ```
  ⛔ Gateway already running for profile 'work' (pid 12345, started 2026-04-28T12:00:00Z).
     Stop it first: hermes --profile work gateway stop
  ```
  Exit code: non-zero (e.g., `2`). No silent override, no `--force` flag in Phase 24 — recovery from a genuinely-zombie pid is via `rm $HERMES_HOME/gateway.pid` (with operator awareness). `--force` MAY be added in v2.2 if real-world ops surfaces a need, but it's not part of CFG-04. The `gateway stop` path (existing or new — see D-17 below) is responsible for removing `gateway.pid` on graceful shutdown.

### CLI Surface Scope

- **D-13: Strict minimum CLI surface — only the global `--profile <name>` flag.** Phase 24 does NOT open the `hermes profile` subcommand namespace. No `profile list`, no `profile create`, no `profile show`, no `profile current`. PROF-01..N (full lifecycle: list/create/use/delete/show/alias/rename/export/import) stays deferred to v2.2 Production Polish per REQUIREMENTS.md "v2.2 Reservation" section. Smallest blast radius for Phase 24, narrowest contract surface.
- **D-14: Profile discovery via `hermes status` Profile section.** Phase 21.7 already shipped `hermes status` with its `--all` / `--deep` / `--json` surface (`crates/ironhermes-cli/src/status_cmd.rs`). Add a Profile section to the existing status output that enumerates `~/.ironhermes/profiles/*/` (any subdir containing a `config.yaml`) and prints each profile's name + last-modified timestamp + Learning Loop status (re-uses Phase 23's banner logic). For each profile, also show whether `gateway.pid` exists and whether the recorded pid is live. Surfaces all the data `hermes profile list` would have provided, without opening a new namespace. Active profile (the one this invocation resolved) is marked with a `*`. `--json` output adds a top-level `profiles[]` array. Does NOT touch `hermes status` core behavior beyond adding the section.
- **D-15: `hermes config show` prepends a `Profile: <name>` line.** Always-on (not gated on `--profile` being set). Format:
  ```
  Profile: default
  🧠 Learning Loop: enabled (memory + skill generation)

  ... rest of YAML ...
  ```
  When --profile is active, the line shows the slug (e.g., `Profile: work`); when bare-`hermes` is used, the line shows `Profile: default` (literal string — `default` is the documented sentinel for the bare-`hermes` root). Sits ABOVE Phase 23's D-17 Learning Loop banner. One small edit to `cmd_config_show` in `crates/ironhermes-cli/src/config_cli.rs:113`. Keeps the Phase 23 banner stack consistent and tells the user which config file they're inspecting at a glance.
- **D-16: `hermes doctor` runs active-profile checks only.** Whichever profile this invocation resolved (default or `--profile X`), doctor checks: home dir exists, config exists, `.env` exists, API keys present, state.db reachable, plus a new check — `gateway.pid` is either absent OR refers to a live process (uses the same `kill -0` probe from D-11). No cross-profile enumeration — that's `hermes status`'s job (D-14). Smallest contract change to `cmd_doctor` in `crates/ironhermes-cli/src/main.rs:409`.

### Cross-Crate Type Pattern (carry-forward from Phase 22.4.2.2 + Phase 23 D-12)

- **D-17: New types in `ironhermes-core` crossing crate boundaries use plain Strings, not embedded downstream enums.** Example: a `ProfileName` newtype wrapping `String` lives in `ironhermes-core` (with the D-03 validator). Consumers (CLI subcommand dispatch, gateway startup, status command) construct their own enums at the call site. Avoids the circular-crate-dep problem documented in PROJECT.md Key Decisions.
- **D-18: PID file write/read helpers live in `ironhermes-gateway`, not `ironhermes-core`.** The gateway is the only crate that writes `gateway.pid`; the only readers are `gateway stop` (gateway crate), `hermes status` (CLI), and `hermes doctor` (CLI). Putting the helper in gateway keeps `ironhermes-core` free of process-management concerns. CLI consumers pull `pub fn read_gateway_pid(home: &Path) -> Result<Option<GatewayPidRecord>>` from `ironhermes-gateway`. The 3-line YAML format is stable enough that `serde` is overkill — hand-rolled parser inside one helper module.

### Test Strategy (locked at this stage)

- **D-19: Two integration tests are mandatory.** Plan must lock both:
  1. `profile_isolation_smoke` — spins up two distinct `IRONHERMES_HOME` paths via `tempfile::tempdir()` (simulating two profiles), runs `setup_wizard`-style apply via Phase 23's `apply_minimum_viable_answers` testability seam, writes a memory entry under each, asserts the two memories don't bleed across.
  2. `gateway_pid_concurrent_refuse` — writes a fake live `gateway.pid` to a temp profile dir (uses current process's pid + ISO timestamp + profile name), invokes the gateway-start path, asserts it returns the D-12 error and exit code without overwriting the file.

### Claude's Discretion

- Exact wording of the D-08 stderr banner, the D-12 conflict error message, and the D-14 Profile section header — Claude can pick clear phrasing.
- Whether to use `nix` crate or `libc` directly for `kill(pid, 0)` — picker should check existing dep tree (`nix` is a common ironhermes-rs dep; if absent, prefer `libc` already pulled by `tokio`).
- Whether `ProfileName` is a tuple-struct newtype or a plain validator function returning `Result<String, _>` — both are acceptable; tuple-struct gives type safety, plain function keeps surface flat. Either OK.
- Whether the D-10 atomic-write uses `tempfile::NamedTempFile::persist()` or a hand-rolled `write_then_rename` helper — tempfile is already a dev-dep and probably a regular dep somewhere; Claude picks the lighter option.
- Whether the D-11 `EPERM` branch logs differently than ESRCH — it's an edge case (gateway started by `root` while operator runs as user); planner can lock or leave to executor.

### Folded Todos

- **`2026-04-17-configuration-setup-wizard-improvements.md`** — this todo was originally folded into Phase 23 with explicit note that the CFG-04 portion (profile isolation) was split out to Phase 24. Phase 24 closes the remaining sliver of that todo. Tag with `resolves_phase: 24`.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### v2.1 Milestone Architectural Principles
- `.planning/PROJECT.md` §"Architectural Principles (carried through every v2.1 phase)" — Principle #2 (cache-awareness) directly applies: switching profiles can include switching `model.default`, which is cache-breaking per Phase 23 D-13. Phase 24 must NOT introduce a cross-profile cache-leak path.
- `.planning/REQUIREMENTS.md` §"Configuration / Setup Wizard" line 170 — CFG-04 verbatim text: "Profile isolation: each profile gets own HERMES_HOME, config, memory, sessions, gateway PID".
- `.planning/REQUIREMENTS.md` §"v2.2 Reservation" line 206 — PROF-01..N deferral statement. Phase 24 MUST stay narrow per this — no `hermes profile list/create/delete/...`.
- `.planning/ROADMAP.md` §"Phase 24: Profile Isolation" lines 447-465 — full success criteria (4 truths) and dependency on Phase 23.

### Phase 23 Carry-Forward (REQUIRED reading)
- `.planning/phases/23-configuration-cli-and-setup-wizard/23-CONTEXT.md` — preflight middleware semantics, cache-breaking schema, secret-redaction conventions, Learning Loop framing (D-14/D-16/D-17), Plan 02 fix-2 (preflight gate narrowed to `Some(Chat) | None && execute.is_none()` — Phase 24's --profile global flag must NOT widen this gate).
- `.planning/phases/23-configuration-cli-and-setup-wizard/23-VERIFICATION.md` — locks the current preflight gate location at `crates/ironhermes-cli/src/main.rs:213-223`. Phase 24's --profile resolution must run BEFORE this gate fires.

### Phase 21.6 Carry-Forward (REQUIRED reading)
- `.planning/phases/21-6-deployment-setup-files/` — `ensure_home_dirs()` was introduced here with the 8-subdir scaffolding. Phase 24 reuses the same function unchanged; just calls it against the profile-scoped home.

### Phase 21.7 Carry-Forward
- `crates/ironhermes-cli/src/status_cmd.rs` — Phase 21.7's `run_status` (D-18..D-22 status surface). Phase 24 D-14 adds a Profile section to this output. Existing fixtures use stub pids (`Some(12_345)` / `Some(67_890)`) — Phase 24 makes them real for the first time.

### v2.0 Cross-Crate Type Convention
- `.planning/PROJECT.md` Key Decisions table, last row: "Cross-crate transport types use plain Strings (no embedded downstream types)" — locked Phase 22.4.2.2. Phase 24 D-17 follows this verbatim for `ProfileName` and `GatewayPidRecord`.

### Codebase Code Sites
- `crates/ironhermes-core/src/constants.rs:30-48` — `get_hermes_home()` and `display_hermes_home()`. NOT modified by Phase 24 (per D-01).
- `crates/ironhermes-cli/src/main.rs:385-401` — `ensure_home_dirs()`. Reused unchanged.
- `crates/ironhermes-cli/src/main.rs:213-223` — Phase 23 preflight gate. Phase 24's --profile resolution runs BEFORE this gate.
- `crates/ironhermes-cli/src/main.rs:409-446` — `cmd_doctor`. Updated per D-16.
- `crates/ironhermes-cli/src/config_cli.rs:113` — `cmd_config_show`. Updated per D-15.
- `crates/ironhermes-cli/src/status_cmd.rs` — Profile section added per D-14.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets

- **`get_hermes_home()` (`ironhermes-core::constants:30`)** — single resolution point used by every consumer (memory factory, prompt builder, skills loader, models cache, state.db, config, .env). Phase 24 hooks this by setting `IRONHERMES_HOME` before any consumer runs. **Do NOT modify the function** — the env-var-first contract is what makes the rest of Phase 24 trivial.
- **`ensure_home_dirs()` (`ironhermes-cli/src/main.rs:385`)** — already idempotent (D-21 belt-and-suspenders), creates the 8-subdir tree. Phase 24 calls it against the profile-scoped path on first --profile use (D-06).
- **`display_hermes_home()` (`ironhermes-core::constants:40`)** — `~/`-relative path rendering. Reused for the D-08 stderr banner.
- **Phase 21.8 slug helpers (`ironhermes-core::skills` ecosystem)** — `to_skill_slug` / `sanitize_name`. Phase 24 D-03 may reuse if the surface fits, else hand-roll a `validate_profile_name` in `ironhermes-core` (10 lines + tests).
- **Phase 23 `preflight::run_preflight_check`** — fires on missing/invalid config. Phase 24 D-06 leans on this to auto-launch the wizard for new profiles.
- **`apply_minimum_viable_answers` testability seam (Phase 23 setup.rs:250)** — used in D-19 test #1 to drive cross-profile setup without rustyline.

### Established Patterns

- **Cache-breaking schema (Phase 23 D-13)** — fields tagged `cache_breaking: true` get a stderr warning on `hermes config set`. Phase 24 doesn't add new cache-breaking fields, but the *act* of switching profiles via `--profile` IS effectively a cache-break (entirely different model.default, system_prompt, memory provider can be in play). The D-08 stderr banner serves as the user-facing signal — no separate cache warning needed because no in-session mutation occurs.
- **Cross-crate plain-String pattern (Phase 22.4.2.2 D-decision, Phase 23 D-12)** — followed by Phase 24 D-17/D-18 verbatim.
- **Stderr-banner UX convention (Phase 21.7 D-11/D-12 yolo banner)** — Phase 24 D-08 mirrors. Stdout untouched, banner once per process at startup.
- **Atomic file writes via tempfile + rename (used in Phase 21.5 memory persistence and Phase 21.8 skill installer lock)** — Phase 24 D-10 reuses for `gateway.pid`.
- **Phase 23 D-15 dotted-path config write** — `hermes --profile work config set model.default X` works automatically because the resolved IRONHERMES_HOME already points at the profile's `config.yaml`. No special-casing in `config_setter`.

### Integration Points

- **`main.rs` top of `main()`** — single insertion point for D-01 profile resolution. Sequence: parse `Cli::parse()` → resolve profile path from `cli.profile` → set `IRONHERMES_HOME` env var → emit D-08 banner if profile active → continue existing `dotenv` + `ensure_home_dirs` + tracing init flow. Must run BEFORE `ensure_home_dirs()` and BEFORE the Phase 23 preflight gate.
- **`status_cmd.rs`** — Profile section added for D-14. New helper `enumerate_profiles(home: &Path) -> Vec<ProfileSummary>` walks `home.parent()/profiles/*/` (or just `~/.ironhermes/profiles/*/` since D-04 locks the location), returns name + last-modified + Learning Loop status + gateway pid status. JSON output adds top-level `profiles[]`.
- **`config_cli.rs::cmd_config_show`** — single line prepended for D-15. Reads the resolved profile name (from a new `current_profile()` helper that reverse-checks `IRONHERMES_HOME` against `~/.ironhermes/profiles/*/` paths, returning `"default"` if no match).
- **Gateway startup (in `ironhermes-gateway`)** — wraps existing start sequence with `acquire_pid_lock(home)` → write `gateway.pid` per D-10 → register cleanup on graceful shutdown (Drop impl on a `PidLockGuard` struct, plus explicit cleanup in the gateway's existing shutdown_all flow). Failure path of D-12 runs the lock check before any other startup work.

</code_context>

<specifics>
## Specific Ideas

- **The `default` sentinel is a literal string, not a special variant.** D-03 reserves the name `default` so users can't create a `~/.ironhermes/profiles/default/` that would shadow the bare-`hermes` root. The D-15 `Profile: default` line in `config show` uses the literal word `default`. The bare-`hermes` invocation does NOT auto-create a `profiles/default/` directory — D-05 explicitly: zero migration.
- **Banner formatting reuses `display_hermes_home()`** — `[profile: work] HERMES_HOME=~/.ironhermes/profiles/work/`. The `~/`-shortening is already implemented (`ironhermes-core::constants:40`). Avoid hand-rolling path display.
- **`gateway.pid` YAML hand-rolled, not `serde_yaml`** — the file is exactly 3 lines (`pid: N\nstarted_at: <ISO>\nprofile: <slug>\n`). A 20-line parse helper is easier to audit than dragging serde_yaml into ironhermes-gateway just for this. Same parser is used by `hermes status` (D-14) and `hermes doctor` (D-16) via D-18's `read_gateway_pid` helper.
- **Per-profile `.env` is automatic** — `Config::env_path()` (`ironhermes-core::config:643`) already resolves to `get_hermes_home().join(".env")`, so each profile gets its own `.env` for free. No special handling needed.
- **Subagent transcripts per-profile** — `$HERMES_HOME/subagent-transcripts/` (Phase 21.7 D-05) is created by `ensure_home_dirs()` and used by `subagent_runner.rs`. Falls out of D-01 automatically. Plan should include a 1-line regression test that asserts a transcript written under `--profile work` lands in `~/.ironhermes/profiles/work/subagent-transcripts/`, not `~/.ironhermes/subagent-transcripts/`.

</specifics>

<deferred>
## Deferred Ideas

- **`hermes profile list/create/show/current/use/delete/rename/alias/export/import`** — full PROF-01..N profile lifecycle. Stays deferred to **v2.2 Production Polish** per REQUIREMENTS.md. Phase 24 ships only the `--profile` flag plus the discovery surface in `hermes status` (D-14). When v2.2 opens, the `hermes profile` namespace can be designed against the foundation Phase 24 ships.
- **`hermes gateway run --force` (force-kill stale gateway and take over)** — useful for ops automation that needs guaranteed startup. Add in v2.2 if real-world use surfaces it. Workaround for now: `rm $HERMES_HOME/gateway.pid` with operator awareness.
- **`hermes doctor --all` (cross-profile sweep)** — surfaces orphan PID files, broken profile dirs. Skipped per D-16 (active-profile only). Add later if needed.
- **`IRONHERMES_PROFILE` env var as a fallback when `--profile` flag is absent** — would let a shell session export `IRONHERMES_PROFILE=work` once and have every subsequent `hermes` use it without retyping. Considered (option C in the resolution discussion) and explicitly NOT chosen for Phase 24. Add in v2.2 alongside the `hermes profile use <name>` lifecycle command if the workflow demand surfaces.
- **Gateway start TUI marker / persistent prompt prefix** (`work │ >` style) — kubectx/direnv-inspired. Discussed in the visibility step, deferred. The D-08 stderr banner is the locked Phase 24 surface; richer TUI integration can land in a future TUI polish phase if operators report banner-blindness.
- **Profile templates / "copy from existing profile"** — `hermes --profile new init --from work` to bootstrap a new profile with another's config. Naturally falls out of v2.2 PROF-01..N lifecycle work; not needed for CFG-04.
- **Windows `OpenProcess`-based PID liveness** — Phase 24 ships Unix-only PID checks (D-11). Windows path stubs panic with a clear "not supported on Windows in v2.1" message. Re-open when Windows gateway support becomes a deliverable.

### Reviewed Todos (not folded)

None reviewed-but-deferred — the only matching todo (`2026-04-17-configuration-setup-wizard-improvements.md`) was already partially folded into Phase 23 and the remaining sliver (CFG-04) is folded into Phase 24 here.

</deferred>

---

*Phase: 24-profile-isolation*
*Context gathered: 2026-04-28*
