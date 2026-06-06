---
gsd_state_version: 1.0
milestone: v3.0
milestone_name: Hermes-agent parity
status: executing
stopped_at: Phase 37.1 context gathered
last_updated: "2026-06-06T20:44:36.015Z"
last_activity: 2026-06-06
progress:
  total_phases: 61
  completed_phases: 26
  total_plans: 138
  completed_plans: 132
  percent: 43
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-11)

**Core value:** A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.
**Current focus:** Phase 37.1 — setup-script-not-working-on-macos

## Current Position

Phase: 37.1 (setup-script-not-working-on-macos) — EXECUTING
Plan: 5 of 6
Status: Ready to execute
Last activity: 2026-06-06
Next: plan urgent Phase 37.1 (setup script not working on macos) — /gsd:plan-phase 37.1

## Recent close-out summary (2026-06-06 — Phase 37 CLOSED)

**Phase 37 (RUSTSEC-2026-0104 reachable-panic remediation + v0.2.0 bump):** 2 plans, 2 waves, both complete. Wave-based execution with worktree isolation, merged to `develop` (HEAD after merges `1457f449`).

- **Plan 01 (Wave 1 — security remediation):** Closed the actively-exploitable Chain 2 (`reqwest`/`hyper-rustls`/`slack-morphism`/`chromiumoxide` → rustls 0.23.x) reachable CRL-parse panic (DoS, CVSS 7.5) by forcing `rustls-webpki` 0.103.10 → **0.103.13**. **Documented deviation (Rule 1):** the plan specified a `[patch.crates-io]` block, but Cargo rejects same-registry patches ("patches must point to different sources"); the executor used the correct mechanism — `cargo update -p rustls-webpki@0.103.10 --precise 0.103.13` (pin in `Cargo.lock`) — and kept a `RUSTSEC-2026-0104` audit-comment block in root `Cargo.toml` (SEC-05 grep anchor). Chain 1 (`serenity 0.12.5` → tokio-tungstenite 0.21 → rustls 0.22.4 → rustls-webpki **0.102.8**) is semver-unpatchable (no serenity 0.13.x) → **risk-accepted + documented**, re-evaluate when serenity 0.13.x ships. Commits `572ee94e`→`cbcf1f6b`.
- **Plan 02 (Wave 2 — release boundary):** Workspace version `0.1.0 → 0.2.0` in the 3 version-bearing Cargo.toml files (root `[workspace.package]` + hardcoded `iron_hermes_ui` + `ironhermes-exec`); ~15 `version.workspace=true` crates inherit; CLI `--version` reads `env!("CARGO_PKG_VERSION")` (no source edit). Security pin preserved through the bump. Commits `041a124a`→`0da6e891`.

**Gate results:**

- SEC-01 (0.103.10 absent), SEC-02 (0.103.13 present), SEC-05 (RUSTSEC comment): **PASS** via `Cargo.lock` + grep. `0.102.8` present = expected Chain-1 exemption.
- SEC-03: `cargo build --workspace` **exit 0** (post-merge integrated tree; 100m cold build under sccache).
- VER-01/02/03 (`version = "0.2.0"` ×3), VER-04 (`--version` via `CARGO_PKG_VERSION`): **PASS** (committed grep + structural + Wave-2 executor worktree `--version | grep 0.2.0` green).
- SEC-04 (no new TLS failures): satisfied by Wave-1 executor's green `cargo nextest run --workspace` + post-merge build exit 0. The post-merge full-suite nextest re-run was **terminated as inconclusive — environmentally dominated** by network/browser integration-test hangs (`hub_search_test`, `browser_prereq`, `sse_provider_error_stream`, `extra_request_options`, `client_invariant`, `shrike_service`) blocking on I/O past `slow-timeout` (which was NOT reaping them — procs seen at 17:38 elapsed vs ~4min terminate-after) plus host saturation. None are rustls/TLS/webpki-related; not a regression from this phase.

**Side finding — macOS codesign mitigation (36.17.7) CONFIRMED working:** during the post-merge nextest, freshly-built test binaries produced split `.dSYM` bundles (`deps/*.dSYM` 3 → 6) and launched with **zero `codesign` processes** throughout the run phase — i.e. the syspolicyd/amfid first-launch serialization stall is gone for binaries built under `[profile.dev] debug = "line-tables-only"` + `-Csplit-debuginfo=packed`. The large 170–296 MB binaries observed earlier were confirmed **stale pre-mitigation artifacts** (full `debug=2`; "rebuilt since build start: 0"), not products of the current config. `RUSTFLAGS` unset, so the macOS `cfg` rustflags block is active.

**Operational note (sccache):** the 100-minute cold workspace build + the nextest test-target rebuild were dominated by **sccache overhead** on a partly-cold cache (low aggregate CPU ~1.5 cores across 36 rustc procs, all waiting on the wrapper). sccache disabled at user request after close-out (commented `rustc-wrapper` in `~/.cargo/config.toml`). Note: removing the wrapper invalidates cargo fingerprints → next build is a one-time full rebuild.

Counter update: `completed_phases: 26 → 27`; `completed_plans: 132 → 134`; `percent: 43 → 44`.

## Recent close-out summary (2026-06-05 — Phase 36.17.7 CLOSED, partial)

**Phase 36.17.7 (Gateway + web + TUI runtime TTS wiring):** Closes the 36.17.5 D-15 deferral — `register_tts_tools` was guarded behind `if let Some(ref session_key)` while `AgentRuntime::from_config` hard-coded `session_key: None`. Now wired across all three surfaces. **5 plans, 3 waves, all complete.**

- **Plan 01 (Wave 1 — foundations):** `TtsPerTurnWiring` struct + `TurnRequest.tts_wiring` + per-turn `register_tts_tools`; `#[derive(Default)]` on `AppRuntimeFactoryInput` + `from_config` rewrite removing the `session_key: None` hard-code (BLOCKER 1); `NotSupportedAudioDispatcher` stub (D-03-b). Commits `da53b83a`→`3e145586`.
- **Plan 02 (Wave 2 — gateway/Telegram):** `telegram_audio_dispatcher` field + `tts_wiring` threading; AudioDispatcher wired on all 3 gateway start paths (Telegram real + Discord/Slack NotSupported stubs, HIGH 6). Commits `f3aa5d66`, `07be9be5`. UAT-T approved.
- **Plan 03 (Wave 2 — TUI):** per-turn `TtsPerTurnWiring` into TUI `spawn_turn` (Platform::Local + rodio). Commit `2967aa2e`. UAT-TUI approved.
- **Plan 04 (Wave 2 — web):** `ChatStreamEvent::AudioOut` protocol + `WebAudioDispatcher` + binary WS frame + Blob URL first-play + inline `<audio controls>`. Commits `9ddfcf00`, `650c28b2`. UAT-W approved (after Web-arm hotfix below).
- **Plan 05 (Wave 3 — audio cache + Registered column):** `AudioCacheConfig` + `GET /audio/:uuid` replay route (T-path-traversal mitigated) + audio_cache GC (startup + periodic) + D-06 `Registered` column (Live/Inspection/—, Path B) + `invariants_36_17_7.rs` (5 D-05 source-grep guards). Commits `ca83fd17`→`5fb94272`.

**Post-Plan-04 hotfixes (this session):** `247e7327` added the missing `Platform::Web` arm to `send_audio_tool.rs` (the dispatcher was wired but the tool match fell through — this is what made web voice actually play); `be26f48e` installed the rustls `aws-lc-rs` CryptoProvider for Edge TTS WSS; `0cc0a396` migrated the `workspace-tests` CI job to `cargo nextest run --workspace --all-features` (+ separate `cargo test --doc`) and fixed a latent `tts_registry.rs` `&PathBuf`→`&Path` trait break that nextest surfaced.

**Outcome:** Voice TTS works end-to-end on **all three** surfaces (Telegram, TUI, Web). Full `cargo nextest run --workspace` GREEN. A6 regression sweep GREEN (no 36.17.5/36.17.6 regression).

**Deferred (accepted by user — partial close):** `/toolset list` slash command does not display the `voice` toolset as `Live` in running surfaces (reads `—`/`Inspection`) — a display/observability gap, not a capability gap, caused by the slash-dispatch handle reading a different registry instance than the per-turn `register_tts_tools` mutates. Root cause + suggested fix in `.planning/phases/36.17.7-gateway-web-tts-runtime-wiring/deferred-items.md`. Also deferred: ~25 pre-existing workspace compiler warnings (out of scope; separate cleanup PR — CI `-D warnings` lint job needs them before going green).

**Workspace hygiene:** untracked 17 stray `.DS_Store` files (already gitignored but committed pre-rule). Counter update: `completed_phases: 26 → 27`; `completed_plans: 129 → 133`; `percent: 43 → 44`.

## Recent close-out summary (2026-06-03 — Phase 36.17.2.2 plans 01-06 + 07 Task 1 merged, awaiting UAT)

**Phase 36.17.2.2 (`<MEDIA: ...>` media delivery + MarkdownV2 final-text rendering):** 6 production plans merged to `develop` sequentially in DAG order; plan 07 Task 1 (live UAT runbook) merged; plan 07 Task 2 (operator runs 9 scenarios on a live Telegram bot + replies `approved`) deferred to a follow-up session.

**Wave-by-wave commit roll-up (final HEAD `2dbcb041`):**

- **Plan 01 (TDD — MarkdownV2 escape, D-04):** 3 commits → `321d7b33` RED golden test table, `c81316de` GREEN `escape_markdown_v2` + `escape_outside_code_blocks` (fence/inline-code/link-URL state machine, backslash respect), `b7f767d0` SUMMARY. Tests: 16/16 markdown_v2. Em-dash + emphasis-marker corrections applied per plan's catch-all branch (documented as Rule 1 deviations).
- **Plan 02 (TDD — MediaTagExtractor, D-05/D-06/D-08/D-09):** 3 commits → `82cb360a` RED tests, `9d09881e` GREEN `MediaTagExtractor` + `MediaSource`/`MediaKind`/`MediaRef` + lib.rs wire, `fe32db13` SUMMARY. Tests: 21/21 media_tag. Diverges from `StreamingContextScrubber` only on `flush_tail` policy (emits buffered tag-text as VISIBLE per user-trust contract; scrubber discards).
- **Plan 03 (MediaSender trait + Telegram prompt, D-17/D-18):** 3 commits → `e4e31bb2` `MediaSender` trait declaration with one-import re-export, `bfea82ec` prompt_builder.rs teaches `<MEDIA: path|url>` convention, `daf735a9` SUMMARY. Tests: 408/408 ironhermes-agent lib regression clean.
- **Plan 04 (telegram.rs / adapter.rs reconciliation, D-01/D-02/D-12/D-13/D-14/D-15/D-18):** 5 commits → `7a65f348` `send_file_multipart -> Result<MessageResponse>` + new `send_audio` per D-13, `c6fda132` `edit_message_markdown_v2` rename across trait+impls + D-02 single-retry-as-plain-text fallback, `64754747` stream_consumer.rs:100 call-site rename, `8e75c641` `impl MediaSender for TelegramAdapter` with D-12 URL-form + D-13 5-type dispatch + D-14 ogg/opus→voice + D-15 size pre-check + T-INPUT-MEDIA-PATH canonicalization + T-INPUT-MEDIA-URL scheme gate + T-LOG-LEAK filename-only logging, `00d66b91` SUMMARY. **Rule-3 fix-up applied** (Blocking): plan enumerated only 3 test files for the trait rename, but discord.rs / slack.rs / user_queue.rs / stream_consumer.rs::MockAdapter all impl `PlatformAdapter` and required the rename — auto-rippled. Tests: 202+ in gateway, 0 fails.
- **Plan 05 (D-01 final-text rendering complete, D-01/D-03/D-04):** 3 commits → `ae30a075` `send_message_markdown_v2` trait method + Telegram impl with D-02 fallback (added to 4 test-fixture adapters too: RecordingFailingAdapter / RecordingPlatformAdapter / FailingPlatformAdapter / GW-05 RecordingPlatformAdapter), `595df71a` `escape_outside_code_blocks` applied at stream_consumer.rs:100 final-edit AND both overflow chunk sites at :121-128 / :131-134, `31a58713` SUMMARY. D-03 invariant preserved: intermediate edits at :114, :161, :176 stay plain text. Tests: 13/13 stream_consumer including renamed `test_final_edit_uses_edit_message_markdown_v2` + new positive `send_plain_count == 0` regression guard.
- **Plan 06 (handler wire-up + E2E integration test, D-07/D-08/D-09/D-10/D-11/D-15/D-18/D-19/D-20):** 6 commits → `cdd83fa2` `GatewayMessageHandler.set_media_sender` field+setter (constructor unchanged, 5 call sites unbroken), `6e7259ba` MediaTagExtractor chained in `run_agent` stream_callback alongside scrubber (scrubber→extractor order per Open Q5), `d8c01a67` D-19 dispatch loop at CORRECTED anchor `handler.rs:1532` between `typing_handle.await.ok()` (:1515) and `match agent_result {` (:1590) — strict awk ordering check passes (HIGH-RISK T-D19-ANCHOR-MISPLACEMENT mitigated), `88c5905c` runner.rs Telegram start path clone-cast wire-up (no trait upcasting), `216efc5e` `tests/telegram_media_delivery.rs` integration test with `RecordingMediaAdapter` + 7 named scenarios (text_only_v2_edit_renders_with_escape, single_photo_text_then_attachment_order, multi_tag_emits_in_stream_order, missing_path_reinserts_tag, oversize_path_reinserts_without_upload, tag_inside_fence_passes_through_no_attachment, parse_mode_400_retries_as_plain_text), `86597e13` SUMMARY. **Synthetic-LLM injection choice:** drove the production composition directly (`scrubber → extractor → dispatch_media_d19` helper) rather than building a `FakeStreamingProvider` — `AgentRuntime::for_tests()` is feature-gated and uses `localhost:0` which cannot stream. **Reinsert-body propagation choice:** `tokio::sync::oneshot::channel<String>` from consumer task to parent (the StreamConsumer is moved into the spawn via `async move` so the accessor alone is unreachable). Tests: 7/7 telegram_media_delivery, 209 total gateway crate.
- **Plan 07 Task 1 (live UAT runbook):** 1 commit → `8851c147` `crates/ironhermes-gateway/tests/telegram_media_uat.md` (221 lines, 9 scenarios covering D-01/D-02/D-07/D-09/D-10/D-11/D-12/D-14/D-15, sign-off checklist with 9 unchecked boxes, document history). Mirrors `session_queue_telegram_uat.md` template per PATTERNS §12 (36.17.2 D-22 protocol inherited). Task 2 (operator runs UAT + replies `approved`) DEFERRED.

**Pre-work YAML fix (`2da0ded3`):** plans 03 and 05 had `depends_on` entries with trailing YAML comments (`- 36.17.2.2-02   # MediaRef...`) which the gsd-sdk plan-index parser treated as part of the dependency ID, breaking DAG resolution and forcing plan 03/05 into wave 0. Moved comments to dedicated comment lines, restoring correct wave ordering — future plans benefit.

**Phase-level must_haves status:**

- Items 1-4 (build/test/grep): GREEN (markdown_v2 16/16, media_tag 21/21, gateway full crate 209/0/1, telegram_media_delivery 7/7).
- Items 5-8 (source assertions): GREEN — `grep -c 'fn set_media_sender' handler.rs` = 1, `grep -c 'take_attachments' handler.rs` = 1, `grep -c 'MediaTagExtractor::new' handler.rs` = 1, `grep -c '\.canonicalize()' telegram.rs` = 3, `impl MediaSender for TelegramAdapter` = 1, `escape_outside_code_blocks` at stream_consumer = 3, `rg fn edit_message_markdown crates/ironhermes-gateway | grep -v _v2 | wc -l` = 0, `rg '\.edit_message_markdown\(' crates/ironhermes-gateway | wc -l` = 0.
- Item 9 (UAT runbook exists): GREEN — 221 lines, 9 scenarios.
- Item 10 (no D-03 regression): GREEN via stream_consumer's renamed test pass + 36.17.2 prior-phase tests untouched.

