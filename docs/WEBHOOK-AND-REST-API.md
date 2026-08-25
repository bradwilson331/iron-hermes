# Webhook Adapter + REST API Server

IronHermes's gateway can also run two independent HTTP listeners alongside the chat
platforms in [`MULTI-PLATFORM-GATEWAY.md`](MULTI-PLATFORM-GATEWAY.md):

- **The webhook listener** (`gateway.platforms.webhook`) — a generic inbound HTTP
  callback receiver. A signed `POST` to a configured route path runs an agent turn and,
  by config, delivers the answer back to a URL, a platform, or the sender's own origin.
  This is what makes CPaaS providers (Twilio, Telnyx), automation tools (n8n), and CRMs
  (Twenty CRM) "reachable as config rather than code" — no new adapter code per service,
  just a route entry.
- **The REST API server** (`gateway.platforms.api_server`) — an OpenAI-compatible-shaped
  HTTP surface: chat completions, sessions, discovery endpoints, and scheduled-job
  management, on its own port.

Both are independent `PlatformAdapter`s spawned by `GatewayRunner::start()` alongside
Telegram/Discord/Slack — silent-skip applies here too (see
[`MULTI-PLATFORM-GATEWAY.md`](MULTI-PLATFORM-GATEWAY.md) for that contract), except that
both of these listeners additionally **fail closed at startup** rather than silently
skip: the REST API server refuses to construct without its key, and the webhook listener
refuses to construct when a no-verification route is combined with a non-loopback bind.
Neither surface defaults to open.

## What ships real, and what does not (read this before configuring either listener)

This section states the honest boundary of this phase's work, because the wire shapes,
auth, and routing described below are fully real and independently testable, but three
things behind them are not yet real model output:

- **`POST /v1/runs`, `POST /api/sessions/{id}/chat[/stream]`, and `POST
  /v1/chat/completions` all execute a deterministic, non-`AgentLoop` stub turn body.**
  Each has the correct wire shape (OpenAI-compatible request/response framing, SSE
  streaming, model-registry gating), but the "answer" is a word-by-word echo of the
  submitted prompt, not a model-produced response. A future plan wires all three onto a
  real `AgentLoop` invocation at once. Do not point a production client at any of these
  three routes expecting model output today.
- **`POST /v1/runs/{id}/approval` fails closed with `404` in production.** The REST API
  server's approval-gate handle is not yet threaded from `GatewayRunner::start()`, so
  there is currently nothing to resolve an approval against.
