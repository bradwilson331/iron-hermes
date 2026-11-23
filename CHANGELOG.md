# Changelog

## [0.5.7] — Small-window models, Tier 2 memory, dashboard canvas, security layers, hardened containers (2026-11-22)

A model with a small context window (8,192 tokens on a local server) failed any
long turn with a `context_length_exceeded` 400, because the agent assumed a
128,000-token window and sent a prompt that did not fit. Agent-launched heavy
workloads could OOM-kill the whole gateway, and a delegated turn could end with
no reply. The web UI had no way to arrange its menu, edit `config.yaml` or show
live boards, and the security layers still had gaps around `execute_code`, web
approvals and inbound text. This release closes those, adds Tier 2 semantic
memory, and hardens the container images.

### Added

- **Menu tab in web Settings** (Phase 54.9). Reorder the docked wheel and the
  header shortcuts, move a screen between them, rename labels, and Reset to the
  shipped layout. The result is saved per profile under `ui.menu` in
  `config.yaml` and applies live. The wheel holds 6 to 11 sections; Chat and
  Settings stay on the wheel. A saved layout that fails validation falls back to
  the default with a one-line notice instead of breaking the page.
- **OpenUI screen** (Phase 54.9, `docs/adr/ADR-0007-openui-generative-ui.md`). Type
  a request and the agent answers in OpenUI Lang, rendered in a sandboxed frame
  as it streams. Views are saved per profile (up to 200) and can be re-run. A
  button inside a view sends a new agent turn through the normal tool, guardrail
  and approval gates; a view never calls a tool directly. The Thesys hosted
  Gateway is off by default; `ui.openui_gateway_enabled: true` plus a
  `THESYS_API_KEY` (vault secret `thesys` or environment) turns it on, and
  conversation content then leaves the machine.
- **Dashboard screen** (Phase 54.10). Each profile keeps up to 12 named boards on
  a pannable, zoomable canvas with a soft snap grid. A board holds AI widgets
  (references to saved OpenUI views), native tiles, and notes. Native tiles are
  usage and cost, sessions, kanban, cron, gateway health and reports; they poll
  while the screen is open and each has a refresh button. Start from a role
  preset (Founder, Engineering, Sales, Ops), Blank, or Plan with AI, which shows
  the planned widgets for you to edit and confirm before anything is generated.
  `/` creates a widget at the cursor, `N` adds a note, and Present mode walks the
  board full screen. Widgets move and resize from the keyboard (arrows nudge,
  Shift+arrows resize, Delete asks twice). Below 900 px wide a board is a
  read-only single column. Existing saved menu layouts gain the Dashboard entry
  on upgrade.
- **`report_publish` tool** (Phase 54.10). An agent run, such as a red-team pass,
  publishes a named report (title, summary, severity counts, findings, body).
  Publishing under the same name adds a version (20 kept); a Reports tile shows
  the latest with its change from the previous version. No approval is needed
  because it writes only to the profile's report store, and every publish is
  recorded in `logs/audit.jsonl`.
- **Raw `config.yaml` editor in web Settings** (Phase 54.13). A "Config file
  (YAML)" section edits the root `config.yaml` or any profile's. **Check** reports
  YAML and type errors with a line and column; unknown keys are listed as
  warnings and do not block the save. Inline secrets never reach the browser (they
  show as `•••• (kept)` and are restored on save). Each save writes
  `config.yaml.bak-<UTC time>` beside the file (newest 10 kept), and a Backups
  list loads an older copy into the editor. Saving needs login when web auth is
  on and `security.web_config_write_enabled: true`; with it off, the editor is
  read-only. A save that turns that flag off asks for typed confirmation. After a
  save the banner offers Restart gateway; the web server itself needs a manual
  restart.
- **Gateway cards can be reordered, and Schedules HISTORY opens the last run**
  (Phase 54.10). Gateway screen cards drag (or move by keyboard) and the order is
  saved per profile; the top card spans the full width. The HISTORY button on
  Schedules shows the selected job's last run time, status and error. It says it
  shows the last run only; run history is still not recorded.
