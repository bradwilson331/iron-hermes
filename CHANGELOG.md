# Changelog

## [0.4.5] — Release branch realigned with develop; project attribution (2026-09-20)

`main` had been parked at the v0.3.1 divergence point (`a5e378ae9`, 2026-06-19)
while all work continued on `develop`. This release brings the release branch
across that gap — 3568 commits — and settles the project's version and
attribution metadata.

The merge was clean, and the resulting tree is byte-identical to `develop` at
`a1205d61c`: the four commits `main` carried on its own (a GHCR Docker publish
workflow, a `.gitignore` update, and a `node_modules` removal) were already
reflected on `develop` and contributed nothing new.

### Changed

- **Workspace version is now `0.4.5`.** Only `Cargo.lock` needed re-resolving.
  Every workspace member already declares `version.workspace = true`, and all
  nine runtime version strings read `env!("CARGO_PKG_VERSION")`, so no source
  change was required. `package.json`, `Dioxus.toml`, `install.sh` and the CI
  workflows carry no version literal.

- **Project `authors` is now `["Wilson Tech"]`**, replacing `["Nous Research"]`.
  This propagated to the three places that state this repository's own
  authorship: the README license line, the `hermes version` footer, and the
  agent's `DEFAULT_AGENT_IDENTITY` system prompt (whose text is pinned by a
  test assertion that moved with it).

  Five manifests that had not been inheriting the workspace value now use
  `authors.workspace = true`: `iron_hermes_ui` (which had hardcoded personal
  attribution), `ironhermes-exec`, and the three `providers/memory-*` crates.
  The last three sit outside `crates/`, so a `crates/*/Cargo.toml` glob misses
  them — `cargo metadata --no-deps` is the only workspace-wide source of truth.

