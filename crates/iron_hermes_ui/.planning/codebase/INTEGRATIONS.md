# External Integrations

**Analysis Date:** 2026-05-02

## APIs & External Services

None detected. The current codebase contains no API clients, HTTP request code, or SDK imports. The `src/main.rs` renders a static hero component with hard-coded external links only.

**External URLs referenced in UI (not API calls):**
- `https://dioxuslabs.com/learn/0.7/` — documentation link (anchor tag)
- `https://dioxuslabs.com/awesome` — resource link (anchor tag)
- `https://github.com/dioxus-community/` — community link (anchor tag)
- `https://github.com/DioxusLabs/sdk` — SDK link (anchor tag)
- `https://marketplace.visualstudio.com/items?itemName=DioxusLabs.dioxus` — VSCode extension link (anchor tag)
- `https://discord.gg/XgGxMSkvUM` — Discord community link (anchor tag)

## Data Storage

**Databases:** None detected

**File Storage:** Local filesystem assets only (`assets/` directory)

**Caching:** None detected

## Authentication & Identity

**Auth Provider:** None detected — no authentication layer present

## Monitoring & Observability

**Error Tracking:** None detected

**Logs:** None — no logging framework configured

## CI/CD & Deployment

**Hosting:** Not configured — no deployment manifests, Dockerfiles, or CI configs present

**CI Pipeline:** None detected

## Environment Configuration

**Required env vars:** None — no environment variables read by the application

**Secrets:** None — no secrets management in use

## Webhooks & Callbacks

**Incoming:** None

**Outgoing:** None

## Async / Network Capability (Dioxus)

The Dioxus framework (`use_resource`, server functions via `#[post]`/`#[get]` macros) supports async network requests and fullstack server functions, but none are implemented in the current codebase. The `AGENTS.md` documents these patterns as available for future use:

- `use_resource` — async data fetching hook
- `#[post]` / `#[get]` macros — server function definitions (requires `dioxus/fullstack` or `dioxus/server` feature)
- `use_server_future` — SSR-safe async data fetching with hydration

To add network capability, enable the `fullstack` or `server` feature in `Cargo.toml`:
```toml
dioxus = { version = "0.7.1", features = ["fullstack"] }
```

---

*Integration audit: 2026-05-02*
