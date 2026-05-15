# PARITY: `hermes-agent/cron/scheduler.py` → `crates/ironhermes-cron`

Comparison of the Python reference implementation (`hermes-agent/cron/scheduler.py`,
1820 LOC, plus its sibling `cron/jobs.py`, 1115 LOC) against the Rust crate
`crates/ironhermes-cron` (lib.rs, job.rs, parser.rs, scanner.rs, store.rs,
tick.rs, delivery.rs, display.rs).

The Python file is a single, batteries-included scheduler that owns parsing,
storage, locking, prompt assembly, delivery routing, script execution, and
agent invocation. The Rust crate is a slimmer, library-shaped subset focused on
**scheduling, persistence, locking, and delivery routing**. Agent execution
lives elsewhere in the workspace and is not in scope for this crate.

Legend:

- ✅ — feature present in Rust with equivalent semantics
- ⚠️ — present but with behavioural differences worth noting
- ❌ — present in Python, absent in Rust (intentional or gap)
- 🆕 — present in Rust only (added in the port)

---

## 1. Module layout

| Concern                          | Python                                    | Rust                                                              |
| -------------------------------- | ----------------------------------------- | ----------------------------------------------------------------- |
| Schedule parsing                 | `cron/jobs.py::parse_duration/parse_schedule/compute_next_run` | `parser.rs`                                                       |
| Job model                        | dict-shaped (`load_jobs` / `_normalize_job_record`) | `job.rs::CronJob` (strongly-typed serde)                          |
| Persistence                      | `cron/jobs.py::load_jobs/save_jobs/_jobs_file_lock` | `store.rs::JobStore`                                              |
| Threat scanning                  | `tools/cronjob_tools.py::_scan_cron_prompt` + `_scan_assembled_cron_prompt` | `scanner.rs::scan_cron_prompt`                                    |
| Tick lock                        | `scheduler.py::tick` (fcntl/msvcrt)       | `lib.rs::acquire_tick_lock` (`.tick.lock` file, O_CREAT \| O_EXCL) |
| Tick orchestration               | `scheduler.py::tick`                      | `tick.rs::run_tick_check` / `complete_job_run`                    |
| Delivery routing                 | `scheduler.py::_resolve_delivery_target(s)` | `delivery.rs::resolve_delivery_target`                            |
| Output persistence               | `cron/jobs.py::save_job_output`           | `delivery.rs::save_job_output`                                    |
| Plain-text formatters            | scattered across CLI helpers              | 🆕 `display.rs` (`format_job_list`, `format_job_detail`, `format_cron_status`) |
| Agent runner / `run_job`         | `scheduler.py::run_job`                   | ❌ Out of scope — belongs to the executor/agent crate              |

---

## 2. Schedule parsing & next-run computation

| Behaviour                                          | Python                                              | Rust                                                            | Status |
| -------------------------------------------------- | --------------------------------------------------- | --------------------------------------------------------------- | ------ |
| Duration tokens: `m/min/.../h/hr/.../d/day/...`    | regex `^(\d+)\s*(m\|...)$`                          | identical regex in `parser.rs::parse_duration`                  | ✅      |
| Interval prefix `every <duration>`                 | yes                                                 | yes                                                             | ✅      |
| Cron expressions (5/6 fields)                      | validated via `croniter`                            | validated via `cron` crate; 5-field normalized to 6 by prepending `0 ` | ⚠️     |
| Cron allowed chars                                 | `[\d*\-,/]`                                         | `[\d*\-,/?]` (adds `?`)                                         | ⚠️     |
| ISO timestamps (`T`, RFC3339, naive, date-only)    | `datetime.fromisoformat`, naive ⇒ system-local tz   | `DateTime::parse_from_rfc3339` → naive `%Y-%m-%dT%H:%M:%S` → date `%Y-%m-%d`; naive ⇒ **UTC** | ⚠️     |
| Bare duration ⇒ Once at now + duration             | yes                                                 | yes                                                             | ✅      |
| `compute_next_run` for `Once` after-firing         | `_recoverable_oneshot_run_at` keeps it valid inside 120s grace until first run | `Once` is "due" only while `run_at > after`; one-shot eligibility decided by `JobStore::mark_job_run` via `RepeatConfig.times == Some(1)` | ⚠️     |
| `ONESHOT_GRACE_SECONDS = 120`                      | yes                                                 | ❌ no analogous grace for Once jobs                              | ❌      |
| `compute_next_run(Cron, last_run_at)` anchors next run at `last_run_at` to survive restarts | yes        | always anchors at `after = now` — no `last_run_at` parameter    | ⚠️     |
| `compute_next_run(Interval, last_run_at)` ⇒ `last + interval` | yes                                       | always `after + interval` (interpreted as `now + interval` at call site) | ⚠️     |
| Dynamic per-schedule grace (`_compute_grace_seconds`, `MIN_GRACE=120s` … `MAX_GRACE=7200s`, half of period) | yes        | single fixed `JobStore.grace_seconds = 3600`                    | ❌      |
| Display strings                                    | `"once at {YYYY-MM-DD HH:MM}"`, `"once in {original}"`, `"every {minutes}m"` | `"once at {raw input s}"`, `"once in {minutes}m"`, `"every {minutes}m"` | ⚠️     |