- **Tier 2 semantic memory** (Phase 58.2, ADR-0008, `docs/MEMORY-TIER2.md`). A
  per-profile store of short, cited claims under `<home>/tier2/`, beside the
  unchanged `MEMORY.md` / `USER.md`. It adds the `memory_search` and
  `memory_explain` tools, a Tier 2 tab on the Memory screen, hourly claim
  consolidation (`memory_consolidate`), nightly "dreams" and a frozen
  `ALIGNMENT_SYNTHESIS.md` block at session start, dated agent notes
  (`memory_note`), and forgetting (`memory_forget` stages a claim, hiding it from
  search at once). **All of it is off by default:** `memory.tier2_enabled`,
  `memory.consolidation_enabled`, `memory.dreams_enabled`,
  `memory.forgetting_enabled` and `memory.promotion_enabled`. Claim extraction and
  dreams use the `memory` model role. Search uses a built-in 384-dimension
  embedder (downloaded on first use; keyword search until it is ready) or an
  OpenAI-compatible endpoint set under `memory.embeddings`. Staged forgets are
  deleted only when a human runs `ironhermes memory purge` (dry run first) or
  Purge now on the Tier 2 tab; no agent tool and no schedule can purge.
- **Agent jobs that detach run in their own systemd scope** (Phase 58.1, Linux).
  A terminal command using `nohup`, `setsid`, `disown` or a trailing `&` now
  runs in `ironhermes-job-<id>.scope`, outside the gateway's cgroup. It survives
  a gateway restart, and an out-of-memory kill of a render or model server no
  longer takes the gateway down. List with `systemctl --user list-units
  'ironhermes-job-*'`. `IRONHERMES_JOB_SCOPE=off` restores the old behavior. See
  `docs/DEPLOYMENT.md`, "Memory cap and agent job scopes".
- **Compact profile for small windows** (Phase 58.3.1). `agent.compact_profile`
  is `auto` (default), `on` or `off`. When the full system prompt plus tool
  definitions would take more than half the window, the agent sends only
  `terminal`, `read_file`, `write_file`, `patch`, `memory`, `web_search` and
  `web_read` (plus `kanban_complete` and `kanban_block` in a kanban worker),
  drops the skills index, and shortens context files; `SOUL.md` stays whole. The
  first compact turn of a session shows a one-line notice on the TUI, web, ACP
  and chat platforms.
- **`compression.model_thresholds`** maps a model name to a compression trigger
  fraction, for example `qwen2.5-7b: 0.3`. Keys match as case-insensitive
  substrings and the longest match wins; values outside 0 to 1 are ignored with
  a warning.
- **Local servers are asked for their loaded window.** Before the first request
  to a loopback or LAN endpoint, ironhermes asks LM Studio, Ollama and llama.cpp
  how many tokens the loaded model has, and uses the answer.
- **`/models refresh` and `ironhermes models fetch` save the model ids each
  provider serves**, and the `/model` picker lists them without a restart.
- **tirith detection sidecar** (Phase 57). A separately licensed
  `ironhermes-tirith` process (AGPL-3.0, `tirith-core` pinned to 0.4.2) is
  started by every agent host and screens tool arguments, inbound chat text,
  cron and kanban prompts, context files, MCP configuration and URLs. The
  existing regex tables keep running and the strictest verdict wins, so they are
  the floor when the sidecar is down. No MIT binary links `tirith-core`.
  `IRONHERMES_TIRITH_BIN` points at a non-default binary; `ironhermes doctor`
  reports the sidecar's presence, pin and a live ping.
- **DEFCON is one dial, set per session with `/yolo <N>`** (Phase 57). `N` is 1
  (strictest) to 5 (most permissive); `security.defcon_level` is the default for
  new sessions. `/yolo <N>` works on the TUI, CLI, web, Telegram, Discord and
  Slack, lasts for the session, is never written to `config.yaml`, and every
  change is audited. A remote surface needs `security.defcon_remote_change_enabled:
  true`; see `docs/CONFIGURATION.md`, "DEFCON levels and `/yolo <N>`".
- **Real approvals on the web** (Phase 57). A command that needs approval shows
  an Approve / Deny card in chat; before this the web gate denied every such
  command.