**Phase-exit gate still open:** plan 07 Task 2 — operator runs 9 scenarios on a live Telegram bot + replies `approved`. Setup checklist documented in the runbook's Prerequisites table (TELEGRAM_BOT_TOKEN from @BotFather, IRONHERMES_HOME=/tmp/uat-36.17.2.2-home, test chat with bot admin, pre-created `/tmp/uat-{photo.png,voice.ogg,music.mp3,doc.pdf,oversize.png}` test fixtures, one reachable public PNG URL). On `approved` reply, executor will flip `nyquist_compliant: true` + `wave_0_complete: true` in VALIDATION.md frontmatter and counter `completed_plans: 121 → 122`.

**Workspace test post-merge gate:** `cargo test --workspace` exit 0 with 2 pre-existing FAIL lines in kanban end_to_end tests (`duplicate_completion_is_rejected`, `full_lifecycle_via_tools_layer`) — both untouched by this phase (zero kanban files modified across plans 01-07). Failures pre-date this phase; tracked separately for follow-up. Plan 06 SUMMARY documents this in its "Deferred Issues" section. Counter update: `completed_phases` unchanged at 23 (phase 36.17.2.2 not yet fully closed); `total_plans: 119 → 126` (+7 for new phase); `completed_plans: 115 → 121` (plans 01-06 fully closed; plan 07 Task 1 only — plan 07 stays open until operator approval).

## Recent close-out summary (2026-05-30 — Phase 36.3.7.6 closed PASS)

**Phase 36.3.7.6 (Kanban LLM-tool surface completion — heartbeat / link / unblock):** Single plan closed PASS 2026-05-30. 7 commits on develop (5 implementation/test + 1 docs + 1 SUMMARY + 1 state close-out = this commit). Bilateral-tracing satisfied per LEARNINGS 2026-05-30 — every BUG ships producer + consumer in the same commit set: BUG-01 producer (`tools/heartbeat.rs::KanbanHeartbeatTool` + `tools/mod.rs::register_kanban_tools` registration) + consumer (`tests/tools_smoke.rs::kanban_heartbeat_appends_event_row` + `kanban_heartbeat_missing_task_id_errors_without_env`) in commit `4a11d30c`; BUG-02 producer (`tools/link.rs::KanbanLinkTool` + `store.rs::insert_link_checked` (WITH RECURSIVE + BEGIN IMMEDIATE) + `error.rs::KanbanError::LinkCycle` variant + registration) + 3 consumers (`kanban_link_happy_path_inserts_row` + `kanban_link_rejects_cycle` + `kanban_link_phantom_id_rejected`) in commit `3530f52a`; BUG-03 producer (`tools/unblock.rs::KanbanUnblockTool` handler-side gate + registration; `store.unblock_task` UNCHANGED) + 2 consumers (`kanban_unblock_happy_path_from_blocked` + `kanban_unblock_rejects_wrong_status`) in commit `cfb7c144`; D-cli-heartbeat-parity producer (`KanbanCommands::Heartbeat` variant + dispatch arm + `cmd_heartbeat` fn) + 2 consumers (`heartbeat_verb_parses_with_id_only` + `heartbeat_verb_parses_with_id_and_note`) in commit `78f798db`; BUG-05 producer (4 line edits in `docs/kanban/reference.md`) + consumer (verifier grep gate `grep -c 'deferred to Phase 36.3.7.1' = 0`) in commit `11ed521e`. All 8 locked CONTEXT decisions materially observable at file:line — D-heartbeat-impl (event row not column at tools/heartbeat.rs::execute), D-link-cycle-detection (WITH RECURSIVE in store.rs::insert_link_checked), D-link-fk-enforcement (SELECT id pre-check + TaskNotFound mapping in tools/link.rs), D-unblock-status-precondition (handler-side `store.get_task` + status check in tools/unblock.rs::execute; store.unblock_task signature byte-stable), D-cli-heartbeat-parity (cli/kanban/mod.rs + commands.rs), D-tool-surface-mounts-store-directly (3 new structs each carry `Arc<TokioMutex<KanbanStore>>` directly; KanbanStoreWriter trait NOT extended), D-docs-reconciliation (`grep -c 'deferred to Phase 36.3.7.1' = 0`), D-no-deferred-subverbs-change (handlers.rs:1143-1148 byte-stable; still 24 entries; `"link"` + `"unblock"` still deferred). All scope-fences upheld: no gateway-side slash arms wired; no new `KanbanEventKind` variants (both `Heartbeat` at events.rs:47 and `Unblocked` at events.rs:36 pre-existed); no external crate deps added (WITH RECURSIVE is native SQLite syntax). **Crate-isolation fence STILL HELD** — `grep -cE '^ironhermes-gateway\s*=' crates/ironhermes-kanban/Cargo.toml` returns 0 lines. **One Rule-3 fix-up cycle applied:** the plan's `tests/heartbeat_cli.rs` example included `#[derive(Parser, Debug)]` on `TestCli` and `#[derive(clap::Subcommand, Debug)]` on `TestSub`, but compile failed with `error[E0277]: KanbanCommands doesn't implement Debug`. Pre-existing `KanbanCommands` enum (Phase 36.3.7 baseline) does not derive Debug; adding it would be a cross-cutting out-of-scope edit. Fix: drop Debug from TestCli + TestSub derives; replace `{other:?}` panic-format strings with plain string literals. Parse-shape assertions unchanged. Scope-fenced. Counter update: `completed_phases: 18 → 19`; `total_plans: 79 → 80`; `completed_plans: 79 → 80`; `percent: 35 → 37`. The v1 promise of the 9-tool LLM surface is now honored end-to-end. **Forward note:** Q5 (cycle path in rejection JSON) deferred to a future phase if LLM-debuggability surfaces a need. Slash-arm gateway dispatch for `/kanban heartbeat|link|unblock` still deferred per D-no-deferred-subverbs-change. See `36.3.7.6-01-SUMMARY.md`.

## Recent close-out summary (2026-05-30 — Phase 36.3.7.5 verifier PASS)

**Phase 36.3.7.5 (Gateway notifier — auto-subscribe + polling loop + notify-* verbs):** All 4 plans closed PASS individually; gsd-verifier phase-level audit run on 2026-05-30 with verdict **PASS** (13/13 phase-level gates PASS = 12 PLAN gates + 1 implicit crate-isolation gate). Bilateral-tracing 7/7 BUGs cite both producer + consumer in same plan SUMMARYs per LEARNINGS 2026-05-29. All 9 locked CONTEXT decisions materially observable at file:line. All 13 scope-fences upheld. Keystone test `auto_subscribe_lifecycle_end_to_end` (`crates/ironhermes-core/tests/handlers_kanban_notify.rs:452`) PASS — drives full cross-crate pipeline (dispatch `/kanban create` → auto-subscribe row → Completed event → `run_notifier_tick` → mock send_fn invoked → subscription row removed). Phase-relevant test suites re-run by verifier: kanban `notifier_logic` 5/5 PASS (Gate 3), gateway `notifier_spawn_gating` 3/3 PASS (Gate 4), core `handlers_kanban_notify` 9/9 PASS (Gates 7+8 incl. `dispatch_kanban_create_with_json_skips_auto_subscribe`). `cargo build --workspace` exit 0 (Gate 10). `grep -c 'v1 NOTE:.*deferred to Phase 36.3.7.5' docs/kanban/reference.md` = 0 (Gate 11); 3 "Shipped in Phase 36.3.7.5" annotations replace the v1-NOTE blocks. Crate-isolation fence held — zero `ironhermes-gateway` Cargo dep declarations in `ironhermes-kanban/Cargo.toml`; SendFn trait-object closure IS the kanban→gateway boundary. Default-off preserved (`notification_sources: None` Default; `notifier_poll_seconds: 3` Default). Commit SHA range: `fcdab1a8` (Plan 01 first feat) → `f9df28f6` (Plan 04 SUMMARY) → `f67153a6` (Plan 04 state close-out). VERIFICATION report at `.planning/phases/36.3.7.5-gateway-notifier-auto-subscribe-polling-loop/36.3.7.5-VERIFICATION.md`. **Forward note** (NOT a failure): Discord/Slack adapter coverage gap in `build_notifier_send_fn` closure is documented forward-compat — those adapters live inside their own spawned tasks and are not retained as runner-scope Arcs, so subscriptions naming `discord`/`slack` will log+drop per `D-log-and-drop-on-fail`. Telegram delivery works end-to-end today. Counter update: `completed_phases: 17 → 18`; plans unchanged at 79/79 (all 4 phase plans were already individually closed before this verification run). Next: pick next phase from milestone-v3.0 backlog.

## Recent close-out summary (2026-05-29)

**Phase 36.3.7 (Kanban kernel v1):** 9/9 plans shipped, 76 tests, INV-36.3.7.md ledger established. Live UAT-09-A/B surfaced 3 receiver-end bugs that the gsd-verifier's 17/17 PASS missed.

**Phase 36.3.7.0 (UAT-discovered fixes):** 5/5 plans shipped (Plan 05 added inline during UAT). 4 receiver-end bugs closed bilaterally. UAT-09-A runtime-PASS through 9/10 stages; stage 7 blocked by out-of-scope Bug #5 (delegate_task schema vs Anthropic) — queued as a new phase.

**Phase 36.17.4 (iron_hermes_ui queue wiring):** confirmed Complete in this rationalization pass — all 6 plans had SUMMARYs + a VERIFICATION.md from earlier work; STATE was just stale.

**Phase 36.3.7.2 (delegate_task tool-schema-compat — drop top-level oneOf):** 2/2 plans shipped + 1 post-merge regression fix. Plan 01 dropped the `oneOf` block; Plan 02 added the system-level receiver-end lock test (`crates/ironhermes-tools/tests/no_top_level_schema_combinators.rs`) that walks every registered tool's schema. Post-merge regression: Plan 01 had also removed the `"required": []` field entirely — pre-existing `tests/delegate_task_timeout_cancel.rs::schema_exposes_timeout_seconds_field` unwrapped `parameters["required"].as_array()` and panicked. Fixed in commit `1448c441` by restoring `"required": []` as a sibling of `properties` (CONTEXT explicitly allowed both forms). Verifier (commit `bb12e47e`) confirmed PASS-WITH-NOTES (5/5 gates) and traced 7 in-tree consumers + 1 cross-crate consumer (anthropic_client.rs::adapt_tools — the original bilateral receiver). Forward-flagged for follow-up: `crates/ironhermes-cli/tests/invariants_26_4_1_cfg_03.rs::phase_amendment_doc_comment_present` is now failing — root cause is Phase 36.3.7.0 commit `c453411f` rewriting a comment block above `run_preflight`, pushing the original CFG-03 amendment doc-comment outside the test's 1500-char scan window. Untouched by 36.3.7.2.

**Meta-finding (captured in `.planning/LEARNINGS.md`):** bilateral-tracing rule for future verifier prompts — for every wire-up claim, trace BOTH producer AND consumer ends. Phase 36.3.7.2 re-demonstrated the rule twice in one phase: (a) Plan 01's grep audit covered `src/` producers but missed `tests/` consumers reading `parameters["required"]` — caught only at the post-merge full-suite run, fixed bilaterally in commit `1448c441`; (b) verifier dispatch with explicit bilateral-tracing emphasis was the only reason the 8th consumer (anthropic_client.rs::adapt_tools — the cross-crate one) was named.

**Phase 36.3.7.1 (dispatcher breaker on remaining failure paths):** 2/2 plans shipped (8 commits on develop). Plan 01 wired `apply_circuit_breaker` into `reclaim_stale_claims` at dispatcher.rs:481; Plan 02 wired it into `enforce_max_runtime` at dispatcher.rs:604. Both used the canonical line 305 bump → event → breaker pattern from 36.3.7.0-03. 4 receiver-end tests added to `crates/ironhermes-kanban/tests/dispatcher_logic.rs` (2 per path × 2 sides of `failure_limit` bound). Verifier (commit `cea58ace`) returned PASS (8/8 gates). Latent finding from Plan 01 SUMMARY: `reclaim_stale_claims` doc-comment promises a `KanbanEventKind::Reclaimed` event but only emits `tracing::info!` — out-of-scope per CONTEXT, queued for a future "dispatcher events parity" phase. **Operational note:** both Plan 01 and Plan 02 executors hit cwd-drift bug #3097 — `cd /absolute/path` in Bash calls landed commits on develop rather than the spawn-time worktree branches. Orchestrator authorized Option B (continue on develop directly); verifier audited and confirmed: monotone commit order per plan, contiguous test-file appends, scope-fenced diffs, provenance-prefixed messages.

**Phase 36.3.7.3 (CLI CFG-03 doc-comment scan-window regression):** 1/1 plan shipped (5 commits, orchestrator-inline execution per CONTEXT decision). Producer fix: 1-line CFG-03 marker added at main.rs:414. Consumer fix: `phase_amendment_doc_comment_present` test rewritten to anchor on the enclosing `async fn main` scope rather than a 1500-char byte window, AND tightened `||` to `&&` (both `"Phase 26.4.1"` AND `"CFG-03"` must appear together — original CFG-03 contract). Inline orchestrator execution mirrored Phase 36.3.7.0 Plan 05 — no worktree, no subagent, no cwd-drift risk. All 6 phase-level gates PASS. Forward note: hardened scope-anchor approach eliminates the entire class of "marker pushed out of window" regressions; future legitimate comment additions near `run_preflight` no longer require auditing the static-grep tests.

**Phase 36.3.7.4 (dispatcher events parity — emit Reclaimed event):** 1/1 plan shipped (1 test commit + 1 docs commit, orchestrator-inline execution). Task 1 read-only verification revealed `cas::release_claim` at `crates/ironhermes-kanban/src/cas.rs:114-145` ALREADY emits the `reclaimed` event row via direct SQL `INSERT INTO task_events ... kind='reclaimed'` at lines 136-140 — called from `dispatcher.rs:459` with `reason="ttl_expired"`. The doc-comment at `dispatcher.rs:19` ("reset to `ready`, append `reclaimed` event") is factually correct; the implementation path is just indirect through `release_claim` rather than a direct `store.append_event` call in `reclaim_stale_claims`. **Per CONTEXT's conditional gate (alternative (a) in locked decisions), BUG-36.3.7.4-01 producer fix was correctly a NO-OP.** Only BUG-36.3.7.4-02 (receiver test) shipped: `reclaim_stale_claims_appends_reclaimed_event_to_task_events` in `tests/dispatcher_logic.rs`, mirroring the 36.3.7.1-01 seed pattern (failure_limit=2 headroom, NULL claim_pid, scheduled_at far future), asserts `count(task_events kind='reclaimed') >= 1`, payload contains "ttl_expired", payload carries the seeded stale_lock. Commit `1161fc7d`. All 7 phase-level gates PASS. Corroboration: pre-existing `archive_running_emits_reclaimed_event` in `store_smoke.rs` already locks the same emission path via a different caller. **Bilateral-tracing closure:** producer = `cas::release_claim` SQL emission (pre-existing); consumer = new dispatcher_logic test + future 36.3.7.5 gateway notifier. **Forward note:** the receiver test guarantees the rows exist for Phase 36.3.7.5's notifier to consume regardless of whether a future refactor moves the emission inline; the doc/impl contract now has explicit regression-test backing even though the producer was correct by construction.

**Phase 36.3.7.5 Plan 01 (kanban_subscriptions schema + CRUD + receiver tests) closed PASS (2026-05-30):** 4 commits on develop — `fcdab1a8` schema CREATE TABLE + 2 indexes + UNIQUE + CHECK + Subscription struct + lib.rs re-export, `dd59715f` 5 pub fn CRUD methods on KanbanStore (append_subscription / list_subscriptions_for_task / list_subscriptions_for_chat / remove_subscriptions / remove_all_subscriptions_for_task) landing next to append_event at store.rs:419, `8519279d` 8 receiver-end tests in NEW tests/subscriptions_logic.rs covering schema migration + UNIQUE + CHECK + empty-vs-7 thread-id semantics + 4 CRUD paths, `072c3afd` SUMMARY with bilateral-tracing table (BUG-01 producer/consumer + BUG-02 producer/consumer + BUG-07a test aggregate). All 8 Task-5 verification gates PASS. ironhermes-kanban test crate at 115 passed / 0 failed (+8 baseline → 115). Zero deviations applied (no Rule-1/-2/-3 fixes needed). Empty-string thread_id substitution at the SQL boundary locks the UNIQUE constraint against SQLite's NULL-is-distinct semantics. CHECK constraint on source IN ('auto', 'explicit') is the storage-layer mitigation for T-36.3.7.5-01-04 (spoofing). SCHEMA_VERSION stayed at 1 (CREATE TABLE IF NOT EXISTS is the migration). No deps on ironhermes-gateway introduced. **Wave 1 half-complete — Plan 02 (notifier.rs polling loop + send_fn injection + notifier_poll_seconds config + 5 polling-loop receiver tests) is file-disjoint and ready to execute next.** See SUMMARY.md.

**Phase 36.3.7.5 Plan 02 (notifier polling loop + send_fn injection + receiver tests) closed PASS (2026-05-30):** 3 implementation commits + 1 SUMMARY commit on develop — `c22febbf` `notifier_poll_seconds: u64` config field with serde default helper (default 3) + doc-comment row + extended `default_matches_context_md` test assertion, `e635e198` NEW `crates/ironhermes-kanban/src/notifier.rs` (~250 LOC) publishing `NotifierContext` (mirrors `DispatcherContext` shape), `run_notifier_loop(ctx, cancel)` (long-lived tokio task), `run_notifier_tick(ctx)` (testable inner step returning `NotifierTickReport`), `SendFn = Arc<dyn Fn(&str, &str, Option<&str>, &str) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>` trait-object alias, `format_terminal_message(task, ev)` (per-kind plain-text formatter), `init_watermark` (startup `MAX(id) FROM task_events` seed) + 2 raw-SQL helpers on `KanbanStore` (`list_terminal_events_after(watermark)` + `max_event_id()`) keeping rusqlite access inside the store + `pub mod notifier;` + 5-name re-export in lib.rs, `f7c40e8e` 5 receiver-end tests in NEW `crates/ironhermes-kanban/tests/notifier_logic.rs` driving `run_notifier_tick` directly with mock SendFn closures (one recording, one failing): `polling_loop_delivers_once_and_removes_subscription`, `reclaimed_event_is_ignored`, `watermark_advances_past_processed_event`, `send_fn_failure_still_removes_subscription`, `no_subscriptions_means_no_send`, `84a94a5a` SUMMARY with bilateral-tracing table (BUG-03 producer = notifier.rs + store.rs helpers + config.rs field + lib.rs re-exports / consumer = the 5 receiver tests) + crate-isolation audit. All 12 Task-5 verification gates PASS. ironhermes-kanban test crate at 120 passed / 0 failed (+5 from notifier_logic, exactly the planned delta from 115 → 120). Workspace suite 3552 passed / 0 failed — zero regressions. **Crate-isolation fence HELD:** `grep -E '^ironhermes-gateway\s*=|^ironhermes_gateway\s*=' crates/ironhermes-kanban/Cargo.toml` returns 0 dep declarations; the single textual match at line 25 is a pre-existing comment from Phase 36.3.7 Plan 01 (`# nix is already used by ironhermes-gateway; ...`), not a Cargo dep entry. The `SendFn` trait-object closure IS the kanban→gateway boundary — the gateway will construct the closure at spawn time (Plan 03) capturing its `Arc<dyn PlatformAdapter>` set; the mock SendFn in the 5 tests is built from zero gateway types, proving the boundary is clean by construction. **Locked CONTEXT decisions materially observable:** D-send-closure-injection (no gateway dep), D-watermark-in-memory (AtomicI64 + `fetch_max` monotonicity, no persistence — gateway-downtime loss accepted for v1), D-log-and-drop-on-fail (`send_fn_failure_still_removes_subscription` locks the policy: subscription removed even when send fails, watermark still advances so next tick does not replay), D-auto-remove-after-attempt (auto-remove on FIRST delivery attempt regardless of success/failure), Reclaimed event ignored (SQL kind filter excludes it; `reclaimed_event_is_ignored` locks the fact). Zero Rule-1/-2/-3/-4 deviations. Threat-model audit clean: T-36.3.7.5-02-01 (watermark drift) mitigated by `fetch_max`; T-36.3.7.5-02-04 (forged Reclaimed) mitigated by SQL filter; T-36.3.7.5-02-SC (crate boundary) mitigated by Gate 11. **Wave 1 of Phase 36.3.7.5 now COMPLETE — Wave 2 (Plan 03 gateway runner spawn + notifier_gating helper + send-closure builder + 3 gating receiver tests + Plan 04 3 notify-* CLI verbs + KanbanStoreWriter trait + CommandContext chat-origin extension + cmd_kanban Create arm + auto-subscribe hook + 9 dispatch + lifecycle e2e tests + docs/kanban/reference.md v1-NOTE reconciliation) is UNBLOCKED.** Plan 03 will import `ironhermes_kanban::{run_notifier_loop, NotifierContext, SendFn}` and construct the send closure from its `Arc<dyn PlatformAdapter>` set. See SUMMARY.md.