- **The webhook listener's `deliver: origin` target only works for `deliver_only`
  routes.** A non-`deliver_only` origin route (one that runs an agent turn and then
  tries to reply to the sender's own callback) is built and unit-tested, but the
  session's reply-routing metadata does not yet reach the send path end-to-end in
  production — it requires a change outside this crate. `deliver: url` and `deliver:
  platform` are unaffected, as is any `deliver_only: true` route regardless of target.
- **The webhook listener's deny-on-no-gate approval behavior is not yet wired into a
  live agent turn.** A webhook-originated turn still fails closed when no approval
  mechanism is available (the trait's own default), but not yet through the specific,
  more precisely worded denial path this phase built and unit-tested in isolation.

## REST API server

### Exposure posture — read this before binding anywhere but loopback

- **Loopback by default.** With no `host`/`port` configured, the REST API server binds
  `127.0.0.1:8642` — the same port the equivalent standalone port target used, so
  existing client configuration carries over unchanged.
- **Refuses to start without a key.** The adapter will not construct — the gateway skips
  this platform and logs why — unless `IRONHERMES_API_SERVER_KEY` is set to a non-empty
  value in the process environment. There is no default key and no "development mode"
  bypass.
- **A non-loopback bind needs BOTH the key AND an explicit opt-in.** Setting
  `gateway.platforms.api_server.host` to anything other than a loopback address is not,
  by itself, enough to expose the listener. The bind is refused unless
  `gateway.platforms.api_server.public_opt_in: true` is ALSO set. Both conditions,
  every time — there is no single flag that opens this surface to the network.
- **The bearer key is the WHOLE authorisation boundary.** There is exactly one
  authentication path on this surface (a constant-time bearer-token comparison against
  `IRONHERMES_API_SERVER_KEY`) — no cookie session, no per-request profile selector.
  Anyone holding the key can drive the agent through any mounted route, resolve
  approvals (once wired), and create, alter, pause, resume, or trigger any scheduled
  job. Treat the key exactly like a root credential to this gateway process.
- **Per-profile access is a second gateway, not a path prefix.** IronHermes profiles are
  process-scoped — the profile flag selects the home directory at process start. A
  per-request profile parameter on this surface would be a promise the runtime cannot
  keep. If you want profile-isolated REST access, run a second `hermes gateway run`
  process with a different profile, bound to a different port.

### Config skeleton

```yaml
gateway:
  platforms:
    api_server:
      enabled: true
      host: null # defaults to 127.0.0.1
      port: null # defaults to 8642
      public_opt_in: false # required, in addition to the key, for a non-loopback host
```

```bash
export IRONHERMES_API_SERVER_KEY=<a long random value — never checked into config.yaml>
```

### Mounted surface

`GET /v1/capabilities` (bearer-gated, like every route below) is the live source of
truth for what is mounted — it is built directly from the router's own family
registration, so it cannot describe a route that does not exist. As of this phase:

| Family | Routes |
|---|---|
| `health` | `GET /health`, `/health/detailed`, `/v1/health` |
| `models` | `GET /v1/models`, `/api/model/options` |
| `meta` | `GET /v1/skills`, `/v1/toolsets`, `/v1/capabilities` |
| `runs` | `POST /v1/runs`; `GET /v1/runs/{run_id}`, `/{run_id}/events` (SSE); `POST /{run_id}/approval`, `/{run_id}/stop` |
| `sessions` | `GET/POST /api/sessions`; `GET/PATCH/DELETE /{id}`; `GET /{id}/messages`; `POST /{id}/fork`; `PATCH /{id}/model`; `POST /{id}/chat`, `/{id}/chat/stream` (SSE) |
| `chat` | `POST /v1/chat/completions` (streaming and non-streaming) |
| `responses` | `POST /v1/responses`; `GET/DELETE /v1/responses/{id}` — mounted (`401`, not `404`, when unauthenticated), all three answer `501 Not Implemented` naming `chat/completions` and the session-chat routes as the real alternatives. No backing store exists for server-side conversation state in this repository. |
| `jobs` | `GET/POST /api/jobs`; `GET/PATCH/DELETE /{id}`; `POST /{id}/pause`, `/{id}/resume`, `/{id}/run` |

`/v1/capabilities`'s response also carries an `omitted_endpoints` array naming the two
endpoints the equivalent standalone port target exposes that this deployment
deliberately does not carry, each with a reason — so the parity gap is stated, not
inferred from a missing route:

1. **A generic per-platform HTTP callback ingress** — no in-scope consumer, because
   every platform that would call it is deferred by this phase's own scope cut.
2. **A managed-cron fire webhook** — belongs to an external scheduling service specific
   to the port target, with no counterpart in this deployment (scheduling here is the
   `jobs` family above, over the gateway's own cron loop).

### Scheduled jobs (`/api/jobs`)

Every route in this family reads and writes the SAME `JobStore` the gateway's own cron
tick loop already reads (`ironhermes-cron`) — a job created here is picked up by the
scheduler that is already running in this process, not a REST-only shadow list.

- `POST /api/jobs` creates a job: `name`, `prompt`, a `schedule` string (the same syntax
  the CLI and the web UI's Schedules editor accept — `"every 2h"`, a 5-field cron
  expression, or an RFC3339 timestamp), and optional `deliver` (defaults to `"local"`)
  and `skills`. The schedule is validated before anything is persisted — an unparseable
  schedule is refused with a client error naming the problem, never silently stored as a
  job the scheduler will skip forever.
- `GET /api/jobs/{id}` resolves by id OR by name (case-insensitive) — the same
  resolution the store's own lookup uses, so a client that lists jobs and sees names can
  address a job by name.
- `PATCH /api/jobs/{id}` writes only the fields present in the request body; an omitted
  field keeps its stored value.
- `POST /api/jobs/{id}/pause` and `/resume` both call the store's own enabled toggle —
  pausing is not a second, REST-only flag; the running scheduler observes the change
  because it is the same field the toggle already writes. Both are idempotent.
- `POST /api/jobs/{id}/run` triggers the job through the store's own manual-trigger
  path, exactly like a trigger issued from the CLI or the web UI.
- Every per-job verb returns `404` for an identifier that does not resolve, and none
  creates a job implicitly.

## Webhook listener

### Exposure posture

- **No default host/port carries over from a config-less state the way the REST server
  has one** — configure `gateway.platforms.webhook.host`/`.port` explicitly for your
  deployment.
- **A no-verification route refuses construction on a non-loopback host.** If any
  configured route selects `signature: none`, the whole adapter refuses to start unless
  the configured bind host is loopback. There is no operator override — either remove
  the no-verification route or bind to loopback.
- **Key material is named by environment variable, never written into config.yaml.**
  `secret_env`/`auth_token_env`/`public_key_env` on a route, and the `env`/`user_env`/
  `pass_env` fields under `outbound_auth`, each name an environment variable the
  adapter reads at construction or delivery time — the credential value itself never
  appears in the config file.

### Config skeleton

```yaml
gateway:
  platforms:
    webhook:
      enabled: true
      host: 0.0.0.0
      port: 8643
      public_opt_in: true
      external_base_url: null # set when behind a reverse proxy; required for Twilio
      routes:
        - name: my-route
          path: /webhook/my-route
          signature: generic_v2
          secret_env: MY_ROUTE_SECRET
          prompt_template: "New message: {body}"
          deliver: url
          deliver_url: https://example.com/callback
```

### Route-config fields and defaults

Every field on a route carries a default — a bare `{}` route entry deserializes
successfully.

| Field | Default | Notes |
|---|---|---|
| `name` | `""` | Route identifier; used as the `chat_id`/session-derivation key. |
| `path` | `""` | The HTTP path this route answers on, e.g. `/webhook/my-route`. |
| `signature` | `generic_v2` | One of the four selectors below. |
| `secret_env` | `None` | Env var holding the HMAC secret, for `generic_v2`. |
| `auth_token_env` | `None` | Env var holding the Twilio auth token, for `twilio`. |
| `public_key_env` | `None` | Env var holding the Telnyx Ed25519 public key, for `telnyx`. |
| `timestamp_skew_secs` | `300` | Allowed clock skew for `generic_v2`'s timestamp binding. |
| `prompt_template` | `""` | Brace-delimited placeholder template rendered against the inbound payload. |
| `deliver` | `url` | One of `origin` / `platform` / `url` (see Delivery targets below). |
| `deliver_url` | `None` | Target URL, when `deliver: url`. |
| `deliver_platform` | `None` | Target platform name (e.g. `"telegram"`, `"buzz"`), when `deliver: platform`. |
| `deliver_chat_id` | `None` | Target chat id on `deliver_platform`; falls back to this route's own `name`. |
| `deliver_only` | `false` | When `true`, this route only renders and delivers — it never runs an agent turn. |
| `outbound_auth` | `none` | `none` / `bearer { env }` / `basic { user_env, pass_env }` — attached to `deliver: url` requests. |
| `session` | `ephemeral` | `ephemeral` (a fresh session per delivery) or `persistent` (one session reused across every delivery on this route). |
| `rails.max_body_bytes` | `1048576` (1 MiB) | Enforced as a router layer BEFORE the body is read. |
| `rails.rate_limit_per_minute` | `30` | Fixed-window, per route. |
| `rails.idempotency_ttl_secs` | `3600` | How long a delivery's claim-check entry survives, so a sender's retry after a timeout runs the turn exactly once. |

### Signature selectors

Four schemes, selected per route via `signature`. This set is closed for this phase —
there is no legacy body-only unversioned fallback, and none will be added:

| Selector | Header(s) | Signed content |
|---|---|---|
| `generic_v2` (default) | `X-Webhook-Signature-V2` + `X-Webhook-Timestamp` (both REQUIRED) | HMAC-SHA256 over `{timestamp}.{raw_body}`, keyed by `secret_env`. A present-but-invalid `X-Webhook-Signature-V2` is refused outright — there is no fallback to an unsigned or legacy path. |
| `none` | — | No verification at all. Refused at construction time on a non-loopback host (see above) — there is no per-request override. |
| `twilio` | `X-Twilio-Signature` | HMAC-SHA1 over the full request URL plus the sorted, concatenated form parameters, base64-encoded, keyed by `auth_token_env`. Only meaningful for `application/x-www-form-urlencoded` bodies; a JSON body under this scheme is refused, not silently HMAC'd over raw bytes. Behind a reverse proxy, set `external_base_url` — Twilio signs the externally visible URL byte for byte. |
| `telnyx` | `telnyx-signature-ed25519` (+ a timestamp header) | Ed25519 signature verified against `public_key_env`. |

### Delivery targets

`deliver` selects where a route's rendered answer goes:

- **`origin`** — deliver back to the sender's own callback, as named in the payload.
  Only reliable today for `deliver_only: true` routes (see the limitations section
  above); a non-`deliver_only` origin route is built and tested but not yet reachable
  end-to-end in production.
- **`platform`** — deliver via a named platform adapter already configured on this same
  gateway (e.g. `telegram`, `buzz`), resolved through the shared delivery registry.
- **`url`** (default) — deliver via an arbitrary URL. **SSRF-checked twice**: once when
  the route config loads, and again immediately before every delivery POST — the second
  check is the documented mitigation for the first check's own DNS-rebinding
  time-of-check/time-of-use gap. **The delivery client does not follow redirects.** A
  `deliver: url` target that answers with a `3xx` will NOT be followed — by design, not
  oversight. If your target endpoint redirects, point `deliver_url` at the final
  destination directly.

### Acknowledge-then-run, and the model-free fast path

A signed, accepted `POST` to a route returns `202 Accepted` IMMEDIATELY — the agent turn
(when one runs at all) executes on a background task after the response has already
gone out. This exists because senders typically time out around ten seconds; waiting for
a full agent turn before acknowledging would cause senders to retry deliveries that were
already in flight, which is exactly what the idempotency rail above exists to absorb
cheaply when it happens anyway.

A `deliver_only: true` route skips the model entirely — it renders `prompt_template`
against the payload and delivers the rendered text directly, with no agent turn at all.
This is the "model-free fast path": the route still runs the full verify → render →
deliver pipeline and every rail, it just never invokes the agent to produce the content
it delivers.

### Rails

Every route carries three defensive limits (`rails` above), enforced in this order:

1. **Body size cap** (`max_body_bytes`) — enforced as a router layer before the body is
   even read, so an oversized payload never reaches signature verification.
2. **Rate limit** (`rate_limit_per_minute`) — a fixed window, scoped per route.
3. **Idempotency** (`idempotency_ttl_secs`) — a self-pruning cache keyed by a
   sender-supplied `X-Webhook-Idempotency-Key` header (falling back to a derived digest
   when absent), so a retried delivery inside the TTL window runs the agent turn exactly
   once.

## Worked examples

These three are the named integration stories this phase targets — "reachable as config
rather than code" is the actual claim, and it is only verifiable by seeing a real round
trip, not by reading the field table above.

### CPaaS inbound message (Twilio-shaped)

```yaml
routes:
  - name: sms-inbound
    path: /webhook/sms-inbound
    signature: twilio
    auth_token_env: TWILIO_AUTH_TOKEN
    prompt_template: "SMS from {From}: {Body}"
    deliver: platform
    deliver_platform: telegram
    deliver_chat_id: "123456789"
```

Twilio POSTs an inbound SMS as `application/x-www-form-urlencoded` with `From`/`Body`
fields and an `X-Twilio-Signature` header. The route verifies the signature over the
URL plus the sorted form parameters, renders the prompt from the form fields, runs a
turn, and relivers the answer to the named Telegram chat.

### Automation-tool round trip (n8n-shaped, `generic_v2`)

```yaml
routes:
  - name: n8n-trigger
    path: /webhook/n8n-trigger
    signature: generic_v2
    secret_env: N8N_WEBHOOK_SECRET
    prompt_template: "Automation event: {event_type} — {summary}"
    deliver: url
    deliver_url: https://n8n.example.com/webhook/callback
    outbound_auth:
      kind: bearer
      env: N8N_CALLBACK_TOKEN
```

n8n (or any generic automation tool) signs its POST with an HMAC-SHA256 over
`{timestamp}.{raw_body}`, sent as `X-Webhook-Signature-V2` + `X-Webhook-Timestamp`. The
route renders a prompt from the event JSON, runs a turn, and posts the answer back to
n8n's own callback URL with a bearer token attached.

### CRM round trip (Twenty CRM-shaped)

```yaml
routes:
  - name: crm-update
    path: /webhook/crm-update
    signature: generic_v2
    secret_env: TWENTY_CRM_WEBHOOK_SECRET
    prompt_template: "CRM record updated: {record_name} ({record_id})"
    deliver_only: true
    deliver: url
    deliver_url: https://twentycrm.example.com/api/notes
    outbound_auth:
      kind: basic
      user_env: TWENTY_CRM_API_USER
      pass_env: TWENTY_CRM_API_PASSWORD
```

A `deliver_only` route, so no agent turn runs at all — the rendered note text is posted
directly to the CRM's own API using HTTP Basic auth, on every signed webhook delivery.

### What is only verifiable against a live third-party account

The automated suite proves the wire-level contract against synthetic requests: each
signature scheme's construction, the SSRF checks, redirect-refusal, rail ordering,
delivery-target dispatch, and the acknowledge-then-run timing. It does **not** exercise:

- A real Twilio/Telnyx account's actual signed payload shape and header casing as sent
  by their production infrastructure (as opposed to this suite's own hand-constructed
  signed fixtures).
- A real CRM or automation tool's authentication flow against `outbound_auth` (token
  validity, expiry, and provider-side error responses for a wrong or expired credential).
- Delivery-endpoint availability and latency under real network conditions — the SSRF
  checks and redirect-refusal are proven against local test servers, not the public
  internet.

Operators integrating a specific third-party service should confirm the first signed
request from that provider's real infrastructure lands as expected before relying on
the route in production.