- **The web chat shows only critical events** (Phase 57). Routine tool traffic
  collapses into one status dot per turn that spins while tools run, turns amber
  when a warned or bypassed call ran, and red on a block or failure. Blocks,
  approvals, credential detections, failed jobs, sidecar outages and `/yolo`
  changes still show. The Logs screen stays complete, and the separate yolo-bypass
  chip is gone.
- **Approvals on Discord and Slack.** A command that needs approval sends the
  prompt to the requester's DM, even when it came from a channel. Only the
  requester's `/approve` or `/deny` from that DM resolves it. In a channel the bot
  answers "Approvals are answered in a DM to the bot", and another user cannot
  approve someone else's request. A timeout denies. Before this, Discord and Slack
  refused every such command with no prompt. Slack needs the `im:write` and
  `chat:write` scopes.
- Image `HEALTHCHECK` on `/`, OCI labels, digest-pinned base images, and cosign
  signing of published images (see `docs/DOCKER-PUBLISH.md`).
- Container lint Gate 11 (checks the compose profile and Dockerfile).
- `IRONHERMES_UID` / `IRONHERMES_GID` remap now works in the web image under a
  read-only rootfs.
- `IRONHERMES_CONFIG_TEMPLATE` seeds `config.yaml` once from a file you name.
- `IRONHERMES_BROWSER_NO_SANDBOX` environment override for `browser.no_sandbox`.
- Optional Cloudflare Tunnel hosting: `docker compose --profile cloudflare up -d`
  starts a hardened, digest-pinned `cloudflared` connector in front of the UI
  (`docs/DEPLOYMENT.md`).

### Changed

- **BREAKING: the read-only rootfs turns apt-style writes into EROFS.** Under the
  compose default (`docker-compose.yml`), `apt install`, `apt-get`, system-wide
  `pip install` and any write to `/etc` or `/usr` fail with "read-only file
  system". Agent workflows that installed system packages must install into the
  volume instead (`npm install -g` lands in `/opt/data/.local`; the Python user
  base is pointed there too, but the image ships no `pip` or `venv` module, so
  Python packages need a pip you bring yourself) or use a derived image. The tmpfs `/tmp` is `noexec`,
  which blocks direct execution from `/tmp`.
- **Chromium runs with `--no-sandbox` by default in both images**
  (`IRONHERMES_BROWSER_NO_SANDBOX=1`). Chromium's own sandbox crashes under the
  hardened compose profile; the container's controls (UID 10000, no
  capabilities, `no_new_privs`, read-only rootfs) are the compensating boundary.
  Run with `-e IRONHERMES_BROWSER_NO_SANDBOX=0` to restore the sandbox where your
  runtime allows it.
- `setpriv` replaces the old privilege-drop helper. The drop happens inside the
  image, fails closed, and holds on hosts that cannot apply the compose runtime
  flags.
- `docker compose up -d` with `docker-compose.yml` is now the documented default
  run. Plain `podman run` remains as the unhardened alternative.
- **Sliplane runs the same single `-dev-tools` image** (D-02 option-a: there is
  no Sliplane-specific Dockerfile). The in-image enforcement (uid 10000,
  `no_new_privs`, empty bounding set) applies with no runtime flags, proven by
  the `sliplane_equivalent` smoke case. Whether Sliplane applies the compose
  runtime flags (read-only rootfs, noexec tmpfs, `shm_size`, `cap_drop`,
  pids/memory/cpu limits) was **not probed** and is treated as unverified
  residual risk (`docs/DEPLOYMENT.md`, "Sliplane probe results").
- **The context window is resolved from more sources, in a fixed order** (Phase
  58.3.1): the per-model override `providers.<p>.models.<m>.context_length`, then
  a window the model server reported or an earlier rejection taught
  ironhermes, then the models cache or static table, then `model.context_length`,
  then the default. `model.context_length` is now a ceiling for a probed or
  learned window (the smaller wins), not an override. An unknown model on a
  loopback or LAN server now defaults to 8,192 tokens instead of 128,000. See
  `docs/CONFIGURATION.md`, "Context window and small models".
- **Small windows get a tighter compression trigger.** Below 32,768 tokens the
  trigger drops automatically (0.35 at 8,192). `compression.model_thresholds`
  sets a per-model fraction that overrides it. Summaries are sized to the
  window, so a summary cannot overflow a small model. Defaults for large windows
  are unchanged.