---

## 3. Job model

| Field                       | Python (`create_job`)                                    | Rust (`CronJob` in `job.rs`)              | Status |
| --------------------------- | -------------------------------------------------------- | ----------------------------------------- | ------ |
| `id`                        | `uuid.uuid4().hex[:12]`                                  | `Uuid::new_v4().to_string()` (full uuid)  | ⚠️     |
| `name`                      | derived from prompt/skill/script if missing              | required, taken from caller               | ⚠️     |
| `prompt`                    | str                                                      | `String`                                  | ✅      |
| `skills` / legacy `skill`   | both kept aligned via `_apply_skill_fields`              | `Vec<String>` only — no legacy mirror     | ⚠️     |
| `schedule` (`ScheduleParsed`) | `{kind, ...}` dict                                     | tagged enum `ScheduleParsed::{Once,Interval,Cron}` | ✅ |
| `schedule_display`          | yes                                                      | yes                                       | ✅      |
| `repeat` `{times, completed}` | yes (auto `times=1` for Once)                          | yes (auto `times=Some(1)` for Once)       | ✅      |
| `enabled`                   | yes                                                      | yes                                       | ✅      |
| `state`                     | `"scheduled" \| "paused" \| "completed" \| "error"`      | `JobState::{Scheduled, Paused, Completed}` | ⚠️     |
| `"error"` state             | recurring jobs that fail to compute next_run             | ❌ no `Error` variant                      | ❌      |
| `paused_at` / `paused_reason` | yes                                                    | yes                                       | ✅      |
| `created_at`, `last_run_at`, `next_run_at` | yes                                       | yes                                       | ✅      |
| `last_status` / `last_error` | yes                                                     | yes                                       | ✅      |
| `last_delivery_error`       | yes (tracked separately)                                 | ❌                                         | ❌      |
| `deliver`                   | yes (string)                                             | yes (string)                              | ✅      |
| `origin`                    | dict; non-dict values defensively treated as missing     | `Option<JobOrigin>` (strongly typed)      | ✅      |
| `model` / `provider` / `base_url` | per-job runtime overrides                          | ❌                                         | ❌      |
| `script` / `no_agent`       | per-job script + skip-agent flag                         | ❌                                         | ❌      |
| `context_from`              | inject another job's last output as context              | ❌                                         | ❌      |
| `enabled_toolsets`          | per-job toolset whitelist                                | ❌                                         | ❌      |
| `workdir`                   | per-job working dir for AGENTS.md / TERMINAL_CWD         | ❌                                         | ❌      |

> The omitted fields are agent-runner concerns. They belong with `run_job`,
> which is itself out of scope for this crate (see §10). When the executor
> crate lands, these fields will need to be added to `CronJob` and threaded
> through `JobStore`.

---

## 4. Storage (`jobs.json`)