**Phase 36.3.7.5 Plan 03 (gateway runner notifier spawn + gating helper + receiver tests) closed PASS (2026-05-30):** 3 implementation/test commits + 1 SUMMARY commit on develop — `16a208ed` store-arc lift refactor in `crates/ironhermes-gateway/src/runner.rs` hoisting `KanbanStore::open_default()` ONCE above both dispatcher + notifier spawns + NEW notifier spawn block mirroring the dispatcher spawn at line 1278 + NEW `crates/ironhermes-gateway/src/notifier_gating.rs` (~80 LOC) with `pub fn compute_notifier_gate` + `pub enum NotifierGate` (DisabledNoSources / DisabledNoOverlap / Enabled) declared as `pub mod notifier_gating;` in lib.rs + 3 private helpers in runner.rs (`collect_enabled_platform_names`, `build_adapter_snapshot`, `build_notifier_send_fn`) constructing the `ironhermes_kanban::SendFn` trait-object closure with case-insensitive platform routing to the runner-scope Telegram adapter Arc, `e7865757` 3 receiver-end tests in NEW `crates/ironhermes-gateway/tests/notifier_spawn_gating.rs` (`gate_returns_disabled_no_sources_when_none` locking default-off semantics for BOTH `None` and `Some(empty)`, `gate_returns_disabled_no_overlap_when_no_intersection` with case-insensitive non-match sub-case, `gate_returns_enabled_with_overlap_when_intersection_exists` with 4 sub-cases: simple match + case-insensitive caller-casing preservation + multi-source filter + full-overlap insertion-order preservation), `e3904f1a` SUMMARY with bilateral-tracing table (BUG-04 producer = notifier_gating.rs + runner.rs spawn block + 3 helpers + lib.rs declaration / consumer = the 3 receiver tests) + 3-level store-arc lift audit + crate-isolation fence verification + 9 verification gate outcomes. All 9 Task-4 verification gates PASS. ironhermes-gateway test crate at 165 passed / 0 failed (+3 from notifier_spawn_gating; exactly the planned delta from 162). ironhermes-kanban test crate still at 120 passed / 0 failed (post-Plan-02 baseline preserved — store-arc lift confirmed semantics-preserving for dispatcher). Workspace suite 679 passed / 0 failed — zero NEW regressions. **Crate-isolation fence STILL HELD:** `grep -E '^ironhermes-gateway\s*=|^ironhermes_gateway\s*=' crates/ironhermes-kanban/Cargo.toml` returns 0 dep declarations. The new `runner.rs` `build_notifier_send_fn` closure captures gateway-side Arcs and produces an `ironhermes_kanban::SendFn` — gateway-→-kanban flow only. **Locked CONTEXT decisions materially observable:** D-gateway-gating (notifier-spawn gate fails closed when `notification_sources = None`; asserted by `gate_returns_disabled_no_sources_when_none` Test 1), default-off preserved verbatim. **One Rule-1 fix-up cycle applied:** INV-36.3.7-08-05 (`tests/kanban_dispatcher_spawned.rs:159` asserting `kanban dispatcher will NOT start` substring exists in the `KanbanStore::open_default()` failure path) initially failed after Task 2 because the warn message rewording broke the substring contiguity. Fixed by rewording the warn to retain the greppable substring AND document that notifier is also skipped — pure message text change, no test relaxation. Re-ran INV suite → 9 passed / 0 failed. Threat-model audit clean: T-36.3.7.5-03-01 (subscription-names-disabled-platform) mitigated by closure's `find().map()` lookup returning `Err("platform X not enabled in gateway")`; T-36.3.7.5-03-SC (store-arc lift changes dispatcher) mitigated via the 3-level audit. **Discord/Slack delivery wiring is a forward-compat permanent fence** — those adapters are constructed inside their own spawned tasks (Discord wraps Serenity Context post-handshake; Slack constructs inside its socket-mode runner) so neither is retained as a runner-scope Arc; subscriptions naming `discord`/`slack` will hit the closure's "not enabled in gateway" arm and the notifier loop log+drops per locked policy D-log-and-drop-on-fail. **Wave 2 of Phase 36.3.7.5 now half-complete — Plan 04 (3 notify-* CLI verbs + KanbanStoreWriter trait + CommandContext chat-origin extension + cmd_kanban Create arm + auto-subscribe hook + 9 dispatch + lifecycle e2e tests + docs/kanban/reference.md v1-NOTE reconciliation) is UNBLOCKED.** Plan 04's auto-subscribe hook will write subscription rows that the now-shipped Plan-03 gateway notifier loop will read at the next tick (3s default). See SUMMARY.md.