- **Requests to small windows send `max_tokens`.** When the compact profile is on
  or the window is 32,768 tokens or less, the request carries a reply reserve
  (1,024 tokens at 8,192, up to 4,096). Large-window requests are unchanged.
- **A request that still does not fit is not sent.** The agent compresses up to
  three times, then refuses the turn with a message that names the window and
  how to fix it, instead of sending a request the provider will reject.
- **The window is re-resolved on a model switch** (`/model`, the picker, a
  fallback provider) instead of once at startup.
- **Web: Settings and Models show the window and where it came from**
  (`override`, `cache`, `pin`, `default`, `probed`, `learned`, `local default`).
  `/usage` prints the same.
- **BREAKING for `autonomous.yolo` and `skills.defcon_level`:** both keys still
  load but are deprecated aliases for `security.defcon_level` and log a warning
  (`autonomous.yolo: true` reads as level 5, `false` as 3). An install that sets
  none of them runs at level 3, so approvals stay on. Bypass of a warned call
  exists only at levels 4 and 5.
- **`execute_code` source is screened** (Phase 57). At level 3 or stricter, code
  that reaches `subprocess`, `socket`, `ctypes` or `importlib` is blocked.
  A reconstructed call to a known mutation command, such as `wrangler delete`
  through `subprocess`, is blocked at every level. A real `execute_code` sandbox
  is still not built and is listed as an accepted risk in `docs/SECURITY.md`.
- **`web_read` and `web_extract` ask for approval at levels 1 and 2 only.** At
  levels 3 to 5 a clean URL runs; a critical URL is blocked at every level.
- **MCP mutation tools are judged on their arguments**, not just their name, and
  `delegate_task` subagents go through the same gate on every surface.
- **The gateway and web units are memory-capped** (Phase 58.1, Linux).
  `ironhermes-gateway.service` and `ironhermes-web.service` ship with
  `MemoryHigh=6G`, `MemoryMax=8G` and `OOMPolicy=continue`, and the gateway unit
  sets `KillMode=control-group` so everything left in its cgroup dies with it.
  The gateway itself uses about 137 MB. `install.sh`, `update.sh` and
  `web-install.sh` overwrite the installed unit; put overrides in a
  `systemctl --user edit` drop-in.

### Fixed

- **Gateway yolo-bypass chat spam.** Yolo bypass notices are now log-only on the
  gateway: zero bypass messages reach Telegram, Discord or Slack chat (target
  `ironhermes::yolo` in the log). The audit rows are unchanged.
- DEFCON wording is removed from gateway chat notices (the audit log keeps the
  raw wording).
- **The sidecar killed for timeouts now fails closed.** A sidecar killed after
  repeated timeouts reports `killed_for_timeouts` for the whole backoff, so
  subjects are Blocked at DEFCON 1-3 and Warned at 4-5 instead of passing on the
  regex floor. The kill backoff is a flat 0.5 s, and timed-out checks keep
  counting toward the queue budget (plan 57-24).
- **A Block on the web card names its cause:** a named rule, the built-in
  rules, "could not be screened (<reason>)" or "the security scanner is not
  responding". A subject the sidecar refused no longer reads as a scanner outage
  (plan 57-25).
- **`/yolo 4` and `/yolo 5` now take effect on the web and in chat.** The model
  is told the session's live DEFCON level on every turn, so after a refusal it
  makes the call again and the gate bypasses it (audited). Before this, only the
  CLI worked, because it puts every slash-command result into the model's
  history. Blocks are still never bypassed.
- **Context files are screened at the session's DEFCON level,** not the process
  default, and their audit rows record that level and the session id.
- **A context-window 400 no longer ends the turn** (Phase 58.3.1). On a
  `context_length_exceeded` rejection the agent reads the real window from the
  error, compresses to fit, and retries once. The learned window is saved in
  `models-cache.json` (not `config.yaml`) per server URL and model. It covers
  every surface that runs the agent loop and the helper-model calls (summarizer,
  vision, and the other auxiliary roles). If the retry also fails, the error
  names the window and the fix.
- **Gateway hygiene no longer leaves orphaned tool calls** when it drops the
  middle of a long conversation to fit a small window.