| Concern                                            | Python (`cron/jobs.py`)                                 | Rust (`store.rs`)                                        | Status |
| -------------------------------------------------- | -------------------------------------------------------- | -------------------------------------------------------- | ------ |
| Atomic write (temp + rename + fsync)               | `tempfile.mkstemp` + `os.fsync` + `atomic_replace`       | temp `.json.tmp` + `fs::rename` (no explicit fsync)      | ⚠️     |
| File permissions 0600 / 0700 on Unix               | `_secure_file` / `_secure_dir`                           | ❌                                                       | ❌      |
| In-process serialization                           | `_jobs_file_lock` (threading.Lock)                       | `Arc<Mutex<JobStore>>` in callers                        | ✅      |
| Format envelope                                    | `{ "jobs": [...], "updated_at": ... }`                   | bare `[...]` (top-level array)                           | ⚠️     |
| Legacy migration                                   | implicit via `_apply_skill_fields`/`_normalize_job_record` per read | explicit `LegacyCronJob` with `agent_input`, `schedule` (string), `next_run`, `last_run`, `last_output` → `CronJob` | ⚠️     |
| Bare control-char auto-repair (`json.loads(strict=False)` → rewrite) | yes                              | ❌                                                       | ❌      |
| Hot reload (re-read jobs.json without rebuilding handle) | always (every CRUD reloads on demand)              | `JobStore::reload()` + called inside `run_tick_check`    | ✅      |
| `add_job` / `update_job` / `remove_job`            | yes                                                      | yes (`JobUpdate` for partial updates)                    | ✅      |
| `get_job` / `find_job` (id or case-insensitive name) | `get_job` (id only)                                    | both — `find_job` adds case-insensitive name lookup       | 🆕     |
| `pause_job` / `resume_job` / `trigger_job`         | yes (`trigger_job` ⇒ run on next tick)                   | partial — `toggle_job(id, enabled)`; **no `trigger`**    | ❌      |
| `rewrite_skill_refs` (curator hook)                | yes                                                      | ❌                                                       | ❌      |
| `mark_job_run` increments completed, advances `next_run_at`, auto-deletes when limit reached | auto-deletes job from list when limit hit | sets `state = Completed`, `next_run_at = None`; **does not delete** | ⚠️     |
| Recurring job with no computable `next_run_at` left enabled with `state=error` and explanatory `last_error` | yes — protects against silent disable | ❌ no `Error` state path                                  | ❌      |
| Atomic mark-run sequence (advance next_run_at FIRST → record → optionally complete) | yes        | yes — same order in `mark_job_run`                       | ✅      |

### `get_due_jobs` semantics

| Step                                              | Python                                                | Rust                                                | Status |
| ------------------------------------------------- | ------------------------------------------------------ | --------------------------------------------------- | ------ |
| Skip disabled                                     | yes                                                    | yes                                                 | ✅      |
| Recover missing `next_run_at`                     | for `once` (grace) + recurring (recompute from now)    | ❌ (missing `next_run_at` ⇒ never due)              | ❌      |
| Skip & fast-forward stale recurring jobs          | dynamic grace (`_compute_grace_seconds`), updates `next_run_at` and persists | fixed grace `3600s`; updates `next_run_at` in-memory (persisted via subsequent `save()` from caller) | ⚠️     |
| Skip paused jobs                                  | yes                                                    | yes (state must be `Scheduled`)                     | ✅      |
| Return due jobs                                   | yes                                                    | yes (returns `&[CronJob]` borrows)                  | ✅      |

---

## 5. Tick locking

| Behaviour                                          | Python                                                | Rust                                                  | Status |
| -------------------------------------------------- | ------------------------------------------------------ | ----------------------------------------------------- | ------ |
| Mechanism                                          | `fcntl.flock(LOCK_EX \| LOCK_NB)` (Unix), `msvcrt.locking` (Windows) | filesystem-level `.tick.lock` created with `O_CREAT \| O_EXCL`; PID written to file | ⚠️     |
| Skip tick when lock held                           | yes                                                    | yes (`Ok(None)` from `acquire_tick_lock`)             | ✅      |
| Stale-lock recovery (dead holder PID)              | not needed — `fcntl` releases on process death         | `try_recover_stale_lock` reads PID, `kill(pid,0)` on Unix; recovers if dead | 🆕     |
| Auto-release on drop                               | `lock_fd.close()` in `finally`                         | `LockGuard::Drop` removes `.tick.lock`                | ✅      |
| Cross-platform                                     | `fcntl` + `msvcrt` fallback                            | Unix uses `libc::kill`; non-Unix conservatively treats holder as alive | ⚠️     |

> The two mechanisms are not interchangeable. If both implementations point at
> the same `cron/` directory, they will not interlock — Python's `flock`
> doesn't see Rust's `.tick.lock` file, and vice-versa.

---

## 6. Threat scanning