- **Upstream attribution is unchanged and deliberate.** `README.md` and
  `docs/ARCHITECTURE.md` still credit [hermes-agent](https://github.com/NousResearch/hermes-agent)
  by Nous Research as the Python project IronHermes is a port of, and the
  parity table and `ironhermes-core` constants still name Nous Research as an
  LLM *provider*. Only this repository's own authorship changed.

### Note on 0.4.0–0.4.4

Those versions were never given changelog entries; the previous entry here is
0.3.9 (2026-09-04) while `Cargo.toml` had already moved to 0.4.0. The feature
work across that span is therefore not summarized above. It is recorded
per-phase under `.planning/phases/` and in the `.planning/STATE.md` history,
and writing it up is an outstanding retrospective task rather than something
reconstructed here.

## [0.3.9] — Origin allowlist diagnosability (2026-09-04)

Follow-up to 0.3.8, from deploying it. The origin allowlist worked as designed
and was still nearly impossible to debug when a configured value was subtly
wrong: the server booted clean and refused every WebSocket upgrade, with no way
to see what it had actually loaded.

### Fixed

- **A trailing DNS root label silently broke the allowlist.**
  `https://host.` is a legal fully-qualified name — the trailing dot is the
  explicit root label — so it passed every guard: valid URL, valid host, https
  scheme, no path. The server started without complaint and refused every
  upgrade, because `Url::origin()` preserves the dot and browsers never send it
  in `Origin`. The root label is now dropped during canonicalization. Safe in
  one direction only, which is why it normalizes rather than rejects: `host.`
  and `host` are the same host by DNS definition, so removing the label can
  only turn a false reject into a correct accept — it cannot make a foreign
  origin match. Scheme and port are untouched.

### Added

- **The resolved origin allowlist is now logged once at startup**, naming both
  the origins and which source supplied them (`config.yaml` or
  `IRONHERMES_WEB_ALLOWED_ORIGINS`). 0.3.8 deliberately kept the origin set out
  of the per-rejection warning so it could not land in the access log on every
  failed upgrade — correct for that log, but it left the list observable
  nowhere at all. A byte-exact comparison whose accepted failure mode is a
  *false reject* needs somewhere to read what was loaded. A once-at-boot line
  does not scale with traffic, is not attacker-triggerable, and carries only
  the deployment's own public URLs.

  The source attribution matters as much as the values: `config.yaml` wins over
  the env var when non-empty, so "I set `IRONHERMES_WEB_ALLOWED_ORIGINS` and it
  was ignored" is a real failure mode that otherwise looks identical to a typo.

## [0.3.8] — Proxy-aware WebSocket origin check + container hardening (2026-09-03)

Headline: live chat and the live kanban board work again behind a TLS-terminating
reverse proxy. The WebSocket cross-site check had been deriving its expected
`Origin` from a flag that has nothing to do with the request's actual scheme, so
every upgrade on the Caddy-fronted deployment was rejected with a 403. Fixing it
surfaced two further bypasses in the same check, both closed here, plus an
operator-configurable origin allowlist and a fail-closed startup guard.

### ⚠ Breaking — read before upgrading a non-loopback deployment

**A non-loopback bind with no configured origins now refuses to start.** This is
deliberate: the previous behavior was to run with a cross-site check that had
silently degraded to a no-op, which is how the outage went unnoticed. The refusal
message names the remedy inline.

Before upgrading anything that binds a non-loopback address — including any
container run with `-e IP=0.0.0.0` — set the public URL browsers use:

```bash
IRONHERMES_WEB_ALLOWED_ORIGINS='https://hermes.example.com'
```

or in `config.yaml`, as a sibling of `web_ui.auth`:

```yaml
web_ui:
  allowed_origins: ["https://hermes.example.com"]
```

`config.yaml` wins when non-empty; the env var applies only when it is absent or
empty (the same precedence `web_ui.auth.password_hash` uses). On a trusted LAN,
`["*"]` accepts any browser origin — but it is an escape hatch, not an off
switch: an upgrade carrying no `Origin` header is still rejected. The default
loopback posture is unchanged and needs no configuration.

### Fixed

- **Every WebSocket upgrade 403'd behind a TLS-terminating reverse proxy.** The
  expected `Origin` took its scheme from `cookie_secure` — a session-cookie
  attribute, unrelated to the request's real scheme. With TLS terminated at the
  edge and `cookie_secure` left at its `false` default, the server expected
  `http://host` while the browser sent `https://host`, and rejected every
  upgrade to `/api/ws/chat` and `/api/ws/kanban`. The scheme now comes from the
  proxy's `X-Forwarded-Proto`, falling back to the previous behavior when the
  header is absent, so loopback and LAN deployments are byte-identical to before.
- **A WebSocket upgrade with no `Origin` header bypassed the check entirely.**
  It is now rejected on both checked paths, ahead of any allowlist logic, so no
  configuration value can reintroduce the bypass.
- **A WebSocket upgrade with no `Host` header skipped the check entirely** — the
  whole origin block was nested inside the `Host` extraction. Found while fixing
  the above; not present in any prior release note because it had never been
  identified.
- **`X-Forwarded-Proto` was matched case-sensitively.** URI schemes are
  case-insensitive (RFC 3986 §3.1) and not every proxy emits lowercase, so a
  proxy sending `HTTPS` reproduced the same 403 this release fixes. The value is
  now matched case-insensitively and canonicalized to lowercase before use.
- **Documentation told operators the opposite of what to do.** The
  `cookie_secure` guidance claimed the flag could not safely be enabled until
  the project shipped its own TLS story — false for a deployment terminating TLS
  at a reverse proxy, and the proximate cause of this outage. Corrected at all
  three places it appeared. The genuinely-still-true warning (this server ships
  no built-in TLS; plain-LAN HTTP is sniffable) is preserved.

### Added

- **`web_ui.allowed_origins`** — an operator-configured origin allowlist, with
  `IRONHERMES_WEB_ALLOWED_ORIGINS` as the env route for platforms that expose
  only environment variables. When configured, the incoming `Origin` is compared
  against the list and the client-supplied `Host` is ignored entirely; entries
  are validated and canonicalized at startup, and a malformed entry fails the
  boot loudly rather than being dropped into a list that silently matches
  nothing.
- **A fail-closed origin startup guard**, independent of the existing
  password-hash bind guard. Both run before the listening socket opens, and stay
  separate `if` blocks with distinct messages so a refusal always names which one
  tripped.
- **Graceful gateway shutdown on `SIGTERM`.** The gateway handled `SIGINT` only,
  so a container stop, a systemd stop, or `hermes gateway stop` terminated it
  outright — no unwinding, no pid-guard drop, a stale `gateway.pid` left for the
  next start to reconcile. `SIGTERM` now routes through the same orderly path as
  Ctrl-C.
- **Container image ships `skills/`, `optional-skills/`, Chromium, and `vi`**,
  and supervises the web server and gateway under `tini` as PID 1 so `podman
  stop` shuts both down cleanly.

### Changed

- Chromium sandbox guidance for rootless Podman corrected in `CONTAINER.md`,
  which also now documents the origin guard beside the existing password guard —
  the section an operator reads immediately before running `-e IP=0.0.0.0`.

## [0.3.7] — Web UI refinement for actual use (2026-08-30)

Headline: the `iron_hermes_ui` web app is usable end to end. Phase 49.4's planned
work landed, and operator testing then surfaced a run of client-side freezes and
UI gaps that this release fixes.

### Fixed

- **Agents screen froze the browser tab.** `PlatformBindings` ran a `use_effect`
  that read `prior_bindings` (subscribing) and rewrote it unconditionally every
  run. Dioxus 0.7 does not dedupe identical signal writes, so the effect
  re-triggered itself forever — a synchronous busy-loop on the single-threaded
  WASM client, with no console error. The effect now subscribes to the bindings
  resource and `.peek()`s the value it writes.
- **Soul's SOUL.md editor always showed `0 LINES`.** The seed effect captured the
  resolved persona *outside* the effect, so it never subscribed to the fetch and
  never ran when the body arrived. Every per-profile `SOUL.md` being empty on a
  typical install masked this; the ROOT persona exposed it.
- **Soul never auto-selected a profile**, leaving the editor on `LOADING` — same
  captured-outside-the-effect defect in the auto-select effect.
- **Models screen saturated the server.** Its eight provider→model cascades each
  issued a live `/models` request on mount (300+ models, 4-5s under load).
  `list_provider_models` now has a 60s single-flight cache, so eight concurrent
  callers collapse to one upstream fetch.
- **The Agents/Kanban profile drawer eagerly fetched the full model catalog**
  even while closed, and rendered it as a native `<select>` of every model. It
  now fetches only when open and renders a filterable capped datalist.
- **Only the active screen renders.** All 16 screens previously mounted at once,
  running every screen's fetches, polls, and WebGL loops simultaneously.
- **The create-profile wizard rendered unstyled on Soul** — its `kn-modal` styles
  live in `kanban.css`, which is linked per-screen; the wizard now links its own.
- **A valid skill could fail to import with no explanation.** `allowed-tools`
  written as a comma-separated scalar (`allowed-tools: terminal, read_file` — the
  Claude Code convention) failed `Vec<String>` deserialization, which failed the
  *whole* frontmatter parse, which made `parse_skill_md` return `None`, which
  surfaced as the catch-all "Couldn't read a SKILL.md from this source". Both
  YAML shapes are now accepted and normalize to the same list; the field keeps
  its `Option<Vec<String>>` type, so tool enforcement is unchanged.
- **Import errors now say what is actually wrong.** `parse_skill_md_verbose`
  returns a typed `SkillParseError` naming the cause (missing/unterminated
  frontmatter, the offending YAML field, or an unusable name), and the import
  preview reports it for a `SKILL.md` that was read but failed to parse.
  Fetch/read failures deliberately keep the generic message, since naming the
  internal cause there would leak probe detail. `parse_skill_md` keeps its
  `Option` API and delegates, so existing callers are unaffected.

### Added

- **Upload a skill from your own machine.** The import wizard gains an UPLOAD tab
  with a real file picker for a `.zip` bundle or `SKILL.md`.
  `stage_uploaded_skill` stages the bytes under the hub quarantine dir and the
  existing preview→install pipeline takes over, so uploads reuse the same trust
  and install path (and the same write gate, size ceiling, decompression caps,
  and traversal rejection) as every other source. The client-supplied filename
  only selects zip-vs-markdown and is never joined into a path.
- **Editable DEFAULT (root) persona on Soul.** A DEFAULT tab reads and writes the
  master `$IRONHERMES_HOME/SOUL.md` that `PromptBuilder::load_soul_md` loads,
  behind the existing profile write gate.
- **Bind a platform back to the default agent.** The Platform Bindings dropdowns
  gain a `default (root agent)` option; `set_bot_binding` accepts the
  `DEFAULT_BOUND_PROFILE` sentinel (checked before `validate_profile_name`, which
  reserves that name for profile *creation*). Previously, binding to a real
  profile was a one-way door.
- **Quick-pick of known skill directories** in the import wizard's local-path tab.

### Changed

- **Version bump.** `[workspace.package].version` moves from `0.3.5` to `0.3.7`,
  the single source of truth for all 21 workspace members.
- **Platform Bindings is a responsive grid**, wrapping into columns instead of six
  full-width rows that consumed most of the Agents viewport.
- **Soul's SOUL.md editor** is reached from the tab strip; an inline `<select>`
  variant was tried and reverted after it blanked the screen at runtime.

## [0.3.5] — Point release (2026-08-04)

### Changed

- **Version bump.** `[workspace.package].version` moves from `0.3.1` to `0.3.5`,
  the single source of truth for all 21 workspace members.
- **Closed two version-drift channels.** `crates/ironhermes-exec/Cargo.toml` and
  `crates/iron_hermes_ui/Cargo.toml` previously hardcoded their own version
  literal instead of inheriting the workspace version; both now read
  `version.workspace = true`.
- **Login page build stamp now derives from the package version at compile
  time.** The five `BUILD` status bar stamps across the login page themes
  previously had the version hand-typed into the HTML template; they now
  resolve `env!("CARGO_PKG_VERSION")` at build time, matching the existing
  precedent in `sys_meta.rs`.

## [0.3.1] — Realtime voice2voice agent + black-box recording (2026-06-19)

Headline: the web **Free / open-mic voice-to-voice** mode is now a first-class Hermes
agent surface, not a plain conversational model (Phase 39.3). Also lands async
multi-turn concurrency (39.1) and end-to-end black-box recording (39.2).

### Added

- **Realtime voice2voice agent tool & skill wiring (Phase 39.3, D-01…D-05).** The
  browser↔OpenAI realtime session is now configured *as Hermes*: system prompt + active
  skills injected as `instructions` (D-01), the full `ToolRegistry` exposed as realtime
  `tools` with no curated subset (D-02). A server-side function-call execution bridge
  (`realtime_tool_call` / `realtime_approve`) routes every voice-triggered tool call
  through the **same approval / yolo / DEFCON gate** as text turns — no bypass (D-03),
  with an in-orb Approve/Deny card that lets the session keep conversing while a call is
  pending. Transcript + `ironhermes-trajectory` persistence parity with text turns (D-04),
  and a talk-while-working in-flight badge + voiced completion (D-05).
- **Async non-blocking multi-turn (Phase 39.1).** `TurnRegistry` + per-session /
  process-wide concurrency caps (`concurrency.session_turn_cap`,
  `concurrency.global_turn_ceiling`); realtime turns register as `Surface::Realtime`.
- **End-to-end black-box recording (Phase 39.2).**
- **Voice config surface.** `voice.realtime_*` (model, voice, transcription, noise
  reduction, VAD), `voice.web_silence_threshold_rms`, and wake-word settings now persist
  from the voice-settings UI to `config.yaml`.

### Fixed

- **Voice2voice security hardening:** redact tool args before trajectory logging
  (T-LOG-LEAK); validate + bind `session_id` in `realtime_save_transcript` (CR-01);
  drain pending approvals on turn teardown (WR-01); re-validate tools on approve (WR-03);
  parseable trajectory timestamps (WR-04).
- **Voice2voice UAT fixes:** approval card / in-flight badge now reactive
  (Dioxus `.read()` vs `.peek()`); AI voice replies appear in chat (GA
  `response.output_audio_transcript.done` event name); transcript history parity
  (real chat session key, assistant side, live bubbles); the composer **FREE** button is
  no longer disabled by STT availability (open-mic uses the Realtime API, not whisper STT).
- Wake word is disabled with a hint in open-mic mode (applies to turn-based mode only).

### Docs

- ARCHITECTURE / CONFIGURATION / GETTING-STARTED updated for the realtime voice agent;
  example config extended with the new `voice.*` and `concurrency` keys.

## [Unreleased] — `hotfix`: cron briefing/audit reliability (2026-06-12)

Fixes the recurring *"Stopped after 2 consecutive failed delegations"* failures on the
scheduled cron jobs (AI/LLM News Briefing, Atlanta Weather, iron security audit). Root
causes were a tool-name-as-skill misconfiguration and a per-job `model` override that
never reached inference. Full write-ups: `HOTFIX-PLAN-cron-web-search.md`,
`PLAN-cron-model-routing.md`, `MERGE-JUSTIFICATION-cron-web-search-hotfix.md`.

### Fixed (code)

- **Per-job `model` override now reaches inference.** The cron runner computed the
  per-job model but built the client from the provider default, so every cron job ran on
  the global default model regardless of `job.model`. Added
  `build_main_client_with_model` and used it in the runner.
  (`ironhermes-agent/src/any_client.rs`, `lib.rs`, `ironhermes-cron-runner/src/runner.rs`)
- **Reject tool names in a cron job's `skills[]`.** A tool (e.g. `web_search`) is not a
  skill — it's enabled via toolsets. Validation now rejects tool names at `cron create`,
  `cron edit`, and the LLM-facing `cronjob` tool (create/update), with guidance.
  (`ironhermes-tools/src/cronjob_tool.rs`, `ironhermes-cli/src/cron.rs`)
- **`cron edit --clear-skills`.** Previously there was no way to empty a job's skills via
  the CLI (no `--skill` meant "leave unchanged"). (`ironhermes-cli/src/cron.rs`)
- **No more misleading "skill skipped" banner for tool names.** When a job lists a tool
  name as a skill, the prompt builder no longer injects a banner telling the agent the
  capability is unavailable; it omits it (debug-log only). Guards legacy/hand-edited jobs.
  (`ironhermes-cron-runner/src/prompt_builder.rs`)

### Added

- `ironhermes_tools::known_tool_names()` / `tool_names_among()` — canonical built-in
  tool-name set, sourced from the existing toolset membership map (single source of
  truth). (`ironhermes-tools/src/toolset_session.rs`, `lib.rs`)

### Runtime config changes (operational — applied to `~/.ironhermes/`, not in this repo)

These live in `~/.ironhermes/cron/jobs.json` and were applied directly (atomic writes;
backups retained alongside the file). Recorded here because that file is not version-
controlled:

| Job | Change | Reason |
|-----|--------|--------|
| AI/LLM News Briefing | `skills: ["web_search"] → []`; schedule `once → cron "30 12 * * *"`; `model → anthropic/claude-haiku-4.5` | remove tool-as-skill; restore daily cadence (a manual rerun had made it a spent one-shot); capable model |
| Atlanta Daily Weather Briefing | `skills: ["web_search"] → []`; `model → anthropic/claude-haiku-4.5` | same skills mistake; capable model |
| iron security audit | `model → anthropic/claude-haiku-4.5` | was on the slow default; reproduced the delegation-timeout kill |

- `enabled_toolsets` left unset on all jobs → each keeps all toolsets (incl. `web`, `tts`,
  `memory`, files). `web_search` is registered because `FIRECRAWL_API_KEY` is set.
- Model slug `anthropic/claude-haiku-4.5` is routed via OpenRouter (the configured cloud
  provider); verified against OpenRouter's live model list.

### Deploy / ops

- Release binary rebuilt and installed to `~/.local/bin/ironhermes` (backup:
  `ironhermes.bak-pre-option1`); gateway restarted under launchd
  (`com.ironhermes.gateway`).
- Resolved a pre-existing **orphan gateway process** (manually-started PID held
  ironhermes' PID-lock, crash-looping the launchd service and causing intermittent
  Telegram 409 "two instances" errors). The gateway is now a single, launchd-managed
  instance.

### Verification

- `iron security audit` re-triggered post-deploy: every generation ran on
  `anthropic/claude-haiku-4.5` (parent override effective), **no `delegate_task`, no
  timeout/2-strike kill**, completed in ~38s (vs. 2–3 min spiral-and-kill on the old
  default), delivered a real 4-vulnerability report.

### Known follow-ups (not in this change)

- `delegate_task` subagents still inherit the global default model (Option 1 retargets the
  parent only). Moving them requires changing the global default (`config.yaml`) or per-job
  subagent re-registration — see `PLAN-cron-model-routing.md` (Option 2 / 1b).
- Pre-existing, unrelated test failures in `ironhermes-cli/tests/skills_cmd_integration.rs`
  (verified failing on the clean tree).
- Minor: `anthropic/claude-haiku-4.5` missing from the local pricing cache (cost reports
  `$0`); per-job `provider`/`base_url` fields still ignored by the runner;
  `ollama` `num_ctx: 256` in `config.yaml`.
