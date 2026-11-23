---
type: adr
id: ADR-0007
title: OpenUI generative UI — go/no-go evidence for an in-app OpenUI screen
status: Accepted
date: 2026-10-29
owners: ["@bradwilson331"]
links:
  - ../../.planning/phases/54.9-menu-configuration-and-openui-integration/54.9-CONTEXT.md
  - ../../.planning/phases/54.9-menu-configuration-and-openui-integration/54.9-RESEARCH.md
  - ../../.planning/phases/54.9-menu-configuration-and-openui-integration/COVERAGE.md
  - ../../.planning/phases/54.9-menu-configuration-and-openui-integration/spike/measurements.log
  - ../../crates/iron_hermes_ui/src/server/artifact_route.rs
---

# ADR-0007: OpenUI generative UI

- **Status:** Accepted (Decision: GO, 2026-10-29, plan 07 checkpoint)
- **Date:** 2026-10-29
- **Deciders:** operator (@bradwilson331)
- **Scope:** whether iron_hermes_ui ships a dedicated OpenUI screen that renders model-authored OpenUI Lang, and by which mechanisms. This document is evidence, not production code. The in-app spike lives only on the local branch `spike/54.9-openui` (commit `5a84adc39acf34801acfbd93b87d5ca5892ad629`, parent `da17cb5771757e8f54428aa2526d1d5c396be788`; never pushed, never merged).
- **Evidence log:** every number below cites a `[TAG]` and line of `.planning/phases/54.9-menu-configuration-and-openui-integration/spike/measurements.log` (abbreviated `measurements.log`).

## Context

Phase 54.9 carries a research gate on its second half (CONTEXT D-09..D-12):

- **D-09** The operator's own providers are the primary generation path; OpenUI Lang is parsed and checked locally. The Thesys hosted Gateway (`api.thesys.dev`, `THESYS_API_KEY`) is allowed only as an opt-in, off by default, key in the vault, and the UI says content leaves the machine.
- **D-10** The deliverable is this ADR plus a throwaway spike that renders one generated view inside the running Dioxus app, so the React-island cost is measured rather than estimated.
- **D-11** Four candidate blockers are evaluated with evidence and no pre-weighting, and the user decides go or no-go: (1) licence, (2) bundle/asset cost, (3) CSP and sandbox compatibility, (4) offline prompt generation and parse/validate.
- **D-12** On no-go: ship the menu half only and record the findings in a backlog phase (999.x). No native Rust OpenUI renderer in this phase.

If GO, the operator-facing shape is D-13..D-15: a new screen with a prompt box, the reply renders as it streams, views are saved per profile, a component action becomes a new agent turn through the normal tool/guardrail/approval gates, and a view never calls a tool.

## Options

1. **Vendored prebuilt bundle (default).** Check in `@openuidev/browser-bundle@0.2.1` `dist/openui-bundle.min.js` and `dist/openui-styles.css` with LICENSE and a NOTICE; serve from a raw axum route inside a sandboxed iframe. No Node in the Cargo build; stock 84-component library.
2. **Custom esbuild build.** `npm install @openuidev/react-lang` plus own bundling: custom component library and a smaller bundle, but a Node build inside a Cargo repo and a larger dependency surface. Deferred until the stock library proves too big or ugly.
3. **Gateway-only.** Thesys generates and validates. Rejected as the primary path by D-09 (content leaves the machine, billed); kept as an opt-in only.
4. **No-go.** Ship the menu half only (D-12) and write the backlog entry (see "If no-go").

Deferred idea, not evaluated: a native Rust/Dioxus OpenUI Lang renderer (D-12 forbids prototyping it here). The spike diff contains no Rust parser for OpenUI Lang.

## Evidence

Measured on macOS 27.0.1, Node 24.14.0, Chrome 155, Firefox 158.0.1, dx 0.7.1, rustc 1.99.0. No pre-weighting: the Reading column states what was observed.