| Pattern category                                   | Python (`tools/cronjob_tools._CRON_THREAT_PATTERNS`) | Rust (`scanner.rs::CRON_THREAT_PATTERNS`) | Status |
| -------------------------------------------------- | ----------------------------------------------------- | ------------------------------------------ | ------ |
| Ignore previous instructions                       | yes                                                   | yes                                        | ✅      |
| Do not tell the user                               | yes                                                   | yes                                        | ✅      |
| System prompt override                             | yes                                                   | yes                                        | ✅      |
| Disregard instructions/rules/guidelines            | yes                                                   | yes                                        | ✅      |
| `curl`/`wget` exfiltrating env vars                | yes                                                   | yes                                        | ✅      |
| `cat .env / credentials / .netrc / .pgpass`        | yes                                                   | yes                                        | ✅      |
| `authorized_keys`, `/etc/sudoers`, `rm -rf /`      | yes                                                   | yes                                        | ✅      |
| Invisible unicode (ZWSP, ZWJ, BOM, BiDi overrides) | yes                                                   | yes                                        | ✅      |
| **Assembled-prompt rescan** (after skill content is loaded) — `_scan_assembled_cron_prompt` + `CronPromptInjectionBlocked` | yes — closes #3968 | ❌ — only `scan_cron_prompt` exists; no assembled-prompt re-entry | ❌ |

---

## 7. Delivery routing

| Behaviour                                          | Python (`_resolve_delivery_target(s)`)                | Rust (`delivery.rs::resolve_delivery_target`)      | Status |
| -------------------------------------------------- | ----------------------------------------------------- | --------------------------------------------------- | ------ |
| `deliver=local` ⇒ no delivery                      | yes                                                   | yes                                                 | ✅      |
| `deliver=origin` ⇒ map from `job.origin`           | yes                                                   | yes                                                 | ✅      |
| `deliver=origin` without origin ⇒ fall back to any platform's configured home channel | yes — iterates `_iter_home_target_platforms` | ❌ — returns `None`                                 | ❌      |
| `deliver=platform:chat_id`                         | yes                                                   | yes                                                 | ✅      |
| `deliver=platform` (no colon) ⇒ use platform's home channel env var | yes                                  | ❌                                                  | ❌      |
| Comma-separated multi-target (`"telegram,discord"`) | yes (`_resolve_delivery_targets` returns list)       | ❌ — single target only                             | ❌      |
| Routing intent token `all` ⇒ every connected platform | yes (`_expand_routing_tokens`)                      | ❌                                                  | ❌      |
| `list`/`tuple` deliver value flattened (`_normalize_deliver_value`) | yes                                  | ❌ (`deliver` is `String` by type)                  | ⚠️     |
| Validation against `_KNOWN_DELIVERY_PLATFORMS`     | yes                                                   | ❌                                                  | ❌      |
| Plugin platform discovery (`platform_registry`) for `cron_deliver_env_var` | yes                            | ❌                                                  | ❌      |
| Channel directory resolution (`resolve_channel_name`, e.g. `Alice (dm)` → real id) | yes                       | ❌                                                  | ❌      |
| Per-platform home env var table (`_HOME_TARGET_ENV_VARS`) + legacy fallback (`QQBOT_HOME_CHANNEL` → `QQ_HOME_CHANNEL`) | yes | ❌                                                  | ❌      |
| Thread/topic id env var (`<X>_HOME_CHANNEL_THREAD_ID`) | yes                                                | ❌ — thread_id flows only from `JobOrigin`          | ❌      |
| Origin records with non-dict / missing fields gracefully treated as missing | yes (`_resolve_origin`)              | ❌ — Serde rejects malformed origin                 | ⚠️     |

### `[SILENT]` marker

| Behaviour                          | Python                                  | Rust                                  | Status |
| ---------------------------------- | --------------------------------------- | ------------------------------------- | ------ |
| Suppress delivery on `[SILENT]`    | `SILENT_MARKER in deliver_content.strip().upper()` (`in` test → matches *anywhere* in upper-cased output) | `delivery.rs::is_silent` ⇒ `output.trim().to_uppercase().starts_with("[SILENT]")` (only the **prefix**) | ⚠️ |
| Saves output to disk regardless    | yes                                     | yes                                   | ✅      |

> The semantics diverge: Python suppresses delivery if `[SILENT]` appears
> *anywhere* in the (upper-cased) response; Rust only suppresses when it is
> the leading token. The system prompt instructs the agent to respond with
> *exactly* `[SILENT]`, so in practice both behave the same on well-behaved
> outputs, but the Python form is more forgiving of stray whitespace, ANSI
> noise, or model preambles.

---

## 8. Output persistence & formatting