- **Switching to a provider that has no default model no longer sends an empty
  model id.** A blank model is refused with a message pointing at `/model` and
  `/models refresh`, and the picker's empty list says the same.
- **Delegated turns finish and reply** (Phase 58.1). A parent turn now aborts a
  delegate child that blocks past its deadline, and a delegate waiting for a free
  slot gives up at that deadline with `Subagent timed out after Ns waiting for a
  delegate slot`, instead of waiting for ever and leaving the chat on "typing".
  The model-supplied `timeout_seconds` is not clamped.

### Security

- **A Block always beats yolo** (Phase 57). A tirith Critical, a Tier-2 command
  such as `rm -rf /`, or a recognised mutation verb is blocked at every level.
  A Tier-1 command or a Python-table hit such as `curl` asks for approval at
  levels 2 to 5 and is blocked only at level 1.
- **A missing, crashed or timed-out sidecar fails closed for tools that change
  things.** `terminal`, `execute_code`, `delegate_task`, `cronjob`, `write_file`,
  `patch` and MCP mutation tools are blocked at levels 1 to 3 and raised to Warn
  at 4 and 5; each event is audited with its reason.
- **Inbound chat text and cron or kanban prompts are screened** for injection and
  hidden characters.
- **Secrets in `config.yaml` stay out of the browser** (Phase 54.13). The raw
  editor masks any key ending in `api_key`, `token`, `secret`, `password` or
  `passphrase` (and similar credential names), and a save is refused rather than
  written with a placeholder it cannot match to an original. Backups are mode
  0600.
- **Tier 2 memory claims pass the memory scanner** (Phase 58.2). Claims come only
  from user and assistant messages, never tool output or fetched pages, and a
  rejected claim is logged to `audit.jsonl`.

## [0.5.0] — Telegram stall fix, site deploys from chat, skills in installs (2026-10-17)

A long-running Telegram gateway could stop answering without ever reporting a
fault, MCP servers that need a browser sign-in could not be authorized from a
headless install, and bundled skills only reached a checkout, not an install.
This release closes all three and adds the first piece of the site-deploy
workflow.

### Fixed

- **The Telegram gateway no longer stalls silently.** Every Telegram HTTP call
  now has a bounded connect and request timeout, the long-poll loop has a
  stalled-poll guard, and an in-process watchdog restarts a poller that stops
  making progress. `status` reports what the poller is actually doing instead of
  the last state it remembers, and a 10-minute poll trace is kept so a stall can
  be diagnosed after the fact.
- **A hung turn can no longer wedge a chat.** Each turn runs under a deadline,
  and the cleanup after it (placeholder edits, typing indicator, queue drain) is
  bounded as well, so one unresponsive adapter call cannot hold the session.
- **The gateway writes a daily log file on every install**, not only under the
  supervisor scripts, with a 14-file retention.
- **Headless MCP authorization works.** A server that needs a browser sign-in can
  be authorized from the CLI or from Telegram by pasting the redirect URL back;
  no local browser is required.
- **`auth.json` is safe across processes.** Concurrent writers no longer lose
  each other's tokens.

### Added

- **Site deploys from chat.** A site registry (`ironhermes sites ...`), a
  `doctor` preflight for each registered site, and a `site-deploy` skill that
  builds and deploys a static site and reports the result back to the chat.
- **A dev-tools container image** (`docker/dev-tools.Dockerfile`) layered on the
  base image with git, the GitHub CLI, Node 22, npm and a pinned wrangler. The
  base image stays lean.
- **The wrangler skill is no longer gated** behind an extra toggle.
- **Skills and optional skills are installed and upgraded by `install.sh` and by
  the container** from the release archive. A history manifest
  (`skills/.bundled-history`) lets an upgrade recognize every pristine bundled
  version and replace it, while operator edits are kept.

### Security

- **Token-free Telegram errors.** Bot tokens no longer appear in error text or
  logs produced by Telegram transport failures.
- **OAuth values are redacted in logs.** Authorization codes, state values and
  tokens are masked wherever an MCP authorization flow logs a URL.
- **`${VAR}` in MCP headers fails closed.** An unset variable referenced from a
  server header or URL now refuses to connect instead of sending an empty or
  partial value.

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