**Phase 36.3.7.5 Plan 04 (3 notify-* CLI verbs + KanbanStoreWriter trait/impl + CommandContext chat-origin extension + cmd_kanban Create arm + auto-subscribe hook + 9 dispatch/lifecycle tests + docs reconciliation) closed PASS (2026-05-30):** 6 implementation/test/docs commits on develop — `bde6f4e2` extends `CommandContext` in `crates/ironhermes-core/src/commands/context.rs` with the additive triple (`kanban_store_writer: Option<Arc<dyn KanbanStoreWriter>>` + `chat_id: Option<String>` + `thread_id: Option<String>`) plus 2 new builders (`with_kanban_store_writer`, `with_chat_origin`) plus a NEW `pub trait KanbanStoreWriter` (sibling of read-only `KanbanStoreReader`) and `pub struct SubscriptionView` (boundary flat view; `PartialEq` only because `created_at: f64` disqualifies `Eq`); `845d8414` removes `"create"` from `DEFERRED_KANBAN_SUBVERBS` in `handlers.rs:1145` and adds a `Some("create")` arm to `cmd_kanban` that parses `title` + `--assignee` + `--json` from the slash-arg slice, calls `writer.create_task_simple`, and conditionally calls `writer.append_subscription` (gated on `ctx.chat_id.is_some() AND !json` per `D-json-skips-auto-subscribe`), failures log+drop (the task is still created); `23842235` adds 3 new `KanbanCommands` variants (`NotifySubscribe`, `NotifyList`, `NotifyUnsubscribe`) + 3 dispatch arms in `handle_kanban_command` + 3 new `cmd_notify_*` async fns in `commands.rs` (subscribe writes `source='explicit'`; list supports `--json` or table; unsubscribe handles 4 filter modes) + NEW `crates/ironhermes-kanban/src/store_writer_impl.rs::KanbanStoreWriterImpl` (concrete production impl; each method opens `KanbanStore::open_default` per call) + `pub fn list_all_subscriptions` on `KanbanStore` (sibling of Plan 01's 5 CRUD APIs); `3ba0f3d0` attaches BOTH new builders to `CommandContext` in `crates/ironhermes-gateway/src/handler.rs::handle_slash_command` at line 430 — `.with_chat_origin(event.chat_id.clone(), event.thread_id.clone()).with_kanban_store_writer(Arc::new(KanbanStoreWriterImpl::new()))` — AND relocates `KanbanStoreWriterImpl` from `ironhermes-cli` to `ironhermes-kanban` (Check A fallback: `ironhermes-cli` already depends on `ironhermes-gateway`; reverse dep would be circular; the cli still re-exports the impl from `kanban/mod.rs` for ergonomic discoverability); `707cf01a` lands 9 receiver-end tests in NEW `crates/ironhermes-core/tests/handlers_kanban_notify.rs` — 5 BUG-05 dispatch tests (subscribe writes 'explicit', list returns rows, list_all under `--json`, unsubscribe with/without filters) + 4 BUG-06 tests (`dispatch_kanban_create_routes_to_create_arm_not_deferred`, `auto_subscribe_writes_row_when_chat_origin_present` asserting `platform="local"` via `Platform::Display`, `dispatch_kanban_create_with_json_skips_auto_subscribe` locking the `D-json-skips-auto-subscribe` decision with `subscribe_calls.len() == 0`, and the keystone `auto_subscribe_lifecycle_end_to_end` driving the full cross-crate pipeline through a `tempfile`-backed `KanbanStore` + a fresh `NotifierContext` with `TokioMutex` + a recording mock `SendFn` closure → asserts `delivered=1`, `events_processed=1`, mock called with `("local", "42", None, "lifecycle done"-containing message)`, subscription row REMOVED after delivery attempt) + `ironhermes-kanban` added as `[dev-dependencies]` on `ironhermes-core` (dev-only — does NOT participate in the production lib link graph; safe because `ironhermes-kanban` already depends on `ironhermes-core` as a regular dep); `09021e84` reconciles `docs/kanban/reference.md` — 2 `v1 NOTE: deferred to Phase 36.3.7.5` blocks at lines ~703 (Auto-subscribe section) and ~761 (Gateway notifications section) replaced with `Shipped in Phase 36.3.7.5` annotations describing shipped behavior + the default-off semantic; 1 inline `# Shipped in Phase 36.3.7.5` comment added above the `notify-*` CLI listing at line ~597. All 11 phase-close gates PASS: `cargo build --workspace` exit 0, lifecycle e2e PASS, 9 new tests pass (`cargo test -p ironhermes-core --test handlers_kanban_notify -- --test-threads=1` → `9 passed; 0 failed`), Gate 5 (CLI verbs ≥6 hits = 6), Gate 6a (`with_chat_origin` def + caller present), Gate 6b (`append_subscription` in handlers.rs = 1), Gate 7 (`--json` skip test PASS), Gate 8 (lifecycle e2e PASS), **Gate 11 (`grep -c 'v1 NOTE:.*deferred to Phase 36.3.7.5' docs/kanban/reference.md` = 0)**, Gate 12 (default-off `notification_sources: None` preserved), **crate-isolation fence STILL HELD** (`grep -E '^ironhermes-gateway\s*=|^ironhermes_gateway\s*=' crates/ironhermes-kanban/Cargo.toml` returns 0 lines), `"create"` removed from `DEFERRED_KANBAN_SUBVERBS` array (verified via `awk` over the array bounds). **One Rule-1 fix-up cycle applied:** `SubscriptionView` initially had `#[derive(PartialEq, Eq)]` per the plan verbatim; `cargo build -p ironhermes-core` broke because `f64 created_at` disqualifies `Eq`; fixed by dropping `Eq` from the derive (`PartialEq` remains; semantic equality at the boundary is unchanged). Bilateral-tracing satisfied per LEARNINGS 2026-05-29 — BUG-05 ships producer (`mod.rs` + `commands.rs` + `store_writer_impl.rs` + `list_all_subscriptions`) AND 5-test consumer in commits `23842235` + `707cf01a`; BUG-06 ships producer (`context.rs` + `handlers.rs` + `handler.rs` + `store_writer_impl` relocation) AND 4-test consumer (incl. lifecycle e2e) in commits `bde6f4e2` + `845d8414` + `3ba0f3d0` + `707cf01a`. No producer-only commits. Threat-model audit clean: T-36.3.7.5-04-04 (EoP / gateway gets write access where v1 only had read) mitigated by the `ctx.kanban_store_writer.is_some()` gate inside `cmd_kanban` Create arm — CLI/TUI sessions that don't attach the writer get a "writer not configured" output. **Phase 36.3.7.5 now COMPLETE end-to-end** — the full gateway-notifier loop (`/kanban create` from a gateway slash dispatch → auto-subscribe row written to `kanban_subscriptions` → terminal event lands in `task_events` → `run_notifier_tick` reads subscription → mock-or-real `send_fn` invoked → subscription row removed after delivery attempt) is wired AND lifecycle-e2e-tested. Default-off semantic preserved: `notification_sources: None` by default; operators opt in by setting `kanban.notification_sources: ["telegram"]` in `config.yaml`. See `36.3.7.5-04-SUMMARY.md`.

**Phase 36.3.7.5 (gateway notifier) planning landed (commit `c7cc6ebd`):** background `gsd-planner` decomposed the phase into 4 sub-plans across 2 waves (develop-direct strategy): Wave 1 = [Plan 01 schema+CRUD+8 tests, Plan 02 NEW `notifier.rs` polling loop+5 tests] (parallel-eligible, file-disjoint); Wave 2 = [Plan 03 gateway runner spawn+gating helper+3 tests, Plan 04 3 CLI verbs+`KanbanStoreWriter` trait+CommandContext chat-origin extension+`cmd_kanban` Create arm+auto-subscribe hook+9 dispatch tests+docs reconciliation]. Key planner decisions surfaced: (a) `CommandContext` does NOT carry `chat_id`/`thread_id` at v1 baseline (only platform + session_id + agent_running) — Plan 04 extends it additively with a `with_chat_origin` builder; the gateway's `handle_slash_command` at `handler.rs:430` is the attach site. (b) `cmd_kanban` Create arm does NOT exist at v1 baseline either — `"create"` is in `DEFERRED_KANBAN_SUBVERBS` at handlers.rs:1145; Plan 04 removes it from the deferred list, adds a `Some("create")` arm, and adds a NEW `KanbanStoreWriter` trait sibling to the existing read-only `KanbanStoreReader` (forward-compatible for future `/kanban comment`, `/kanban complete` slash arms). (c) Crate-isolation enforced by Cargo.toml grep gate — `kanban` does not depend on `gateway`; the `SendFn` trait-object closure IS the kanban→gateway boundary, gateway INJECTS it at spawn time. (d) Bilateral-tracing-by-construction enforced per LEARNINGS 2026-05-29 — every BUG ships producer + consumer in the SAME plan, never split. Phase ready to execute via `/gsd-execute-phase 36.3.7.5`.

**UAT-09-A Run #6 (2026-05-30, ~3.5 hours elapsed across two attempts):** the 36.3.7.x cascade's live bilateral consumer signal — first end-to-end kanban worker round-trip after all 4 companion phases landed. **Stage 7 PASS reproducible 2x**: zero `400: input_schema does not support oneOf` errors across both attempts, confirming the 36.3.7.2 schema unblock works end-to-end through Anthropic-via-OpenRouter. **Two bonus live breaker confirmations** from the dispatcher event log + tick stderr: (1) `detect_crashed_workers → apply_circuit_breaker` (36.3.7.0-03 wiring) — `gave_up failures=2 effective_limit=2` log line; (2) `reclaim_stale_claims → apply_circuit_breaker` (36.3.7.1-01 wiring) — `reclaimed {reason: "crashed"}` event entry. Both worker runs hit an upstream `500 Internal Server Error` on the SECOND streaming completion (after the first tool result returned) — reproducible across two attempts → provider-side, not a 36.3.7.x regression; documented for operator follow-up. Stages 1-8 + 10 + 2 bonus all green; Stage 9 (workspace dir D-31) N/A by design (`--workspace scratch` is a pass-through marker per `paths.rs::non_dir_workspaces_pass_through`). Operational corrections to the runbook (commit `e4452f0c` + `c2c90b3d`): `kanban create` requires `--assignee <NAME>` + `--body <PROMPT>` + short positional TITLE; `kanban dispatch` is one-shot by default (no `--once`); worker logs + workspaces resolve under profile-scoped HERMES_HOME (`~/.ironhermes/profiles/testbanner/...`, not the global root). Evidence appended at `5737b64a` to `36.3.7.0-04-UAT-EVIDENCE.md`.

**Side-quest (UAT prep): tools_smoke env-var race (commit `4208bc28`).** While dry-running the UAT runbook, the operator hit `panic: task not found: t_547330196a6c44b6` from `kanban_complete_rejects_stale_run_id`. Root cause: 7 tests in `crates/ironhermes-kanban/tests/tools_smoke.rs` set process-global `HERMES_KANBAN_TASK` / `HERMES_PROFILE` then call `tool.execute()`. Cargo runs the tests in parallel, they race — test A's tool reads test B's task_id, looks for it in test A's in-memory store, panics. Producer-side correctness unchanged (the dispatcher's `worker_spawn` correctly sets these per-subprocess); the bug is purely a test-harness in-process shared-namespace issue. Fix: `static ENV_LOCK: std::sync::Mutex<()>` + `let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())` at the top of each of the 7 affected tests. Poison-tolerant (a prior-test panic doesn't deadlock the next one). 21 LOC, no new deps. Verified race-free via 3 consecutive full-suite runs: 14 passed / 0 failed across all 3. Reinforces the bilateral-tracing meta-rule — producer/consumer integrity holds in production, but the test harness can violate the implicit single-writer assumption that the env-var-as-channel pattern depends on.

## Phase 36.2 Closure Summary

**Wave execution:** 11/11 plans complete (commits aa65bdba, c90c17e1, 8d7ce6ce, 3db67dd0, 7bc8a059, b355e116, fd7f63f9, 0ebcf16c, 2d379cd1, 8b9c68bb, f86d647a, 99a491a1, ce9b3200, a5a2f902, 6493e8b2, bf5a9013, 8fa811b4, bcb7b25f, 8333bfdc, etc.)

**Chat-fix series (post-merge, 2026-05-25):** 7 commits patching usage_events streaming defects

- a9fb0d0d, 4eead836, 0987e2e2, c74cac60, 402113b3, 0b7a9b85, 9071afc6

**Code-review BLOCKER closeout (2026-05-26):** all 10 CR-* findings from REVIEW.md fixed

- CR-01 `12a887d9` · CR-02 `7f6431dd` · CR-03 `185c98aa` · CR-04 `d94d73fa`
- CR-05/06/07/10 `4fededb0` · CR-08 `185c98aa` · CR-09 `e865a5ca`

**Gateway integration fixes (2026-05-26):**

- `7fd63515` CommandContext.state_store · `beaaf471` TurnRequest.state_store
- `2f253697` canonical UUID alignment · `6360ae72` drop with_intercepts (chat truncation)

**Operator tooling (2026-05-26):**

- `8203ec36` `hermes pricing refresh --source openrouter`
- `f000d234` disk pricing cache merged per-turn
- `cd3e2ee5` `hermes pricing backfill` + `56a454eb` `--clean-orphans`

**Verification:** workspace release build clean (3m 53s), 3288 tests pass / 0 failures.

## Performance Metrics

**Velocity:**

- Total plans completed: 153
- Average duration: — min
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 11 | 2 | - | - |
| 15 | 3 | - | - |
| 19.1 | 5 | - | - |
| 18 | 15 | - | - |
| 20 | 4 | - | - |
| 21 | 3 | - | - |
| 22 | 2 | - | - |
| 22.1 | 2 | - | - |
| 21.1 | 2 | - | - |
| 21.3 | 5 | - | - |
| 21.4 | 3 | - | - |
| 21.5 | 4 | - | - |
| 21.6 | 3 | - | - |
| 21.8 | 6 | - | - |
| 22.4.2 | 5 | - | - |
| 22.4.2.2 | 2 | - | - |
| 22.4.2.3 | 1 | - | - |
| 25.3 | 18 | - | - |
| 25.6 | 3 | - | - |
| 26.3 | 1 | - | - |
| 21.8.3.1 | 2 | - | - |
| 27.1.1 | 7 | - | - |
| 27.1.2 | 1 | - | - |
| 27.1.3 | 2 | - | - |
| 27.1.4.1 | 2 | - | - |
| 27.1.4.1.1 | 1 | - | - |
| 26.3.2 | 1 | - | - |
| 27.1.4.2 | 1 | - | - |
| 32.1 | 8 | - | - |
| 26.7.1 | 2 | - | - |
| 34a | 2 | - | - |
| 28.1 | 6 | - | - |
| 34b | 4 | - | - |
| 36.3.7.9 | 9 | - | - |
| 36.3.7.10 | 6 | - | - |
| 36.3.7.12 | 5 | - | - |
| 36.17.6 | 3 | - | - |

**Recent Trend:**

- Last 5 plans: —
- Trend: —

*Updated after each plan completion*
| Phase 12 P02 | 8 | 2 tasks | 3 files |
| Phase 12 P04 | 35 | 2 tasks | 8 files |
| Phase 13 P01 | 3 | 2 tasks | 1 files |
| Phase 13 P02 | 3 | 2 tasks | 3 files |
| Phase 13 P03 | 5 | 2 tasks | 4 files |
| Phase 17 P01 | 8 | 2 tasks | 2 files |
| Phase 17 P02 | 4 | 2 tasks | 3 files |
| Phase 17 P03 | 4 | 2 tasks | 9 files |
| Phase 19 P03 | 6min | 2 tasks | 6 files |
| Phase 19 P04 | ~3 min | 2 tasks | 3 files |
| Phase 19 P05 | 8 min | 2 tasks | 2 files |
| Phase 19 P06 | 7min | 2 tasks | 7 files |
| Phase 18 P15 | 3 | 3 tasks | 4 files |
| Phase 20-memory-provider-plugin-contract P01 | 19 | 3 tasks | 10 files |
| Phase 20 P02 | 42 min | 3 tasks | 13 files |
| Phase 20 P04 | 8 min | 3 tasks | 9 files |
| Phase 20 P03 | 5 min | 2 tasks | 4 files |
| Phase 21 P21-02 | 17 | 1 tasks | 3 files |
| Phase 22 P01 | 3 | 2 tasks | 1 files |
| Phase 22 P02 | 5 | 2 tasks | 2 files |
| Phase 22.1 P01 | 4 | 2 tasks | 4 files |
| Phase 22.1 P02 | 4 | 2 tasks | 3 files |
| Phase 21.1 P01 | 4 | 3 tasks | 5 files |
| Phase 21.1-slash-commands P02 | 35 | 2 tasks | 3 files |
| Phase 21.3 P01 | 5 | 2 tasks | 4 files |
| Phase 21.3 P02 | 11min | 2 tasks | 9 files |
| Phase 21.3 P03 | 3min | 1 tasks | 3 files |
| Phase 21.3 P04 | 5min | 2 tasks | 5 files |
| Phase 21.3 P05 | 3min | 2 tasks | 1 files |
| Phase 21.4 P01 | 3 | 1 tasks | 1 files |
| Phase 21.4 P02 | 90 | 2 tasks | 11 files |
| Phase 21.4 P03 | 4 | 2 tasks | 3 files |
| Phase 21.5 P01 | 3 | 2 tasks | 2 files |
| Phase 21.5 P02 | 4 | 2 tasks | 2 files |
| Phase 21.5 P03 | 8 | 2 tasks | 6 files |
| Phase 21.5 P04 | 2min | 1 tasks | 1 files |
| Phase 21.6 P01 | 5 | 2 tasks | 5 files |
| Phase 21.6 P02 | 13 | 2 tasks | 2 files |
| Phase 21.6 P03 | 4 | 2 tasks | 2 files |
| Phase 21.8 P01 | 7 | 3 tasks | 7 files |
| Phase 21.8 P02 | 7 | 2 tasks | 8 files |
| Phase 21.8 P03 | 14 | 2 tasks | 9 files |
| Phase 21.8 P04 | 104 | 2 tasks | 5 files |
| Phase 21.8 P05 | 22 | 2 tasks | 5 files |
| Phase 21.8-skill-remote-download-and-install-from-skills-sh P06 | 15 | 2 tasks | 4 files |
| Phase 21.2 P01 | 5 | 2 tasks | 6 files |
| Phase 21.2 P02 | 8 | 2 tasks | 9 files |
| Phase 21.2-mcp-client-tool-and-fold-in-slash-commands-related-to-mcp-cl P03 | 5 | 2 tasks | 7 files |
| Phase 21.2 P04 | 9 | 2 tasks | 9 files |
| Phase 21.2 P05 | 4 | 2 tasks | 2 files |
| Phase 21.2 P06 | 4 | 2 tasks | 1 files |
| Phase 21.2 P07 | 2 | 2 tasks | 3 files |
| Phase 21.2 P09 | 8 | 2 tasks | 2 files |
| Phase 21.2 P11 | 25 | 2 tasks | 7 files |
| Phase 22.3 P1 | 2 | 1 tasks | 2 files |
| Phase 22.3 P2 | 2 | 3 tasks | 3 files |
| Phase 22.3 P3 | 176 | 2 tasks | 2 files |
| Phase 22.3 P4 | 15 | 3 tasks | 5 files |
| Phase 22.3 P5 | 545 | 3 tasks | 5 files |
| Phase 22.3 P6 | 8 | 1 tasks | 1 files |
| Phase 22.3 P8 | 3 | 1 tasks | 1 files |
| Phase 22.3 P9 | 3 | 1 tasks | 1 files |
| Phase 22.3 P11 | 52 | 3 tasks | 3 files |
| Phase 22.3 P12 | 11 | 1 tasks | 1 files |
| Phase 22.4.2.1 P01 | 597 | 3 tasks | 9 files |
| Phase 22.4.2.1 P02 | 5min | 2 tasks | 5 files |
| Phase 22.4.2.1 P03 | 5 | 2 tasks | 3 files |
| Phase 24 P01 | 247 | 2 tasks | 3 files |
| Phase 24 P03 | 264 | 3 tasks | 3 files |
| Phase 24 P04 | 202 | 2 tasks | 2 files |
| Phase 24 P05 | 12 | 3 tasks | 6 files |
| Phase 24 P06 | 10 | 2 tasks | 2 files |
| Phase 24 P07 | 849 | 2 tasks | 1 files |
| Phase 25.2 P00 | 5 | 4 tasks | 17 files |
| Phase 25.2 P02 | 10 | 2 tasks | 2 files |
| Phase 25.2 P03 | 4 | 2 tasks | 3 files |
| Phase 25.2 P04 | 6 | 1 tasks | 2 files |
| Phase 25.2 P05 | 7 | 1 tasks | 2 files |
| Phase 25.2 P06 | 8 | 2 tasks | 2 files |
| Phase 25.2 P07 | 10 | 2 tasks | 2 files |
| Phase 25.2 P08 | 3 | 1 tasks | 1 files |
| Phase 25.2 P09 | 17 min | 1 tasks | 1 files |
| Phase 25.2 P10 | 22 | 1 tasks | 1 files |
| Phase 25.2 P12 | 8 | 2 tasks tasks | 5 files files |
| Phase 25.2 P13 | 108 | 3 tasks | 5 files |
| Phase 25.2 P14 | 25 | 3 tasks | 5 files |
| Phase 25.5 P05 | 2min | 1 tasks | 3 files |
| Phase 27.1 P03 | 1 | 2 tasks | 1 files |
| Phase 26.2.1 P14 | 35min | 4 tasks | 5 files |
| Phase 32.2 P02 | 18 | 2 tasks | 2 files |
| Phase 32.2-subagent-delegation-parity P03 | 1105 | 2 tasks | 4 files |
| Phase 36.17.3 P01 | 25min | 3 tasks | 5 files |
| Phase 36.17.3 P06 | 10min | 3 tasks | 4 files |
| Phase 37.1 P02 | 4 | 2 tasks | 4 files |
| Phase 37.1 P03 | 6 | 1 tasks | 1 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- v2.0: Port hermes-agent architecture faithfully — deviate only with documented rationale
- v2.0: Two-tier memory: built-in MEMORY.md/USER.md always active + optional external provider on top
- v2.0: Memory providers scoped to SQLite, Grafeo, DuckDB only (not all 8 Python backends)
- v2.0: Frozen-snapshot pattern — system prompt built once at session start, mid-session writes take effect next session
- [Phase 12]: AnyClient uses enum dispatch (not trait objects) for zero-cost multi-provider abstraction
- [Phase 12]: AgentLoop.client changed from LlmClient to AnyClient; resolve_base_url/resolve_api_key deleted
- [Phase 13]: busy_timeout(5000ms) + deterministic jitter retry (no rand dep) for SQLite write contention
- [Phase 13]: SearchFilter with composable WHERE clauses and FTS5 snippet() using << >> markers
- [Phase 13]: prune_sessions deletes messages explicitly before sessions (no CASCADE); SessionExport with Serialize+Deserialize for JSON export
- [Phase 13]: SessionStore composes Arc<Mutex<StateStore>> + HashMap as write-through cache; every create/message writes to SQLite immediately
- [Phase 17]: Snapshot field changed from HashMap<MemoryTarget, String> to HashMap<MemoryTarget, Vec<String>> - raw entries stored, header computed lazily
- [Phase 17]: Error transformation in MemoryTool: blocked -> content_rejected envelope; capacity_exceeded -> D-15 envelope with suggestion field
- [Phase 17]: Single-pass marker conversion for <<match>> -> >>>match<<< avoids chained String::replace double-substitution
- [Phase 17]: session_search schema only added to LLM tool list when state_store is configured — acts as subagent safety gate
- [Phase 17]: Mutex<Connection> wraps rusqlite::Connection to satisfy Sync bound on MemoryProvider trait
- [Phase 17]: Factory in ironhermes-agent returns Arc<Mutex<dyn MemoryProvider>> vs Box<dyn> in core for MemoryTool compatibility
- [Phase 19]: 19-03: setup_needed envelope shape aligns with Phase 17 D-15 structured errors; setup_note is a verbatim-quotable relay string
- [Phase 19]: 19-03: credential_dir precedence = SkillsConfig.credential_dir → HERMES_HOME/credentials → ~/.ironhermes/credentials (per D-10)
- [Phase 19]: Plan 04: SkillsConfig.config stored as HashMap<String, HashMap<String, serde_yaml::Value>> with serde(default) for backward compat
- [Phase 19]: Plan 04: [Skill config: ...] header keys lex-sorted for deterministic prompt output and cache safety
- [Phase 19]: Plan 04: declared_config_schema returns None for unknown skill / no hermes meta / empty config — single sentinel for 'no schema'
- [Phase 19]: Plan 05: scan_skill_content layers SKILL_THREAT_PATTERNS over existing context THREAT_PATTERNS via short-circuit composition; scope=frontmatter+body (D-14), enforcement=Community-hard-reject + Builtin/Official-WARN-BUT-LOAD at registry-load (D-15/D-16)
- [Phase 18]: Disk-load responsibility moved into agent factory file branch so gateway needs only a single factory call
- [Phase 18]: Used .err().unwrap() instead of .unwrap_err() to extract errors from Result<Arc<Mutex<dyn MemoryProvider>>, _> which lacks Debug on T
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: kept std::sync::Mutex in build_memory_provider return type; deferred tokio::sync::Mutex workspace migration to Plan 20-02 atomic wave
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: grafeo DB path must use .grafeo file extension (memory_graph.grafeo) — required for grafeo persistence flush
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: MemoryProviderConfig deleted entirely (no compat shim per D-10/D-20); all providers migrated in lockstep
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: env-mutating tests use OnceLock<Mutex<()>> + double-set idiom (re-assert IRONHERMES_HOME before each build_memory_provider call) to tolerate racing prompt_builder tests
- [Phase 20]: Plan 20-02: MemoryManagerHandle trait in ironhermes-tools resolves tools→agent circular dep; impl lives in ironhermes-agent so MemoryTool can delegate to handle_tool_call via dyn dispatch
- [Phase 20]: Plan 20-02: full workspace migration from std::sync::Mutex to tokio::sync::Mutex executed atomically; load_memory promoted to async fn; queue_prefetch fires as detached tokio::spawn on natural-end break with last user message as query
- [Phase 20]: Plan 20-02: on_pre_compress fire site placed inside ContextEngine.compress_messages (not at caller boundary) to structurally guarantee D-23 ordering; trait-level contract test in ironhermes-core locks the ordering into a regression test reusable by any future provider crate
- [Phase 20]: Plan 20-04: file-provider get_config_schema written in memory_provider.rs (actual impl site from 20-01), not memory_store.rs; tests placed in memory_store.rs tests mod with qualified trait syntax
- [Phase 20]: Plan 20-04: ConfigField.description is Option<String> — all 4 providers use Some("...".to_string()); assertion helper uses is_some_and non-empty
- [Phase 20]: Plan 20-04: sqlite_mirror_fixture uses Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>> SharedProvider (per 20-02), not Box<dyn>+parking_lot as plan samples showed; no new dep
- [Phase 20]: Plan 20-04: DuckDB threads field declarative only — wizard prompts+persists; PRAGMA threads=N runtime wiring deferred
- [Phase 20]: Plan 20-03: scripted-stdin D-23 integration test uses always-present file provider (3 defaulted fields) instead of cfg-gated TestProvider — zero new code surface, full wizard round-trip still covered
- [Phase 20]: Plan 20-03: run_memory_setup_with_io<R: BufRead, W: Write> is the pure testable core; public run_memory_setup(&Cli) is a thin wrapper that locks real stdin/stdout
- [Phase 20]: Plan 20-03: Fix 2 closure — run_chat and run_single now build MemoryManager + register_memory_tool + set_memory_manager + delegate_task memory slot; CLI reaches gateway parity for cross-invocation memory persistence
- [Phase 20]: Plan 20-03: static-grep regression test (run_chat_and_run_single_both_wire_memory_manager) locks the three wiring calls in main.rs against future refactor regressions
- [Phase 21]: TuiHandle uses shutdown(self) consuming self — Wave 3 wraps in Arc<TuiHandle> per W3
- [Phase 21]: ActivityState::Thinking absent (W6) — only Idle/Streaming/ToolCall{name}
- [Phase 21]: dead_code suppressed in tui/mod.rs with module-level allow — removed in Wave 3 on wiring
- [Phase 22]: Mirrored run_gateway registration order exactly for consistency and maintainability
- [Phase 22]: Separated Arc::new(registry) into explicit statement in run_single for pattern consistency
- [Phase 22]: hooks_config kept in scope for Plan 02 HookRegistry construction
- [Phase 22]: HookRegistry construction block identical across run_chat, run_single, and run_gateway for maintainability
- [Phase 22.1]: TuiExtension name() has no default — every extension must name itself for debug logging and widget ID prefixing
- [Phase 22.1]: catch_unwind with AssertUnwindSafe wraps all extension calls in dispatch_command for T-22.1-03 panic containment
- [Phase 22.1]: format_help accepts Option<KeybindingRegistry> — None-safe; Plan 02 wires the real registry
- [Phase 22.1]: reserved_rows() is the single source of truth for DECSTBM row count; all five hardcoded saturating_sub(3) calls replaced
- [Phase 22.1]: render_loop takes mpsc::UnboundedReceiver<TuiEvent> directly; zero-extension case passes empty collections (no Option wrapper)
- [Phase 22.1]: build_scanner_frame() delegates to knight_rider::frame() when colors match defaults for exact Phase 21 output fidelity
- [Phase 21.1]: match-on-name dispatch in handlers.rs (no trait objects) per RESEARCH.md Open Question 1
- [Phase 21.1]: CommandResult re-exported as SlashCommandResult to avoid ambiguity with crate::error::Result
- [Phase 21.1]: q alias assigned to quit (not queue) per hermes-agent exit priority
- [Phase 21.1]: CommandContext kept minimal: platform, session_id, agent_running (required) + skill_registry (optional)
- [Phase 21.1]: map_core_to_tui detects quit/clear by well-known message strings since TUI CommandResult has no Quit/ClearSession variants
- [Phase 21.1]: /start gateway behavior preserved by checking def.name == 'start' in NewSession arm
- [Phase 21.1]: SessionKey::to_string_key() used instead of to_string() (no Display impl on SessionKey)
- [Phase 21.3]: tiktoken-rs 0.11.0 singletons return &'static CoreBPE (lazy_static), not Arc<RwLock<CoreBPE>> -- no .read() needed on singleton references
- [Phase 21.3]: 37 models in static table across 7 families (Claude, GPT, Llama, Gemini, Mistral/Mixtral, DeepSeek, Qwen); helper functions cl100k()/o200k() keep table DRY
- [Phase 21.3]: D-06 precedence: context_length() on ResolvedEndpoint checks config.yaml first, then model metadata, then DEFAULT_CONTEXT_LENGTH
- [Phase 21.3]: Hysteresis test recalibrated for tiktoken: wider threshold band and dynamic filler to be robust against both BPE and heuristic counting
- [Phase 21.3]: Pure parse functions take serde_json::Value for testability; OpenRouter entries override models.dev for same key (richer tokenizer data)
- [Phase 21.3]: tokio promoted from dev-dep to dep in ironhermes-core for block_in_place in slash command handler
- [Phase 21.3]: Minimal 3-line change in ProviderResolver::build() (let mut + load + merge_cache) auto-loads disk cache for all runtime entry points
- [Phase 21.4]: GAP-2 fix must apply with_memory_manager() inside build_context_engine before Arc::new() — method is on concrete types only, not ContextEngine trait
- [Phase 21.4]: GAP-4 fix uses Option<Arc<...>> return from build_memory_manager (not no-op sentinel) — all consumers already guard on if let Some
- [Phase 21.4]: MEM-06 is VERIFIED correct — frozen snapshot pattern implemented and tested in all 3 entry points (run_single, run_chat, run_gateway)
- [Phase 21.4]: Return Option<Arc<Mutex<MemoryManager>>> from build_memory_manager so callers handle disabled state uniformly
- [Phase 21.4]: Apply with_memory_manager() on concrete engine types inside build_context_engine before Arc::new() — method not on ContextEngine trait
- [Phase 21.4]: Add memory_manager as last parameter to build_context_engine and attach_context_engine with None at all existing call sites for backward compat
- [Phase 21.4]: on_session_end fires with MemoryEntries::default() best-effort in run_single and run_chat clean exit; ctrl-c path intentionally skips (async unsuitable)
- [Phase 21.4]: memory_cmd.rs exposed in lib.rs for test access; memory_setup.rs remains binary-only (references crate::Cli)
- [Phase 21.5]: load_provider_config is module-private helper; per-arm config loading pattern in factory match; Arc<Mutex<Connection>> for SQLite spawn compat
- [Phase 21.5]: sanitize_fts_query tokenizes and double-quotes each term for FTS5 MATCH injection prevention
- [Phase 21.5]: conversation_extracts table created lazily in on_pre_compress (not in schema.rs)
- [Phase 21.5]: system_prompt_block reads live DB (not frozen snapshot) for contextual awareness
- [Phase 21.5]: extract_entity_triples uses heuristic pattern matching (not regex) for entity extraction; GrafeoDB interior mutability enables &self graph mutation in on_pre_compress/sync_turn
- [Phase 21.5]: DuckDB fire-and-forget bridge commands (SyncTurn/OnPreCompress/QueuePrefetch) have no respond channel; errors logged via tracing::warn
- [Phase 21.5]: memory_provider_tool_names is a HashSet populated once in run() from memory_manager.get_tool_schemas() -- avoids re-querying on every tool call
- [Phase 21.6]: Rust 2024 edition requires unsafe blocks for env var mutation in tests -- used unsafe with SAFETY comments and --test-threads=1 constraint
- [Phase 21.6]: gosu from tianon/gosu:1.19 image (not apt) per RESEARCH.md anti-pattern
- [Phase 21.6]: debian:bookworm-slim over distroless (needs python3 + bash for entrypoint)
- [Phase 21.6]: chmod 600 on .env in entrypoint for credential protection (T-21.6-06)
- [Phase 21.6]: install.sh downloads from GitHub Releases first, falls back to cargo install for end users
- [Phase 21.6]: setup-ironhermes.sh uses ln -sf symlink for rebuild-friendly developer workflow
- [Phase 21.8]: Plan 01 — pub use sanitize::{...9 functions} deferred from Task 1 to Task 2 (empty stub cannot satisfy pub-use of unwritten functions)
- [Phase 21.8]: Plan 01 — sanitize_name preserves underscore (NON_SAFE=[^a-z0-9._]+) while to_skill_slug strips it (NON_SLUG=[^a-z0-9-]); contrast locked via test
- [Phase 21.8]: Plan 01 — C1 control byte tests use \u{XX} not \xXX (Rust string-literal restriction for bytes > 0x7f)
- [Phase 21.8]: Plan 01 — pre-existing workspace clippy warnings outside scope; logged to deferred-items.md and left unmodified
- Phase 21.8 Plan 02: Added GitHubSource::auth() accessor (Rule 2) — sibling adapters reuse Phase 19.1 auth machinery
- Phase 21.8 Plan 02: Hand-rolled urlencoding() helper (~15 lines) in blob.rs — zero new workspace deps mandate
- Phase 21.8 Plan 02: SkillsShBlobSource.{github_api_base,raw_content_base} test-override fields added (Rule 2) — plan 05 wiremock needs all three hops redirectable
- Phase 21.8 Plan 02: ENV_LOCK is per-module — lock.rs + manifest.rs can race on HERMES_HOME under parallel tests; passes 100% single-threaded
- Phase 21.8 Plan 03: added bundle_folder_hash helper (Rule 2) — drift detection against SkillLockEntry.computed_hash must use D-13 no-separator algorithm; bundle_content_hash (0x00-separated) cannot substitute
- Phase 21.8 Plan 03: UpdateOutcome.old_hash surfaces SkillLockEntry.computed_hash (D-13 folder hash) not pre-21.8 bundle_content_hash; tests updated to read old_hash from lock
- Phase 21.8 Plan 03: AuditUrlGuard uses Drop-implementing RAII struct so MutexGuard lifetime extends across .await points in wiremock-backed async tests
- Phase 21.8 Plan 03: migrate_from_hub_manifest called idempotently at top of install/update/uninstall — covers both CLI and agent-tool paths with one placement
- Phase 21.8 Plan 03: extract_owner_repo returns empty for https:// / well-known: / <2-segment identifiers; caller treats empty as 'do not audit'
- Phase 21.8 Plan 04: CLI Task 1 swaps (import + line 136 + doc comments) landed with the hub-level deletion to keep the workspace compiling — plan's Task 1 acceptance grep demands both call-site swaps in one assertion
- Phase 21.8 Plan 04: D-21 lines 2 (Discovering) and 3 (Downloading) emit with identifier/0 placeholders — installer doesn't surface owner/repo + byte count before install() returns; deferred to plan 05 wiremock wiring
- Phase 21.8 Plan 04: added pub format_error_clean wrapper around strip_terminal_escapes — D-16 print-boundary contract needs a testable seam that doesn't capture process stderr
- Phase 21.8 Plan 04: migrate_from_hub_manifest called belt-and-braces at the top of cmd_install, cmd_update, cmd_remove, AND cmd_list_impl — installer.rs already calls it; idempotent second run is a plan 03 invariant
- Phase 21.8 Plan 05: Rule 2 — added GITHUB_API_BASE + GITHUB_RAW_CONTENT_BASE env overrides to SkillsShBlobSource::new with https_only relaxer when any override uses http://; needed for subprocess CLI round-trip test against wiremock
- Phase 21.8 Plan 05: Rule 1 — plan referenced CARGO_BIN_EXE_hermes but the binary is named ironhermes per [[bin]] name; subprocess test uses CARGO_BIN_EXE_ironhermes
- Phase 21.8 Plan 05: integration-test identifiers align with sample_tree_json('ascii-art/SKILL.md') fixture — all tests use foo/bar/ascii-art so hops 1+2 succeed and the intended error path is reached at hop 3
- Phase 21.8 Plan 05: any_file_named recursive walker hand-rolled in tests; walkdir NOT added to dev-deps per plan's zero-new-workspace-deps guarantee
- Phase 21.8 Plan 05: expected_happy_path_hash computed inline from known fixture bytes with sha2 (workspace dep) — avoids exposing installer-private helpers for test-only hash computation
- G-01: post-install server-vs-client hash equality is ADVISORY (log-only, never fails install) — D-14 opaque contract enforced in code
- G-02: HubErrorKind::ShaMismatch variant KEPT, narrowed to drift-detection semantics only; no longer raised on server/client parity
- rmcp feature flag is transport-streamable-http-client (not transport-streamable-http) — verified from crates.io API
- mcp_servers in Config stored as HashMap<String, serde_yaml::Value> to avoid circular dep; parsed by ironhermes-mcp at runtime (D-21)
- Phase 21.2 Plan 02: rpc_registry in execute_code.rs preserved as Arc<ToolRegistry> (no RwLock) — read-only safe subset per D-10 Pitfall 3
- Phase 21.2 Plan 02: tokio::sync::RwLock used throughout for Arc<RwLock<ToolRegistry>> (not std::sync) for async compatibility
- transport-streamable-http-client-reqwest feature required for from_uri (not -client alone) — reqwest backend required, confirmed from rmcp source
- CallToolRequestParams is non_exhaustive: must use .new().with_arguments() builder pattern
- ServerTaskResult.failure_reason data contract: sanitized error string when retries exhausted, None on clean cancellation (Plan 04 D-12 dependency)
- McpReloader trait in ironhermes-core/commands/context.rs (not ironhermes-mcp) avoids circular dep; matches MemoryManagerHandle pattern from Phase 20
- dyn McpReloader coercion before reload() call to disambiguate from McpManager::reload(new_configs) concrete method
- build_mcp_manager() helper extracted — DRY across run_chat, run_single, run_gateway wiring sites
- colored crate limitation: pad raw string before colorizing ({:<N} format width requires &str/String, not ColoredString)
- mcp_config.rs module in ironhermes-cli: all 5 hermes mcp subcommands live here (D-14)
- Phase 21.2 Plan 06: attempt_connect_and_list_with_timeout wraps tokio::time::timeout around config.connect_timeout (default 60s); used at all 3 call sites (cmd_add/cmd_test/cmd_configure) closing GAP-1
- Phase 21.2 Plan 06: RetrySaveAbort 3-way prompt defaults to Abort (return Ok(()) with 'Cancelled.' dimmed before save); SaveAnyway keeps legacy vec![],0 escape hatch but requires explicit consent; Retry re-enters the connect loop — closes GAP-2
- Phase 21.2 Plan 06: literal-copy regression tests via include_str! lock user-facing prompt strings against silent drift (GAP-2 + GAP-3 regression tests)
- Phase 21.2 Plan 07: sanitize_server_name is single source of truth; make_prefixed_name delegates to it; sanitizer now covers @ and / in addition to - and . — closes GAP-4 / CR-01 with symmetric register/unregister contract
- Phase 21.2 Plan 09: GAP-6a close — tracing init now branches on interactive REPL (Chat subcommand OR bare hermes WITHOUT -e) vs other entry points; interactive default = EnvFilter::new("error"); non-interactive keeps legacy ironhermes=info add_directive; RUST_LOG always wins via from_default_env
- Phase 21.2 Plan 09: GAP-6b close — cmd.stderr(std::process::Stdio::piped()) inside connect_stdio's configure closure so child stderr no longer inherits parent terminal fd; inline std::process::Stdio::piped() spelling kept (no top-of-file use import) to match grep acceptance verbatim
- Phase 21.2 Plan 09: runtime regression test spawns std::process::Command directly (not TokioChildProcess) to isolate the Stdio::piped() contract — zero dependency on a live MCP server; cfg(unix)/cfg(not(unix)) split covers macOS+Linux+Windows
- Phase 21.2 Plan 09: dotenv + ensure_home_dirs + Cli::parse() moved ABOVE tracing_subscriber::init so the filter branch can read cli.command / cli.execute; clap derive parse is pure/idempotent — safe reorder
- Phase 21.2 Plan 11: GAP-8 close — Option B plan-blessed fallback chosen per orchestrator directive. connect_stdio returns (RunningService, None) for Child handle under rmcp 1.5; cmd.kill_on_drop(true) in configure closure + tokio::time::timeout(Duration::from_secs(2), handle) in McpManager::shutdown_all together guarantee ironhermes gateway exits in ~2s/server on Ctrl+C. Signature change + child_slot plumbing STILL land so Option A upgrade is a single-line future delta.
- Phase 21.2 Plan 11: McpManager.tasks value tuple widens from (JoinHandle, CancellationToken) to 3-tuple (JoinHandle, CancellationToken, Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>); cleaner than second map lookup; Drop impl pattern widens to 3-tuple sync-safe (sync-context can't await Child::kill).
- Phase 21.2 Plan 11: GatewayRunner::start calls mcp_manager.shutdown_all().await BEFORE self.cancel.cancel() and BEFORE JoinSet drain — enforced by source-grep regression test locking call-site < propagation-anchor ordering.
- Phase 21.2 Plan 11: Rule 3 auto-fix — crates/ironhermes-cli/src/mcp_config.rs destructures (client, child) tuple after connect_stdio/connect_http signature change; dropping child at end-of-scope relies on kill_on_drop(true) for stdio reaper.
- Phase 21.2 Plan 11: regression test shutdown_all_returns_within_timeout_when_stdio_child_blocks pass time 2.51s vs 5s outer bound proves kill_on_drop + 2s JoinHandle timeout fully close GAP-8 at user-facing level under Option B.
- 22.3-01: suggest_typo placed in commands/typo.rs (not inline in handlers.rs) for testability per CONTEXT Claude's Discretion
- 22.3 Plan 02: tempfile variant A chosen — tempfile already a dev-dep in ironhermes-agent; no new dep per Phase 21 D-18
- 22.3 Plan 02: touch() fires BEFORE SubagentRegistry::register — corrects CONTEXT D-07 inversion per RESEARCH §Wiring Sites §1
- 22.3-03: Corrected rustyline 15 API — set_history_ignore_dups(true) not set_history_duplicates(HistoryDuplicates::Prev) (non-existent)
- 22.3-03: Added use rustyline::config::Configurer import (Configurer trait required for history config methods)
- ResetTerminal is unit variant in both enums (no String payload) per UI-SPEC CLR-7 — /clear produces no output text
- Gateway gets silent no-op ResetTerminal arm — no TTY in gateway context, added for exhaustiveness
- reset_terminal_visual placed in render.rs between prompt_position_ansi and prepare_prompt_with_reserve
- ALIAS-2 error-copy deferred to plan 22.3-06 — variable context mismatch at handlers.rs:254
- 22.3-06: INV-22.3-02 uses print_banner(); (with semicolon) to count only call sites, not fn definition or doc comments
- 22.3-06: INV-22.3-05 wrong-API guards use receiver-call form rl.set_history_duplicates( and import form to avoid false-positive on Plan 22.3-03 educational comment at repl_input.rs:249
- 22.3-08 (WR-02): stdout flush prepended to reset_terminal_visual BEFORE is_tty guard — unconditional flush drains buffered streaming tokens even when stderr is piped but stdout is a TTY
- 22.3-08: flush placed AFTER in-function `use std::io::Write as _;` (trait import already present) and BEFORE `let mut out = stderr();` — zero new use-statements, zero reordering of pre-existing lines
- 22.3-09 (WR-03): ReplInputChannel::shutdown signature widened from `self` to `mut self` (binding-mode only — no call-site edits required) so `self.worker.take()` can extract the JoinHandle for explicit `handle.join()` after the Shutdown command send; send-then-join ordering is mandatory because the worker is blocked on `cmd_rx.blocking_recv()` until it observes Shutdown
- 22.3-09: in-body `// Phase 22.3 WR-03` tag added (in addition to doc-comment WR-03 reference) so the acceptance awk range from signature to closing brace contains the marker — future readers can trace the fix without chasing doc-comment refactors
- 22.3-11 (GAP-22.3-01): wrote pub fn write_into_scroll_region(bytes, reserved) in tui/render.rs wrapping DECSC + absolute CUP to scroll_end + payload/flush + DECRC; non-TTY + tiny-terminal fallbacks collapse to plain stdout write; re-exported through tui/mod.rs
- 22.3-11: run_chat with_streaming callback + post-turn Hermes: label both routed through write_into_scroll_region using tui_stream.reserved_row_count() / tui.reserved_row_count(); run_single's streaming callback left untouched per CONTEXT D-15 scope (no persistent rustyline prompt to clobber)
- 22.3-11: DECSC (\x1b7) / DECRC (\x1b8) / DECSTBM (\x1b[1;) byte sequences remain encapsulated in tui/render.rs only — main.rs contains zero inline escape bytes; acceptance grep guards enforce this invariant for future refactors
- 22.3-12: INV-22.3-07/08/09 use raw string literals r"\x1b[1;" for source-text grep — include_str! loads source TEXT so escape literals render.rs compile-time produces are 7-char ASCII not ESC byte
- 22.3-12: sibling test file invariants_22_3_streaming.rs (not appended to invariants_22_3.rs) preserves Plan 22.3-06 closed 6-test deliverable per preservation gate
- 22.3-12: INV-22.3-08 scope-sanity asserts print!("{}", delta) is STILL present in main.rs (run_single at ~528) — catches future out-of-scope regression per CONTEXT D-15
- [Phase ?]: CronJobReader trait defined in ironhermes-core to avoid circular dep with ironhermes-cron
- [Phase ?]: CLI renderers in cron.rs delegate to shared ironhermes-cron::display formatters (D-06 shared renderer)
- [Phase ?]: App.cron_store defaults to None per D-02 — gateway is primary cron host; runtime load deferred
- [Phase ?]: Option 2 Arc<TokioMutex<JoinSet>> for worker_join_set — dispatch async move makes &mut borrow infeasible
- [Phase ?]: Path B (synthetic JoinSet drain test) for gateway_drains_workers_within_timeout — full GatewayRunner requires live TG token per RESEARCH §6
- [Phase ?]: Phase 24 Plan 01: validate_profile_name returns Result<String, ProfileNameError> (D-17 plain-String cross-crate convention)
- [Phase ?]: Phase 24 Plan 01: PROFILES_SUBDIR = 'profiles' constant in ironhermes-core::constants, re-exported via pub use constants::* (D-04)
- [Phase 24]: Plan 03: dotenvy moved AFTER resolve_and_set_profile — Config::env_path() calls get_hermes_home(), so dotenvy must run after pivot or it loads from wrong home (Pitfall 1 fix)
- [Phase 24]: Plan 03: dirs added as runtime dep (not dev-dep) since resolve_and_set_profile calls dirs::home_dir() at process startup
- [Phase 24]: Plan 03: --profile uses global = true (D-07, works on all subcommands incl. gateway run); --yolo uses global = false to exclude gateway
- [Phase 24]: Plan 03: Phase 23 preflight gate condition byte-for-byte unchanged per 23-VERIFICATION.md lock
- [Phase 24]: Plan 04: Step 0 PID lock prepended to GatewayRunner::start() as first statement; existing steps 1..N renumbering skipped (minimal diff)
- [Phase 24]: Plan 04: _pid_guard RAII binding kept across full start() body — Drop removes gateway.pid on clean return, error propagation, and future drop
- [Phase 24]: Plan 04: gateway_pid.rs uses i32::MAX as u32 (not u32::MAX) for guaranteed-ESRCH stale PID — inherits Plan 02 fix (u32::MAX wraps to POSIX kill(-1,0) returning Live on macOS)
- [Phase 24]: Plan 04: gateway_pid.rs passes &Path directly to acquire_pid_lock (no env_lock/set_var) per RESEARCH §Pitfall 6
- [Phase ?]: Phase 25.2 Plan 00: Used pdf-extract = { workspace = true } (workspace pin 0.10) — overrode CONTEXT D-24 reference to 0.7 per RESEARCH.md verified versions
- [Phase ?]: Phase 25.2 Plan 00: Deferred pub use web_extract::WebExtractTool; re-export to plan 25.2-13 — Wave 0 stub has no struct yet
- [Phase ?]: Phase 25.2 Plan 00: Used pub(crate) on env_lock/EnvGuard in tests/web_extract_integration.rs so plan 25.2-14 sibling test modules can reference them
- [Phase ?]: Phase 25.2 Plan 02: ExtractConfig inserted between BrowserConfig and GatewayConfig (D-23 file-layout); 'summarization' added as 6th RESERVED_ROLE_NAME (Phase 25.2 D-13)
- [Phase ?]: Phase 25.2 Plan 02: validate_role_name body unchanged — iterates RESERVED_ROLE_NAMES so summarization auto-accepts; validate_role_name_accepts_all_reserved_roles auto-covers new role
- [Phase ?]: Phase 25.2 Plan 02: AuxiliaryConfig cascade doc + RESERVED_ROLE_NAMES doc + validate_role_name doc all updated to 'six roles' for cross-site coherence (Rule 2 critical-correctness add)
- [Phase ?]: Phase 25.2 Plan 03: SummarizationClientHandle trait lives in ironhermes-core (not ironhermes-tools) — sibling crates need to reference the contract from both directions; tools holds Arc<dyn ...>, agent impls; same cycle-break logic as Phase 20 MemoryManagerHandle
- [Phase ?]: Phase 25.2 Plan 03: Used fully-qualified #[async_trait::async_trait] form rather than adding 'use async_trait::async_trait;' import to provider.rs — keeps import block byte-stable; async-trait already a workspace dep on ironhermes-core
- [Phase ?]: Phase 25.2 Plan 03: Compile-only dyn-compatibility test (Arc<dyn SummarizationClientHandle>) locks Send + Sync bounds against future regressions
- [Phase 25.2]: Plan 04: Used std::sync::OnceLock for one-time Regex compile (not LazyLock) — std-only, MSRV-compatible
- [Phase 25.2]: Plan 04: Hand-rolled percent_decode_lossy (15 lines, lifted from Phase 21.8 Plan 02) — preserves D-25 zero-new-deps mandate
- [Phase 25.2]: Plan 04: contains_secret builds combined haystack 'lower_orig + lower_decoded' so single .contains() pass covers raw + percent-encoded URL forms
- [Phase 25.2]: Plan 04: secret_url_patterns_const_contains_required_entries asserts EXACTLY 9 patterns to lock count against silent additions
- [Phase ?]: Phase 25.2 Plan 05: classify_url uses url::Url::parse + host_str() lowercase compare against literal allow-list (matches ironhermes-core::ssrf:16 pattern); evil-youtube.com correctly classifies as Web (T-25.2-host-spoof mitigation)
- [Phase ?]: Phase 25.2 Plan 05: select_backend() reads env vars at call time per web_read.rs:550 pattern; FIRECRAWL > EXA > TAVILY > Local; no caching to allow Plan 14 env_lock-coordinated tests
- [Phase ?]: Phase 25.2 Plan 05: Rule 3 auto-fix added url = { workspace = true } to ironhermes-tools Cargo.toml — workspace already pinned url = 2 at root for ironhermes-core::ssrf; tools crate just lacked the consumer line
- [Phase ?]: Phase 25.2 Plan 05: reroute_for_pdf() splits on ';' first to isolate primary content type; tolerates 'application/pdf; charset=binary' parameter variants without an extra mime crate dep
- [Phase ?]: [Phase 25.2 Plan 06]: ExtractionResult struct exported from web_extract crate root with plain-String error envelope (D-02 / Phase 22.4.2.2 D-18 cross-crate convention)
- [Phase ?]: [Phase 25.2 Plan 06]: fetch_with_firecrawl mirrors web_read.rs:171-248 verbatim except return type (Result<ExtractionResult>) — Err on backend failure so Plan 13 dispatcher falls through chain
- [Phase ?]: [Phase 25.2 Plan 06]: FIRECRAWL_ENDPOINT_OVERRIDE env var (Phase 21.8 Plan 02 SkillsShBlobSource pattern) — single env var, plain-String, Plan 14 wiremock testable
- [Phase ?]: [Phase 25.2 Plan 06]: D-07 Option B (inline Markdown header) locked across all backends — matches web_read.rs:159 precedent
- [Phase ?]: [Phase 25.2 Plan 06]: SSRF pre-validation runs as line 1 of fetch_with_firecrawl body (T-25.2-03 mitigation enforced structurally)
- [Phase ?]: [Phase 25.2 Plan 07]: fetch_with_exa uses .header("x-api-key", &api_key) (NOT bearer_auth) per Exa API docs verified 2026-05-01 — auth-divergence point vs firecrawl/tavily
- [Phase ?]: [Phase 25.2 Plan 07]: fetch_with_tavily derives title via derive_title_from_url() URL-path-last-segment fallback (Tavily returns no title field); strips .pdf and .html suffixes — covered by 4 unit tests
- [Phase ?]: [Phase 25.2 Plan 07]: EXA_ENDPOINT_OVERRIDE + TAVILY_ENDPOINT_OVERRIDE via private resolve_endpoint() helpers — byte-exact parity with firecrawl.rs:118-120 (Plan 06 SkillsShBlobSource pattern) for Plan 14 wiremock testing
- [Phase ?]: [Phase 25.2 Plan 07]: #[allow(dead_code)] on ExaStatus.status + TavilyFailedResult.url (Rule 2 critical-correctness) — preserves API schema parity without regressing 9-warning baseline; mirrors Plan 06 FirecrawlMetadata.status_code rationale
- [Phase 25.2]: Plan 08: LocalFetchOutcome { result, content_type, raw_bytes } enables Plan 13 dispatcher to mid-fetch reroute to PDF (D-03) without a second GET — bytes already on hand from body read
- [Phase 25.2]: Plan 08: Local backend pre-fetch + post-redirect SSRF re-validation (D-18) lifted byte-for-byte from web_read.rs:142-150; Content-Type primary-token parse tolerates 'application/pdf; charset=binary' variants without a mime crate dep (Plan 25.2-05 reroute_for_pdf precedent)
- [Phase ?]: [Phase 25.2 Plan 09]: Two public entry points (extract_pdf + extract_pdf_bytes) instead of fn-with-Option<Vec<u8>> — call sites in Plan 13 are syntactically distinct (UrlClass::Pdf arm vs reroute arm)
- [Phase ?]: [Phase 25.2 Plan 09]: PDF_MAX_BYTES=50MB and PDF_EXTRACT_TIMEOUT_SECS=30 hard-coded as module consts (not Config fields) — RESEARCH.md threat T5 + Assumption A3 give fixed bounds; config-tunable would invite operators to disable DoS mitigation
- [Phase ?]: [Phase 25.2 Plan 09]: Bytes-cap check duplicated in extract_pdf_bytes (in addition to fetch_pdf_bytes) — defends against Plan 08's mid-fetch reroute path where bytes arrive from outside this module; belt-and-braces
- [Phase ?]: [Phase 25.2 Plan 09]: Three-arm Result destructure on timeout/spawn_blocking/extract chain produces distinct error envelopes (pdf_too_large / pdf_text_extraction_timeout / pdf_text_extraction_failed / pdf extract task panicked) — actionable telemetry
- [Phase 25.2]: Plan 10: D-10 YouTube dispatch via tokio::process::Command shell-out to youtube-content skill helper script (HYPHENATED, verified vs SKILL.md frontmatter). Phase 19 skills runtime has no programmatic execute API — on-disk helper IS the canonical extension point. URL passed as separate arg(url): no shell, no format-string, T-25.2-shell-injection mitigated by construction. 5/5 unit tests pass; integration coverage deferred to Plan 14.
- [Phase 25.2]: Plan 12: Inline backend chain in fetch_web_with_chain (Firecrawl→Exa→Tavily→Local) instead of select_backend() — gives per-fallthrough warn telemetry the enum-based selector cannot express
- [Phase 25.2]: Plan 12: tokio::spawn per URL (not futures::join_all) preserves Pitfall 6 ordering by tagging tasks with idx and sorting before serializing; per-URL panics map into ExtractionResult.error
- [Phase 25.2]: Plan 12 [Rule 1 fix]: Replaced .entered() with async-block + .instrument(span) in tiers.rs/chunked.rs — Plan 11's EnteredSpan was held across await and broke tokio::spawn Send bound
- [Phase 25.2]: Plan 14: AnyClientSummarizationHandle is verbatim port of AnyClientVisionHandle (any_client.rs:158-238); register_web_extract_tool wired in run_chat/run_single/run_gateway with parity guard test; smoke test uses ToolSchema 2-level shape (d.function.name, not d.name)
- [Phase 25.5]: Replaced mock STATUS_TEXT with dynamic config_summary data for /status handler — real model/provider/context displayed
- [Phase ?]: FROZEN.md committed in source Hexapod repo at 7ba53c1 — freeze is git-recorded per Claude's Discretion bullet 2
- [Phase ?]: Plan 26.2.1-14: GAP-07-R3 closed via Branch (c) live-DOM-diagnostic-driven CSS triple-guard (html-prefix specificity 0,2,1->0,2,2 + visibility/opacity !important); D-26.2.1-14-C diagnostic-first GAP closure pattern established
- [Phase ?]: Plan 26.2.1-14: GAP-09-R3 partially closed via .filter(|s| s.message_count > 0) post-filter in api.rs (D-26.2.1-14-B option i); D-26.2.1-14-D user-approved residual deferred to phase 26.2.12 (foreign-format directories with non-zero msg_count still leak)
- [Phase ?]: Plan 26.2.1-15 (round-4): scanlines feature removal across 7 source files; removal-guard test `scanlines_feature_is_fully_removed` added; legacy serde migration via default tolerant posture; D-26.2.1-15-A/B/C established
- [Phase ?]: Plan 26.2.1-15 (round-5): synonym closure — `.scan-bar` overlay (Plan 03 HudChrome) deleted from site.css + hud_chrome.rs; removal-guard test extended with 3 new asserts (`.scan-bar`, `scan-bar-move`, `class: "scan-bar"`); D-26.2.1-15-D in-place amendment / D-26.2.1-15-E textual-pattern guard preserved; lesson: textual-grep removal guards cannot catch synonyms — future feature removals should consider structural CSS pattern matching or wasm-bindgen-test runtime assertions
- [Phase ?]: D-05 Phase 32.2: clarify and send_message silently excluded from build_child_registry for ALL children
- [Phase ?]: D-06 Phase 32.2: execute_batch returns Err immediately on oversize batch, citing delegation.max_concurrent_children
- [Phase ?]: D-08 Phase 32.2: max_iterations per-call override wired in both execute paths; no upper cap per PROV-09
- [Phase 32.2-subagent-delegation-parity]: ChildRole defaults to Leaf on all parse failures — least privilege per T-32.2-10
- [Phase 32.2-subagent-delegation-parity]: effective_tools pre-pass adds delegate_task BEFORE the match loop — never after (RESEARCH Pitfall 1)
- [Phase 32.2-subagent-delegation-parity]: Depth threading via AgentSubagentRunner struct fields — SubagentRunner trait signature unchanged (RESEARCH Pitfall 6)
- [Phase 35.1-05]: run_skills_section early-return guard removed — create_dir_all guarantees dir exists; SkillRegistry handles empty dir gracefully
- [Phase 35.1-05]: find_project_skills_source checks IRONHERMES_SOURCE env var first, then walks current_exe() up to 10 levels — graceful None for production installs
- [Phase ?]: 36.17.3-01: MessageQueue<K> trait + QueueError relocated to ironhermes-core; SessionKey moved to core with back-compat re-export from gateway; SessionQueue impls MessageQueue<SessionKey> via String->MessageEvent adapter (peek omitted per Resolution 3)

### Roadmap Evolution

- Phase 22 added: CLI feature parity
- Phase 21.1 inserted after Phase 21: Slash Commands (INSERTED)
- Phase 21.2 inserted after Phase 21: MCP client tool and fold in slash commands related to MCP client use (INSERTED)
- Phase 21.3 inserted after Phase 21: Model metadata & models.dev — context lengths, token estimation (URGENT)
- Phase 21.4 inserted after Phase 21: Persistent Memory gap analysis verification (URGENT)
- Phase 21.5 inserted after Phase 21: Memory Provider Plugin (INSERTED)
- Phase 21.6 inserted after Phase 21: Port deployment setup files from hermes-agent (INSERTED)
- Phase 21.7 inserted after Phase 21: Multi-agent and autonomous agents and sandbox status (INSERTED)
- Phase 21.8 inserted after Phase 21: skill remote download and install from skills.sh (URGENT)
- Phase 22.3 inserted after Phase 22: REPL UX hardening (visual stability + reset + unified history) (URGENT)
- Phase 22.4 inserted after Phase 22: ratatui-backed REPL (tmon architecture) (URGENT)
- Phase 22.4.1 inserted after Phase 22.4: tui_rata handler re-port — closes Plan 22.4-07 §Handler Coverage deferral by routing dispatch_slash through ironhermes_core::commands::CommandRouter::resolve + existing registry handlers so every classic-TUI command works in the ratatui REPL (INSERTED)
- Phase 22.4.2 inserted after Phase 22.4: wire up slash commands — replace `Phase 22.4.x stub:` placeholders in `tui_rata` invoke_handler arms with real handlers delegating to owning subsystems (MemoryManager [Phase 20], SubagentRegistry, active_skills, session storage, McpManager); narrows generic `not yet wired` fallback (INSERTED)
- Phase 22.4.2.1 inserted after Phase 22.4.2: Cron cmds and telegram delivery broken (URGENT)
- Phase 22.4.2.2 inserted after Phase 22.4.2: Cron create defaults to TG origin when gateway active (whitelist len==1) (URGENT)
- Phase 22.4.2.3 inserted after Phase 22.4.2: fix the pre-existing INV-22.3-02 banner-bleed before milestone (URGENT)
- Phase 25.1 inserted after Phase 25: built-in browser tools — 11 tools for browser automation (URGENT)
- Phase 25.2 inserted after Phase 25: web extract tools (URGENT)
- Phase 25.3 inserted after Phase 25: session-workspace parity (URGENT)
- Phase 25.5 inserted after Phase 25: iron_hermes_ui (URGENT)
- Phase 25.6 inserted after Phase 25: replicate CLI web wiring (URGENT)
- Phase 26.1 inserted after Phase 26: Fix websocket error for chat (URGENT)
- Phase 26.2 inserted after Phase 26: Fix Dioxus ui session tabs (URGENT)
- Phase 26.3 inserted after Phase 26: chromiumoxide user-data-dir (URGENT)
- Phase 26.4 inserted after Phase 26.3: web ui side tabs panel (URGENT)
- Phase 26.4.1 inserted after Phase 26.4: config fix (URGENT)
- Phase 25.7 inserted after Phase 25: registering all skills in .ironhermes/skills and .ironhermes/optional-skills on install or commandline skills --scan <PATH> option (URGENT)
- Phase 21.8.1 inserted after Phase 21.8: local-dir-install bug — installer rejects dir path identifiers (USERNAME/download/<skill>/) and requires a tarball; bug surfaced in 21.8 post-completion UAT (URGENT)
- Phase 21.8.2 inserted after Phase 21.8.1: skills hot reload command (URGENT)
- Phase 21.8.3 inserted after Phase 21.8.2: tui-streaming-scroll-fix-and-scrollbar (URGENT)
- Phase 21.8.3.1 inserted after Phase 21.8.3: personality applied doesn't chage the llm responses (URGENT)
- Phase 27.1 inserted after Phase 27: Import Free_Hexapod gsd planning (URGENT)
- Phase 27.1.1 inserted after Phase 27.1: Safe Foundation — hexapod walk/stop/sensors (INSERTED)
- Phase 27.1.2 inserted after Phase 27.1.1: Navigation — rotate/head/buzzer (INSERTED)
- Phase 27.1.3 inserted after Phase 27.1.2: Expression + Skill Doc — LEDs + protocol reference (INSERTED)
- Phase 27.1.4 inserted after Phase 27.1.3: hexapod video and sonic stream capture for navigation (URGENT)
- Phase 27.1.4.1 inserted after Phase 27.1.4: gateway fallback gap (URGENT)
- Phase 27.1.4.1.1 inserted after Phase 27.1.4.1: fallback on transport errors not just HTTP status — classify_llm_error only falls back on HTTP-status errors, not Connection refused / connect timeout / DNS (URGENT)
- Phase 26.3.2 inserted after Phase 26.3: Chrome singleton user browser-profile (URGENT)
- Phase 26.5 inserted after Phase 26: tui_rata overlay layer + theming — modal-overlay primitive, Skin model (3 built-ins) + /skin wiring, session picker + model picker overlays; ports Ink-TUI UX into the in-process ratatui REPL (URGENT)
- Phase 26.6 inserted after Phase 26: tui_rata thinking panel + Skills Hub + rich prompts — togglable expanded thinking panel (knight-rider = collapsed view), browse-only Skills Hub overlay, rich approval/secret/sudo overlays; depends on 26.5 (URGENT)
- Phase 26.2.1 inserted after Phase 26.2: new web ui with wheel menu (URGENT)
- Phase 27.1.4.2 inserted after Phase 27.1.4.1.1: hexapod led_off fails (URGENT)
- Phase 32.1 inserted after Phase 32: Agent cron execution (URGENT)
- Phase 32.2 inserted after Phase 32.1: subagent delegation parity (URGENT)
- Phase 32.3 inserted after Phase 32: delegation agent runaway (URGENT)
- Phase 26.7 inserted after Phase 26.6: wire up web to real services (URGENT)
- Phase 26.7.1 inserted after Phase 26.7: Agents page live updates — periodic-poll baseline + TERMINATED-HOLD-N fade (N=5s), then ws-event-driven upgrade for <1s update latency (URGENT)
- Phase 32.3.1 inserted after Phase 32.3: fix delegate_task kill abort wiring — close shrike handle_map gap (residual bug surfaced during 26.7.1 Wave 2 UAT 2026-05-19) (URGENT)
- Phase 26.7.2 inserted after Phase 26.7.1: Sessions load session data (URGENT)
- Phase 26.7.3 inserted after Phase 26.7.2: Skills page - enable tab, search and toggle on-off features (URGENT)
- Phase 35 added: Cron subagent budget isolation (T-28.1-16) — follow-up from Phase 28.1
- Phase 35 edited: edited fields: title, goal, requirements — broadened to global per-subagent independent budgets (retire PROV-10); T-28.1-16 now a consequence
- Phase 35.1 inserted after Phase 35: hermes-agent install and setup parity (URGENT)
- Phase 36 added: Gateway running-agent guard wiring — completes GW-05 (re-opened after Phase 21.1 cross-AI review surfaced gap; codex HIGH-1)
- Phase 36.2 inserted after Phase 36: Agent loop & core parity — prompt caching, per-provider rate-limit tracking, usage/cost accounting, error classification (from iron-hermes-planning.md §2.1 non-PARITY items) (URGENT)
- Phase 36.3 inserted after Phase 36: Tools parity — vision/image/video gen, TTS/STT, computer_use, smart-home, kanban, planning tools, first-class send_message, multi-environment exec, browser CDP/dialog (from iron-hermes-planning.md §2.2 non-PARITY items) (URGENT)
- Phase 36.3.1-36.3.12 inserted after Phase 36.3: Split Phase 36.3 (Tools parity) into 12 per-tool-family sub-phases: vision, image gen, video gen, voice I/O, computer use, smart home, kanban, messaging/clarify, planning tools, browser polish, web search expansion, multi-env exec. See ROADMAP.md for full list. (URGENT)
- Phase 36.4 inserted after Phase 36: Skills library — bundle hermes-agent's 27 built-in + 18 optional skills; install via GitHub, migrate from hermes-agent, or openclaw local install (from iron-hermes-planning.md §2.3 — runtime/install exists, library does not) (URGENT)
- Phase 36.4.1-36.4.3 inserted after Phase 36.4: Split Phase 36.4 (Skills library) into 3 migration-path sub-phases: 36.4.1 GitHub tap + lock-file seed (fastest path); 36.4.2 hermes-agent Tier-1 port (highest fidelity); 36.4.3 openclaw catalog bridge (MCP). Paths are additive, not exclusive. (URGENT)
- Phase 36.5 inserted after Phase 36: Provider parity — closes hermes-agent's biggest tactical gap (OAuth provider proxy) plus enterprise/observability layer. Three targets: (1) OAuth provider — Claude Pro / ChatGPT Pro / SuperGrok device-flow auth; (2) Claude Compliance API for enterprise audit export — https://support.claude.com/en/articles/13015708-access-the-compliance-api; (3) Cloudflare AI Gateway as unified provider proxy — https://developers.cloudflare.com/ai-gateway/get-started/ (from iron-hermes-planning.md §2.4) (URGENT)
- Phase 36.6 inserted after Phase 36: TUI parity & visibility fix — TWO scopes: (1) UNRESOLVED BUG: AI responses still not rendering visibly in ratatui TUI (prior scroll-width work in feedback_scroll_width_inner.md did not fully resolve); (2) Ink-UX feature port to ratatui per project_tui_ink_ux_phases memory (overlays, pickers, skins, thinking panel, command palette, mode picker, model switcher, OSC8 hyperlinks; Telegram approval UX + voice remain deferred). Bug fix is urgent and must precede feature port (from iron-hermes-planning.md §2.5) (URGENT)
- Phase 36.6.1-36.6.4 inserted after Phase 36.6: Split Phase 36.6 (TUI parity & visibility fix) into 4 sub-phases: 36.6.1 BUG FIX AI response visibility (blocking, must ship first); 36.6.2 Ink-UX port — thinking panel + overlays; 36.6.3 Ink-UX port — input UX (command palette, mode picker, model switcher); 36.6.4 TUI polish (OSC8, skin engine, terminal compat). 36.6.1 gates the others — feature port is wasted effort if responses are invisible. (URGENT)
- Phase 36.7 inserted after Phase 36: Multi-platform gateway parity — 19 missing platforms vs hermes-agent (3 shipped: Telegram/Discord/Slack). Targets: WhatsApp, Signal, SMS, Email, Matrix, Mattermost, MS Teams, iMessage, LINE, SimpleX, DingTalk, Feishu, Wecom, WeChat (Weixin), QQ, Yuanbao, generic webhook, HTTP REST API, Home Assistant trigger. Likely needs split — recommend per-platform-cluster sub-phases (foundation/mainstream/privacy/mobile/APAC/automation) rather than per-platform to keep sub-phase count tractable (from iron-hermes-planning.md §2.6) (URGENT)
- Phase 36.7.1 inserted after Phase 36.7: Scoped Phase 36.7 down to foundation only: 36.7.1 ships generic webhook adapter + HTTP REST API server. The other 17 hermes-agent platforms (WhatsApp, Signal, SMS, Email, Matrix, Mattermost, MS Teams, iMessage, LINE, SimpleX, DingTalk, Feishu, Wecom, WeChat Weixin, QQ, Yuanbao, Home Assistant) are DEFERRED — not on active roadmap, no sub-phases created. Decision: ironhermes serves a narrower audience than hermes-agent's 22-platform breadth; webhook+REST covers any custom integration need. (URGENT)
- Phase 36.8 inserted after Phase 36: ACP adapter — Agent Client Protocol server. Currently a FULL gap in ironhermes (hermes-agent ships acp_adapter/ + acp_registry/ for Zed/VS Code/JetBrains via uvx). Stdio transport, tool listing + dispatch + streaming, approval-event surface, pairing-code auth, edit-approval UI. Identified in iron-hermes-planning.md §2.7 as the single biggest 'cannot switch from hermes-agent' blocker for editor-driven users. (URGENT)
- Phase 36.9 inserted after Phase 36: MCP server (server-side MCP) — currently a FULL gap in ironhermes. ironhermes-mcp crate is CLIENT only (consumes external MCP servers). Phase 36.9 ports hermes-agent's mcp_serve.py 9-tool surface: conversations_list, conversation_get, messages_read (FTS), attachments_fetch, events_poll, events_wait, messages_send, permissions_list_open, permissions_respond, channels_list. With this shipped, Claude Code / Cursor / any MCP-aware host can drive ironhermes the same way they currently drive hermes-agent. From iron-hermes-planning.md §2.8. (URGENT)
- Phase 36.10 inserted after Phase 36: Memory & state parity — narrow gap. ironhermes-state already ships SQLite + FTS5 (schema v8 WAL); three memory backends (sqlite/grafeo/duckdb) already pluggable; memory_tool, frozen-snapshot pattern, and ironhermes-trajectory ledger all in place. The single visible gap is no session_search tool wrapper exposing the existing FTS — likely 1 plan. Optional secondary scope: add managed-memory provider impls (honcho, mem0, supermemory) to mirror hermes-agent's memory plugin set. From iron-hermes-planning.md §2.9. (URGENT)
- Phase 36.11 inserted after Phase 36: Configuration & secrets parity — security-load-bearing gap. ironhermes currently reads credentials only from env vars + plaintext config; hermes-agent additionally supports AWS Secrets Manager, Bitwarden CLI, macOS Keychain. Phase 36.11 adds CredentialSource trait + 3 first-class implementations (Keychain/AWS/Bitwarden) and reserves room for Linux Secret Service, Windows Credential Manager, 1Password CLI. Pairs naturally with DEFCON scale work (project_security_defcon_scale) — strict DEFCON levels should refuse plaintext .env credentials. From iron-hermes-planning.md §2.10. (URGENT)
- Phase 36.12 inserted after Phase 36: Packaging & distribution parity — distribution-load-bearing. ironhermes currently ships: Dockerfile, install.sh + install-gitea.sh, launchd/systemd/cron deploy scripts, quick_setup_script.ps1 (Windows, status unverified). Gaps vs hermes-agent: Homebrew tap (macOS native), Nix flake (reproducible/NixOS), Termux/Android (constraints file equivalent), crates.io publication verification, Windows native install verification. Each is independently shippable — splitting per channel is reasonable. From iron-hermes-planning.md §2.11. (URGENT)
- Phase 36.13 inserted after Phase 36: Plugins & extensions — architecturally uncertain. Hermes-agent's plugins/ system (15+ bundled plugins, ctx.llm + tool_override primitives) overlaps significantly with ironhermes's existing skills + MCP + crate workspace patterns: memory providers (Phase 20 + 36.10), model providers (36.5), platforms (34 + 36.7), image/video gen (36.3.2/3), kanban (36.3.7), browser (shipped) all already covered through other extension mechanisms. REAL gaps that need attention regardless: observability hooks (Datadog/New Relic metrics+traces — no current substrate), lifecycle hooks beyond HookRegistry, ctx.llm + tool_override runtime primitives. Recommend treating this as a decision phase before committing to a plugin loader port. From iron-hermes-planning.md §2.13. (URGENT)
- Phase 36.13 edited: Phase 36.13 SCOPE LOCKED: Option A — REJECT plugin loader port. Decision rationale: hermes-agent's plugins/ loader is Python-dynamic-loading-shaped (cheap in Python, expensive in Rust); equivalent composability in ironhermes is achieved via crate workspace + skills + MCP, which already cover memory providers / model providers / platforms / image_gen / video_gen / kanban / browser. Only three primitives have no current substrate: observability export (Datadog/New Relic — to be ported as OpenTelemetry/OTLP), ctx.llm runtime override, tool_override runtime override. These ship directly on AgentRuntime, not via a plugin system. Decision will be ratified in an ADR landed in PROJECT.md / ARCHITECTURE.md. Aligned with 'ironhermes is its own thing' strategic posture and parallel narrowing of Phase 36.7 multi-platform gateway.
- Phase 36.15 inserted after Phase 36: Small Model Mode (SMM) — provider extra_request_options for num_ctx/top_k/etc. (URGENT)
- Phase 36.16 inserted after Phase 36: Small Model Mode architecture port (consumes 36.15 knob); seeded from SmallModelMode_ARCHITECTURE.md (URGENT)
- Phase 36.17 inserted after Phase 36: iron_hermes_ui web logging in $IRONHERMES_HOME/logs (URGENT)
- Phase 36.17.1 inserted after Phase 36.17: in-mem FIFO queuing parity of python deque for chat sessions (URGENT)
- Phase 36.17.2.1 inserted after Phase 36.17.2: fix /queue slash-command failing to wake parked worker (regression from 36.17.2 mpsc→Notify switch) (URGENT)
- Phase 36.17.3 inserted after Phase 36.17: wire up TUI with gateway queue and slash queue commands (URGENT)
- Phase 36.17.4 inserted after Phase 36.17: wire up iron_hermes_ui to the gateway queue + slash commands (URGENT)
- Phase 36.3.7.12 inserted after Phase 36.3.7.11: Goal mode - kanban worker loop (Ralph loop) — autonomous worker primitives on the kanban surface: goal_mode card flag, /goal worker dispatcher loop, DecomposeFn + SpecifyFn runtime closures wired into AgentRuntime, threat model for auto-advance. Companion to Phase 36.3.7.11's manual-review dashboard. (URGENT)
- Phase 36.17.2.2 inserted after Phase 36.17.2: IronHermes Telegram client delivers streaming final media messages (URGENT)
- Phase 36.17.5 inserted after Phase 36.17: integrate TTS functions (URGENT)
- Phase 37 added: RUSTSEC-2026-0104 reachable panic (urgent security advisory)
- Phase 37.1 inserted after Phase 37: setup script not working on macos (URGENT)

### Pending Todos

6 pending. Latest:

- [skills] Slash command integration SKILL-13 (2026-04-17)
- [tools] Tool registry improvements (2026-04-17)
- [cli] CLI feature parity (2026-04-17)
- [cli] Configuration and setup wizard improvements (2026-04-17)

### Blockers/Concerns

- **Default config deadlock (18-11 scope):** With `compression.protect_first_n=3` (documented default) and a [sys, user, asst-tool_use, tool_result] shape, the two-direction guard correctly collapses the prune range to zero — compression cannot fire. UAT only passed after lowering to 2. Fix: auto-extend/auto-shrink `protect_first_n` around tool-pair boundaries.
- **Post-compression retry loop (18-12 scope):** Live UAT saw the agent re-call `web_read` on every turn for 10 consecutive turns (hit MAX_COMPRESSION_PASSES), never returning a summary. `[CONTEXT HISTORY]` summary content does not convey tool-call completion, so the model treats every turn as a fresh request.

## Quick Tasks Completed

| Date       | Slug                       | Outcome                                                                                                  |
|------------|----------------------------|----------------------------------------------------------------------------------------------------------|
| 2026-05-17 | transparent-logo-asset     | Restored true PNG alpha on `crates/iron_hermes_ui/assets/i_hermes_logo.png` (removed baked-in checkerboard via 18%-fuzz floodfill). |
| 2026-06-02 | 260602-ds9                 | Closed BUG-1 (fetch_board `include_archived` parameter + ScreenKanban re-fetch on toggle + archived-fetch regression test) and BUG-2 (drawer + 4 modals cyan border + opaque tinted dark fill via existing tokens, zero new hex) from 36.3.7.11 UAT failures U2/U6/U7/U8. iron_hermes_ui test + clippy gates green. |
| 2026-06-02 | 260602-nd7                 | Closed U9 FAIL from 36.3.7.11 UAT (drawer COMMENTS section did not auto-refresh on cross-process `kanban comment` write — D-21 contract broken). Root cause was producer-side, not UI: `KanbanStore::add_comment` wrote `task_comments` but never appended a `task_events` row, so the dashboard tail (D-15) had nothing to broadcast and the drawer's per-task event counter never bumped. Fix appends `KanbanEventKind::Edited` with payload `{subkind:"comment", comment_id, author}` (events.rs frozen surface preserved per Phase 36.3.7.6) inside one rusqlite transaction wrapping both INSERTs. Bilateral regression coverage: 3 producer-end tests (`crates/ironhermes-kanban/tests/comment_appends_event.rs`) + 1 consumer-end byte-offset-localized test (`crates/iron_hermes_ui/tests/kanban_drawer.rs`). 4 new tests, all green. Deferred (out-of-scope, pre-existing at base d2e51d52): DEFER-1 = 2 e2e tests in `ironhermes-kanban/tests/end_to_end.rs` (env-var race in dispatcher); DEFER-2 = Rust 1.94 clippy lint upgrade — 37+ errors in code blamed to commits 0db139084 + 9cc4114d8. |

## Session Continuity

Last session: 2026-06-06T20:44:36.002Z
Stopped at: Phase 37.1 context gathered
Resume file: None