| Behaviour                                          | Python                                                | Rust                                                | Status |
| -------------------------------------------------- | ----------------------------------------------------- | --------------------------------------------------- | ------ |
| Save path                                          | `~/.hermes/cron/output/{job_id}/{YYYY-MM-DD_HH-MM-SS}.md` | `~/.hermes/cron/output/{job_id}/{YYYYMMDD_HHMMSS}.md` | ⚠️ |
| Atomic temp + rename                               | yes (`tempfile.mkstemp` + `atomic_replace`)            | yes                                                 | ✅      |
| `0700` / `0600` chmod                              | yes                                                   | ❌                                                  | ❌      |
| Path-traversal guard on `job_id`                   | implicit (uuid hex)                                    | explicit reject of `/`, `\`, `..`, empty            | 🆕     |
| Truncation for platform delivery                   | embedded in `_deliver_result` (header + content)       | `format_delivery_message` (`MAX_PLATFORM_OUTPUT = 4000`, `floor_char_boundary` for UTF-8) | 🆕 |
| `wrap_response` header/footer (configurable via `cron.wrap_response`) | yes                            | ❌                                                  | ❌      |
| MEDIA: tag extraction → native attachment send     | yes (`BasePlatformAdapter.extract_media`)             | ❌                                                  | ❌      |
| Plain-text formatters for list/detail/status views | ad-hoc in CLI helpers                                  | `display.rs::format_job_list / format_job_detail / format_cron_status` | 🆕 |

---

## 9. Tick orchestration

| Behaviour                                          | Python (`scheduler.py::tick`)                         | Rust (`tick.rs::run_tick_check` + `complete_job_run`) | Status |
| -------------------------------------------------- | ----------------------------------------------------- | ---------------------------------------------------- | ------ |
| Acquire tick lock                                  | yes                                                   | yes                                                  | ✅      |
| Reload `jobs.json` inside the tick                 | implicit — every helper reloads                       | yes — `JobStore::reload()` inside `run_tick_check`   | ✅      |
| Advance `next_run_at` for **all** due recurring jobs BEFORE running any of them (at-most-once) | yes — `advance_next_run` per job | ⚠️ — `complete_job_run` advances **after** the run via `mark_job_run`; no pre-tick advancement of all due jobs | ⚠️ |
| Per-job execution                                  | `run_job(job)` (LLM agent or `no_agent` script)       | ❌ — the executor is responsible for this; the crate hands due jobs back to the caller | ❌ |
| Parallel execution (`ThreadPoolExecutor`, configurable `HERMES_CRON_MAX_PARALLEL` / `cron.max_parallel_jobs`) | yes | ❌ — caller decides | ❌ |
| Workdir jobs serialized (TERMINAL_CWD is process-global) | yes — partitioned & run sequentially            | n/a — no workdir field                               | ❌      |
| ContextVar isolation per job (`contextvars.copy_context().run(...)`) | yes                                | n/a — handled by tokio task scope at caller          | n/a    |
| Inactivity timeout (`HERMES_CRON_TIMEOUT`, default 600s, polled via `agent.get_activity_summary`) | yes | ❌ — caller's concern                                  | ❌      |
| Post-tick MCP orphan cleanup (`_kill_orphaned_mcp_children`) | yes                                          | ❌                                                   | ❌      |
| Save output → mark run → deliver order             | save → deliver → mark                                  | save → mark → return target (delivery handled by caller) | ⚠️ |
| Empty `final_response` ⇒ soft failure (status=error, "agent completed but produced empty response", issue #8585) | yes | ❌ — caller decides via `success` boolean             | ❌      |

---

## 10. Agent execution — out of scope

Everything below lives inside Python's `run_job` (≈600 LOC, lines 1013–1656)
and is **not implemented anywhere in this crate**. It belongs in the executor /
agent layer and is listed here for traceability only.

- `no_agent` short-circuit: bash/python script's stdout delivered verbatim
- `script` pre-run + wake-gate parsing (`{"wakeAgent": false}` ⇒ silent skip)
- `_run_job_script`: HERMES_HOME/scripts/ sandboxing, bash/python interpreter
  selection, `HERMES_CRON_SCRIPT_TIMEOUT`, secret redaction via
  `agent.redact.redact_sensitive_text`
- `_build_job_prompt`: cron hint banner, skill loading via `skill_view`,
  `context_from` chain (8K char cap), assembled-prompt threat rescan
- `CronPromptInjectionBlocked` failure mode → blocked delivery doc
- AIAgent construction: model/provider/base_url/api_mode/acp_command resolution,
  `resolve_runtime_provider` with fallback chain, `credential_pool`, MCP tool
  discovery, `enabled_toolsets`/`disabled_toolsets` wiring, `skip_context_files`,
  `load_soul_identity`, `skip_memory`, `platform="cron"`
- Per-job `workdir` ⇒ `TERMINAL_CWD` + AGENTS.md/CLAUDE.md/.cursorrules pickup
- `HERMES_CRON_SESSION` env flag, `HERMES_CRON_AUTO_DELIVER_*` ContextVars
- Inactivity-tracked timeout polling with `agent.interrupt`
- Live-adapter delivery (E2EE matrix paths) with standalone fallback
- MEDIA: tag splitting + per-extension routing
  (`send_voice`/`send_image_file`/`send_video`/`send_document`)
- `SessionDB` lifecycle for cron sessions, `cleanup_stale_async_clients`,
  `agent.close()` to keep fd count bounded
- `wrap_response` header/footer and per-job `name` interpolation
- `_deliver_result` multi-target loop with per-target error accumulation

---

## 11. Summary of intentional vs incidental gaps

**Intentional gaps** — these belong outside the crate by design:

- `run_job` / agent loop / AIAgent construction
- Script execution and wake-gate parsing
- Skill loading and assembled-prompt rescan
- Live-adapter delivery (E2EE paths), MEDIA tag routing
- MCP discovery and orphan reaping
- `HERMES_CRON_AUTO_DELIVER_*` ContextVar wiring (Rust does this with task locals at the executor)
- Inactivity timeout (a property of the agent, not the scheduler)

**Incidental gaps** — worth tracking as port-completion tasks:

1. **`last_delivery_error`** field on `CronJob` (separate from `last_error`)
2. **`JobState::Error`** variant for recurring jobs whose `compute_next_run` returns `None`
3. **`one-shot grace window`** (`ONESHOT_GRACE_SECONDS = 120`)
4. **Dynamic per-schedule grace** (replace fixed `grace_seconds = 3600` with half-period clamped to 120s..7200s)
5. **`compute_next_run(Cron|Interval, last_run_at)`** anchoring at last run instead of `after`
6. **Recover missing `next_run_at`** in `get_due_jobs` (recompute for recurring, grace-window recovery for once)
7. **`trigger_job`** (force run on next tick by setting `next_run_at = now`)
8. **Multi-target delivery** in `resolve_delivery_target` (returning `Vec<DeliveryTarget>`)
9. **`all` routing token** + comma-separated `deliver` parsing
10. **`deliver=platform` (no chat_id)** ⇒ look up `<PLATFORM>_HOME_CHANNEL` env var
11. **`deliver=origin` fallback** to any platform's home channel when origin is missing
12. **`_KNOWN_DELIVERY_PLATFORMS`** allowlist (prevents env-var enumeration via crafted platform names)
13. **Legacy env var fallback** (`QQBOT_HOME_CHANNEL` → `QQ_HOME_CHANNEL`)
14. **Thread-id env var** (`<X>_HOME_CHANNEL_THREAD_ID`)
15. **Unix 0700/0600 chmod** on cron dirs and `jobs.json` / output files
16. **`{ "jobs": [...], "updated_at": ... }` envelope** vs bare array — pick one and decide migration
17. **Bare-control-char auto-repair** for corrupted `jobs.json` (`strict=False` then rewrite)
18. **fsync on save** (Rust does `flush()` but not `fsync`; atomic rename only guarantees crash-consistency if the temp file's data is durable)
19. **`[SILENT]` matching** — decide between `starts_with` (Rust) and `in` (Python) and align
20. **`format_delivery_message` `wrap_response` header/footer** (currently always `[Job: <name>]\n…`)
21. **`rewrite_skill_refs`** curator hook (called by the skill curator after consolidation)
22. **Defensive `JobOrigin` parsing** — non-dict origins from hand-edited `jobs.json` should be treated as missing instead of failing serde

**Rust-only additions** worth keeping:

- `JobStore::find_job` (case-insensitive name lookup)
- `display.rs` plain-text formatters
- `delivery.rs::format_delivery_message` UTF-8-safe truncation
- Explicit path-traversal guard on `save_job_output(job_id, …)`
- Stale tick-lock recovery via PID liveness check
- Strongly-typed `JobUpdate` partial-update struct