| Blocker | Measurement | Reading |
|---------|-------------|---------|
| 1. Licence | All four packages MIT, "Copyright (c) 2011-2024 Thesys Inc." in each tarball LICENSE (`[TRANSITIVE-LICENCES]`, measurements.log line 62). Production dependency closure of react-ui + react-lang + lang-core + react + react-dom + zod: 210 packages; 185 MIT, 15 ISC, 1 BSD-2-Clause, 1 BSD-3-Clause (highlight.js), 1 CC0-1.0 (highlightjs-vue), 1 0BSD, 2 resolved by hand to MIT (@ag-ui/core LICENSE file; format's legacy `licenses` array), 4 uninstalled optional peers. No copyleft, no unlicensed package. | No licence blocker found. **Obligation found:** the minified bundle embeds ZERO licence banners (`[BUNDLE-SCAN]`, line 44, LICENCE_BANNERS=0), so a vendored copy must ship the MIT text plus a NOTICE/THIRD-PARTY file for the MIT/ISC/BSD dependencies. The closure is a superset of what the bundle inlines; the bundle is opaque, so the audit trail is the upstream lockfile and the closure above. |
| 2. Bundle / asset cost | **Dioxus wasm** (release, dx): baseline 10,610,776 B, with the spike 10,621,732 B = **+10,956 B raw (+0.10%)**, +2,127 B gzip, +1,496 B brotli (`[WASM-SIZE]`, line 313). **Frame asset**: JS raw 3,230,025 B, CSS 396,572 B; gzip -9 total 932,168 B, brotli -11 total 746,212 B (`[SIZES]`, line 28). **As actually served by the app's own compression layer**: 3,630,203 B identity, 937,177 B gzip, 923,588 B br (`[FRAME-TRANSFER]`, line 323). **Time**: click on the Orb tab to bootstrap ready 289 ms cold, 146 ms warm (frame from HTTP cache); first openui-push to first rendered text 129-138 ms (`[IN-APP-RUN]`, line 338). Vendored-file SHA-256 pinned (`[VENDORED-FILE-SHA256]`, line 24). | The React island adds nothing meaningful to the wasm. Its cost is one ~0.9 MiB compressed download on first visit to the screen (cacheable: `Cache-Control: private, max-age=86400`), 3.6 MB of vendored files in git, and 53,352 prompt chars per generation (`[OFFLINE-PROMPT-AND-PARSE]`, line 34; roughly 13k tokens is the chars/4 estimate, not measured against a tokenizer). The compression layer's dynamic br is barely smaller than gzip; precompressing at br -11 would save about 20%. |
| 3. CSP / sandbox | Frame served with the verbatim `ARTIFACT_CSP` directives plus `frame-ancestors 'self'`, inside `<iframe sandbox="allow-scripts">`: renders and streams in Chrome (in-app and standalone) and Firefox 158; exactly **1 non-fatal report, `script-src` blocked=eval**; adding `'unsafe-eval'` removes the report with identical output, so it is not needed (`[HARNESS-RUNS]` runs A/B, line 366). No other directive reported a violation for the samples run (none of them used images or Card `sources`). A generated `@OpenUrl` button clicked and a direct `window.open` are inert (no new page target; `[A6-OPENURL]`, line 414). A message from any window other than the iframe is rejected by the host's `event.source` check; a sibling sandboxed iframe's push is ignored by the frame (SIBLING_RAN=true, applied=false). | Passes under the existing artifact CSP with no loosening. Chrome and Firefox only: **Safari is untested** (headless automation needs an admin step; `[BROWSER-RESULTS]`, line 446). Not tested: a Form submit round-trip, an oversize (>8192 B) message, framing by another origin. The CSP is also what blocks the bundle's Google favicon URLs (hostname leak) and remote images (`[BUNDLE-SCAN]`). |
| 4. Offline prompt + parse | `gen-system-prompt.mjs` writes `library.prompt(openuiChatPromptOptions)` = 53,352 chars with no network call; `createStreamingParser` on a good sample gives 0 errors, on a bad sample 1 (`unknown-component`) (`[OFFLINE-PROMPT-AND-PARSE]`, line 34). Two real model replies parsed locally with 0 errors (`[GENERATION-PARSE-AND-RENDER]`, line 473). The regenerated prompt is byte-identical to the one pushed to the model (PROMPT_IDENTICAL=true). `cloud: true` and Autofix were not called. | Offline generation and validation work. The parser reports and does not repair; "repair" is TEXT FALLBACK, PARTIAL, or regenerate with `meta.errors` fed back. The generator is JS-only, so the prompt is a build-time checked-in asset. Mid-stream, `onParseResult` shows transient `errors` and `unresolved`: judge them only after `openui-end`. |
| In-app render (D-10) | Real Dioxus app (release build of the spike branch), headless Chrome driving login, the SYSTEM wedge and Settings > Orb: framed view rendered progressively (11 distinct render steps over 20 chunks), the Rust web-sys listener ACCEPTED the frame's `openui-action` and REJECTED a forged same-shape message from the host window (`[IN-APP-RUN]`). | Works inside the running app. The spike host registered a second message listener on every remount (duplicate log lines); production must register one listener or remove it on unmount. |
| Found, not on the list | No `new Function(`, XMLHttpRequest, WebSocket, sendBeacon, posthog, or thesys.dev host in the bundle (`[BUNDLE-SCAN]`). The Card `sources` strip builds `https://www.google.com/s2/favicons?...domain=<host>` image URLs; the prompt advises `https://picsum.photos/...` images. | Both are image loads, harmless under `img-src data:`. The overlay should forbid image URLs, `Query()`/`Mutation()` and `@OpenUrl`. |

## Telemetry and supply chain

- **Package legitimacy.** Seam verdict SUS for all four packages (latest versions only days old; browser-bundle 658 weekly downloads). The mandatory blocking human check ran before any install: the operator replied "pass" on 2026-10-29 for exactly `@openuidev/browser-bundle@0.2.1`, `@openuidev/react-lang@1.0.2`, `@openuidev/lang-core@1.0.2`, `@openuidev/react-ui@0.18.1`. Evidence shown to the operator: all MIT, repository `git+https://github.com/thesysdev/openui.git`, sole maintainer `abhithesys <abhishek@thesys.dev>`, weekly downloads 658 / 533,429 / 695,686 / 126,455 (`[APPROVAL]`, line 6).
- **Install-time telemetry.** `@openuidev/lang-core` ships `postinstall: node ./postinstall.cjs`, which sends a pseudonymous install event to PostHog (`https://dgoeivjus9jfp.cloudfront.net/capture/`); opt-out is `OPENUI_TELEMETRY_DISABLED=1` or `DO_NOT_TRACK=1`. It never ran: every install used `--ignore-scripts` with both variables, into a scratch directory outside the repo. `@openuidev/browser-bundle` has no postinstall. `@openuidev/cli` depends on `posthog-node` and is not used.
- **SHA-256 pins** (tarballs; each also equals the registry SHA-1 `dist.shasum`, `[TARBALL-SHA256]`, line 15):
  - browser-bundle 0.2.1 `d4a9915ff3053fa388ea69e8dd165382de77611f2b414dadb0609b9236f45d06`
  - lang-core 1.0.2 `165597c31b551f5321be09be36d5210f547a943cf0bb6c6132b8974d6cd5dbe0`
  - react-lang 1.0.2 `1619705a7b4e50ee235923d9311a63dcd297d64687b3e936bac7a71b31dc75fc`
  - react-ui 0.18.1 `db14d4c2477f7cef87903fef4ad5087bc37be9353701ea7ceddcf2c720689ac9`
  - vendored files: `dist/openui-bundle.min.js` `bba936ff93f31a2955460f40d80bcd329b6a5848d6a05df54427e5dd137181df`, `dist/openui-styles.css` `93b69b218203bd054a88b7c8821d6c0dc6944af38119ada37e8082e59c0a18f3`
- **Runtime telemetry.** Separate and opt-in (`OPENUI_RUNTIME_TELEMETRY_ENABLED`); the bundle contains no telemetry host. The vendor's Reliability Monitoring is switched on automatically by Gateway and Autofix, a further reason both stay opt-in or off (COVERAGE.md).
- **Spike side effects on the operator's machine.** Booting the spike server against the real `IRONHERMES_HOME` also started the operator's configured MCP servers (retries against the `cloudflare_*` remote servers in the log) and created 2 chat sessions in `state.db` through the two generation probes; no MCP tool was called. The spike used a throwaway web login via `IRONHERMES_WEB_PASSWORD_HASH` (env outranks the vault hash); the operator's real credential was neither read nor changed. No request went to `api.thesys.dev`.

## Sandbox and bridge

This is a **narrow, one-way exception** to the artifact viewer's "no bridge" rule. `artifact_viewer.rs` states it "never passes ... a bidirectional `postMessage` bridge into the iframe"; the OpenUI screen needs the host to stream text in and the view to send a click out. The exception is limited to:

- **Sandbox:** `sandbox="allow-scripts"` only. No `allow-same-origin` (so the frame is an opaque origin and cannot reach the app's cookies, DOM or storage), no `allow-popups`, no `allow-top-navigation`, no `allow-forms`, no `allow-modals`.
- **CSP** (response header on the frame document): `default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'`. That is `ARTIFACT_CSP` verbatim plus `frame-ancestors 'self'`; `connect-src 'none'` blocks all outbound network from the frame.
- **Host to frame:** `postMessage({type:"openui-push", text})`, `{type:"openui-end"}`, `{type:"openui-reset"}`, `{type:"openui-theme", vars}` with target `"*"` (an opaque-origin frame cannot be addressed by origin). The frame accepts a message only when `event.source === window.parent`. Confirmed: a sibling sandboxed iframe's push was ignored (SIBLING_RAN=true, SIBLING_PUSH_APPLIED=false, `[HARNESS-RUNS]`).
- **Frame to host:** exactly one type, `{type:"openui-action", label, message, form_state}`. The host accepts it only when `event.source` equals the iframe's `contentWindow` and the payload is at most 8192 bytes. Confirmed in the real app: the Rust listener ACCEPTED the frame's action and REJECTED a forged message posted by the host window (`[IN-APP-RUN]`). The 8192-byte cap is coded in the spike listener but was not exercised with an oversize payload.
- **Changed vs the default (bootstrap):** `Renderer.onAction` also fires for `@OpenUrl` with `{type:"open_url"}` and no message; the spike forwarded it as `label:"open_url", message:""`. The production bootstrap must forward only the `continue_conversation` / `@ToAssistant` shape and drop every other action type. `@OpenUrl` itself is inert under this sandbox (`[A6-OPENURL]`).
- **Changed vs the default (host):** register ONE `message` listener (HermesApp root) or remove it on unmount. The spike used `use_effect` plus `Closure::forget` and duplicated every message after a remount. The `use_context_provider`-at-root rule in the crate CLAUDE.md applies to any context the listener writes into.
- **Parse states in the frame:** the Renderer exposes `onParseResult(result|null)` and `onError(errors[])`, so the frame can show TEXT FALLBACK and PARTIAL itself with no second outbound message type. Exact props: `onError` fires with "Response could not be parsed as valid openui-lang" for plain prose (and `onParseResult` has `root` null, nothing rendered) and with `Unknown component ...` for a bad component (which still renders an empty Card); an undefined child reference is NOT in `onError` but is `result.meta.unresolved.length > 0` (`[RENDERER-PARSE-SURFACE]`, line 431).
- **Not measured:** that the host can start pushing on the iframe `onload` event alone (inline scripts run before `load`; the spike used a `ready` message, which a build can drop). Theme: the frame renders the stock light theme inside a dark host; `openui-theme` CSS variables are the restyle path and were not exercised beyond the message handler.

## Generation path

Default being tested: the OpenUI screen opens its own `/api/ws/chat` socket, sends `ChatRequest { openui: true, .. }` on its own session, the server pushes the checked-in system prompt as a one-shot overlay via `AppState::push_web_skill_overlay` before the normal turn spawn, and the host renders only the Delta text after the last `ToolCallEnd`.

**Confirmed**, with changes noted. Two real prompts ran through the real app with the operator's own configured provider (z.ai GLM-5.3): the overlay produced OpenUI-Lang-only replies; the agent called a tool (`cronjob`, then `skills`); the text after the last `ToolCallEnd` parsed locally with 0 errors, and one reply rendered in the sandboxed frame with a working follow-up action (`[GENERATION-PROBE-1]` line 452, `[GENERATION-PROBE-2]` line 463, `[GENERATION-PARSE-AND-RENDER]` line 473).

- **Guardrails apply on this path.** In probe 1 the security guardrail blocked `cronjob` (`CriticalEvent kind=sidecar_unavailable`, `ToolCallEnd success=false`) and the model answered with a warning Callout, still valid OpenUI Lang.
- **Changed: filter the stream.** The same socket also carries `VoiceStatus`, `TurnStarted`, `TurnSeverity`, `CriticalEvent`, `ToolCallStart/End`, `Finished`, `TurnEnded`; the renderer must take only `Delta` after the last `ToolCallEnd`, and `Error` ends the turn.
- **Changed: socket handshake.** The WebSocket upgrade is rejected without an `Origin` header (fail-closed); a non-browser client must send one. The session cookie is `SameSite=Strict`.
- **Not exercised: approvals.** This home has `autonomous.yolo: true`, so no `ApprovalRequested` event appeared; the root approval-card wiring (Chat shows the card and badge) is untested and is a plan 10 acceptance item.
- **Overlay content.** The prompt advises image URLs (picsum.photos) and documents `@OpenUrl`, `Query()` and `Mutation()`; append overlay text telling the model to emit none of them (images are CSP-blocked, `@OpenUrl` is dropped by the bridge, `Query/Mutation` need a toolProvider we never supply).
- **Cost shape.** The overlay is one-shot per generation (53,352 chars). First Delta arrived 16-18 s after send in both probes (provider plus tool-call latency), which is the real time-to-first-render for a tool-using prompt; the render itself is 130 ms.
- **Spike scaffolding differences.** The spike read the prompt from `$OPENUI_SPIKE_PROMPT` and added `ChatRequest.openui: bool` with `#[serde(default)]`; production checks the prompt file in. Every other `ChatRequest` constructor needs the new field.

## Saved views

Default: one JSON file per view under the ACTIVE profile's home, `<profile home>/openui/views/<uuid>.json`, home resolved by `resolve_active_profile_for(ActivationSurface::Chat, None)` -> `default` sentinel = `get_hermes_home()`, any other slug = `profile_dir_for(slug)`; at most 200 views and 256 KiB per snapshot per profile.

**Changed: one correction to the resolution chain** (read from source; storage was not built in the spike). `resolve_active_profile_for` returns the persisted record's name or, when none covers the surface, `ironhermes_core::current_profile()`, which is derived from `IRONHERMES_HOME`. `profile_dir_for(name)` always joins `get_hermes_home()/profiles/<name>`. If the server process itself runs under a profile home (so `current_profile()` is that profile), `profile_dir_for(current_profile())` points at a nested, nonexistent `<profile home>/profiles/<profile>`. Build rule: use `get_hermes_home()` directly when the resolved slug equals `current_profile()` (or is the `default` sentinel), and `profile_dir_for(slug)` only for a slug that came from the persisted activation record. Both functions exist as `pub(crate)` in `iron_hermes_ui::server` (`profile_activation_api.rs`, `profile_api.rs`) and `resolve_active_profile_for` is currently `#[allow(dead_code)]` (no production caller). The caps (200 views, 256 KiB) and the file-per-view layout are unchanged by anything measured.

## Gateway

Recorded from the vendor docs read 2026-10-29 (`[DOCS-READ]`, line 296); no request was sent to Thesys and no conversation content left the machine.

- **Naming.** The hosted service is now branded "OpenUI Gateway" / "OpenUI Cloud"; the endpoint and key names are unchanged.
- **Endpoint:** `https://api.thesys.dev/v1/embed`, OpenAI-compatible (Chat Completions and Responses API). **Auth:** `Authorization: Bearer $THESYS_API_KEY` (key minted at console.thesys.dev/keys; browser frontend tokens are only for Conversations access, not used). **Stream format:** standard OpenAI SSE (`stream: true`). **Model naming:** `{provider}/{model}`, e.g. `openai/gpt-5`. **Billing:** no token markup; each response also consumes one plan API call (Free: 3,000 per month).
- **Retention / training: not stated** for API inputs and outputs. Facts that bear on it: generations through the Gateway "automatically appear in Reliability Monitoring" (so they are stored and visible in the vendor console); the pricing page says the free no-token-cost models may use request data for training; the Autofix page says the conversation "is not stored beyond the request logs"; the privacy policy (last updated 2024-07-29) says information is kept as long as required for the platform until the account is terminated, with no API-payload period. The UI disclosure must say content leaves the machine and retention is unknown.
- **Changed: the prompt question.** Thesys's own Gateway quickstart passes `generateSystemPrompt({ cloud: true, library })` as the system message, the managed prompt D-09 forbids as a default. Whether the Gateway accepts the locally generated `library.prompt()` text is not stated and was not tested. Build decision: send the local prompt first; if the Gateway's validation needs the managed block, that is a documented reason the opt-in path differs, to be confirmed with one real call once the operator supplies a key.
- **Default confirmed otherwise:** `ChatRequest { openui_gateway: true }` makes the server stream from the endpoint above with the vault key `THESYS_API_KEY`, emitting the same Delta/Finished events, no tools on that path, off by default.

## Decision

Decision: GO

- **Date:** 2026-10-29
- **Decided by:** operator (@bradwilson331), at the plan 07 checkpoint (`checkpoint:decision`, `gate="blocking-human"`, never auto-selected).
- **Operator reply (verbatim):** "go"
- **Reason:** (no reason stated; operator accepted the orchestrator's recommendation that all four D-11 blockers measured clean)
- **Plain GO, not GO-WITH-CHANGES.** The operator was told that plans 08-10 executors fold in the "changed" items from this ADR while building them, with no re-plan round: drop every action that is not `@ToAssistant` / `continue_conversation` (including `@OpenUrl`); register one host `message` listener; send an `Origin` header and filter the stream to `Delta` after the last `ToolCallEnd`; apply the corrected profile-home resolution rule for saved views.

## Consequences

If GO (plans 08-10 are written against the defaults above, with the "changed" items folded in):

- Vendored files (3.6 MB) plus a 53 KB prompt in git and on the scrubbed public showcase branch; MIT text and a NOTICE listing the MIT/ISC/BSD dependencies must ship next to them; the vendored SHA-256 values above are the pin. Open Question 4 recommendation: check the files in (offline, no network at build); the alternative is a build-time download verified against the same SHA-256.
- Upgrades of `@openuidev/*` are manual, re-run the SHA-256 and licence audit, and re-run `gen-system-prompt.mjs` so the prompt matches the bundled library.
- New surface: a raw route (mounted in both `main.rs` and the mirrored `test_router` in `login_page.rs`), a `ChatRequest` field, a prompt overlay, per-profile view storage, and a web-sys bridge with one listener.
- Recurring cost is prompt tokens (about 13k per generation by the chars/4 estimate) and provider latency, not wasm size.
- Known gaps a build must close or accept: Safari unverified; approval cards on the OpenUI socket unexercised; Form submit and oversize-message paths unexercised; light-theme frame inside a dark app.

If NO-GO: nothing above ships; the menu half stands alone (D-12).

### If go: next steps

Decision is GO, so plans 08-10 build the screen with the "changed" items above folded in as they go. The follow-on, once saved views (plan 10) ship, is the **OpenUI whiteboard canvas** (SEED-003, `.planning/seeds/SEED-003-openui-whiteboard-canvas.md`): generated widgets and notes placed on a per-profile board with geometry, `/`-to-place, AI-planned boards from a brief, and a present mode. A board is an ordered set of saved views plus geometry and notes, stored server-side per profile, and each widget is a real agent turn, so it can render live ironhermes data (sessions, kanban, spend, logs) and Re-run refreshes it. Reference (borrow the model and interactions, not the code; it is React/Next.js): https://github.com/jaibhasin/AI-Native-Dashboard (Apache-2.0), live demo https://ai-native-dashboard-sigma.vercel.app/. Not part of 54.9.

## If no-go

Text for the backlog entry plan 07 copies (next free number believed to be 999.6; confirm with the phase tool because ROADMAP heading order is unreliable):

> **Backlog 999.x: OpenUI generative-UI screen (deferred after ADR-0007).** Goal: a dedicated in-app screen where the operator types a request, the agent replies in OpenUI Lang using the operator's own providers, and the reply renders as it streams in a sandboxed iframe; per-profile saved views; component actions become a new agent turn.
> Evidence is complete and measured (see `docs/adr/ADR-0007-openui-generative-ui.md` and `.planning/phases/54.9-menu-configuration-and-openui-integration/spike/measurements.log`): licence clean (MIT, permissive transitive closure, NOTICE required); wasm +0.10%, frame asset ~0.75-0.93 MiB compressed; renders under the existing artifact CSP in `sandbox="allow-scripts"` with one non-fatal eval report; offline prompt (53,352 chars) and local parse/validate work; one real tool-using prompt through the app rendered end to end.
> Reason it was not built: operator decision at the plan 07 checkpoint (record the stated reason here).
> To resume: fold in the ADR's "changed" items (drop non-`@ToAssistant` actions; one host listener; stream filtering and the Origin header; overlay forbids images/`Query`/`Mutation`/`@OpenUrl`; profile-home resolution rule), re-verify Safari and the approval-card path, and re-audit the four `@openuidev/*` versions and their SHA-256 pins before any install (SUS verdicts, `lang-core` postinstall telemetry: always `--ignore-scripts` plus `OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1`). Spike branch `spike/54.9-openui` at `5a84adc39acf34801acfbd93b87d5ca5892ad629` is local only and may be gone; the ADR and log are the durable record.
