# Tier 2 semantic memory

Operator guide for the Tier 2 memory subsystem (Phase 58.2, [ADR-0008](adr/ADR-0008-muse-memory-integration.md)).
Configuration keys are listed in [CONFIGURATION.md](CONFIGURATION.md#memory-memory).

## Overview

IronHermes memory has two tiers:

- **Tier 1** is the existing in-prompt memory: `MEMORY.md` (2,200 chars) and `USER.md` (1,375 chars), managed by the
  `memory` tool and any of the `file`, `sqlite`, `duckdb` or `grafeo` providers. Tier 1 stays authoritative and the `memory`
  tool behaves as before. When an entry leaves Tier 1 (removed, or the old text of a replace or condense), Tier 2 moves it into
  today's daily notes, `live/YYYY-MM-DD.md`, and keeps it searchable under the same claim id.
- **Tier 2** is a searchable store of short, cited *claims* kept per profile under `<home>/tier2/`. It indexes every Tier 1
  entry, keeps the entries that leave Tier 1, holds the agent's daily notes, and (when enabled) also extracts claims from your conversations, writes human-readable summary files, runs a
  nightly "dreams" review, and lets claims be hidden ("forgotten").

Tier 2 is a standalone subsystem beside Tier 1. It is not a memory provider and does not use the `mirror_provider` slot, so it
works with every Tier 1 provider.

## Turning it on and rolling back

All five flags default to `false`. With every flag off, IronHermes behaves exactly as it did before Phase 58.2 and no
`tier2/` directory is created.

| Flag | What it enables | Needs |
|---|---|---|
| `memory.tier2_enabled` | The Tier 2 store, Tier 1 indexing, `memory_search`, `memory_explain`, the Memory > Tier 2 tab | `memory.memory_enabled: true` (the default) |
| `memory.consolidation_enabled` | Hourly claim extraction from conversations, `memory_consolidate`, the derived `bank/`, `people/`, `groups/` files | `tier2_enabled` |
| `memory.dreams_enabled` | Nightly dreams, and the standing-guidance block injected at session start | `tier2_enabled` (dreams work from consolidated claims, so also `consolidation_enabled` in practice) |
| `memory.forgetting_enabled` | `memory_forget` and the UI Forget / Undo buttons (stage and cancel only, see [Forgetting](#forgetting)) | `tier2_enabled` |
| `memory.promotion_enabled` | The gateway's hourly consolidation writes durable user facts into `USER.md` / `MEMORY.md` (see [Promotion into Tier 1](#promotion-into-tier-1)) | `tier2_enabled` and `consolidation_enabled` |

```yaml
memory:
  memory_enabled: true
  tier2_enabled: true
  consolidation_enabled: true
  dreams_enabled: false
  forgetting_enabled: true
```

The flags are read when the process starts; restart the CLI, web server or gateway after changing them.

**Rollback.** Set the flags back to `false` and restart. Tier 2 stops reading and writing, its tools disappear, and no
synthesis block is injected. Your data under `<home>/tier2/` is kept untouched, so turning the flags back on resumes where you
left off. A Tier 2 setup error (for example a bad `memory.embeddings` block) disables only Tier 2 and is logged; Tier 1 keeps
working.

Known limit: with `memory.provider: sqlite`, edits made through the `memory` tool *during a session* are not indexed into
Tier 2 until the next process start (that provider serves a frozen snapshot of its entries). With the `file` provider they are
indexed immediately. With `sqlite`, an entry removed during a session moves at the next process start, and an entry added
and removed within one session never reaches Tier 2.

## On-disk layout

Everything lives under the profile home (`$IRONHERMES_HOME`, default `~/.ironhermes`; named profiles have their own), never
in `memory.db`, which the `memory-sqlite` provider owns:

```
<home>/tier2/
  tier2.db              claims, evidence, embeddings, FTS5 index (SQLite)
  index.usearch         vector index; a derived cache, rebuilt from tier2.db if missing or stale
  live/YYYY-MM-DD.md    append-only daily notes: claims extracted from conversations, entries moved out of Tier 1, and
                        the agent's `memory_note` notes, cited as memory://live/YYYY-MM-DD.md#L<n>
  live/.append.lock     cross-process lock for appends; leave it in place
  bank/                 world.md, experience.md, opinions.md, reflections.md (derived)
  people/<slug>.md      derived, one file per person
  groups/<slug>.md      derived, one file per group
  dreams/               YYYY-MM-DD.md, ALIGNMENT_SYNTHESIS.md, ALIGNMENT_STATE.yaml,
                        REPAIR_THREADS.yaml, PROGRESSION_HISTORY.yaml
  forget/pending.json   claims staged for forgetting
  models/               built-in embedding model cache
```

Claim ids look like `claim:0123456789ab`. Tier 1 claims cite `memory://tier1/MEMORY.md#L<n>` or `memory://tier1/USER.md#L<n>`
while the entry is in Tier 1, and `memory://live/YYYY-MM-DD.md#L<n>` once it has moved out. `memory_explain` still lists the
original Tier 1 line.
The `bank/`, `people/` and `groups/` files are regenerated from the database after each consolidation run; edits made to them
by hand are overwritten. Directories are created with mode 0700.

## Tools

| Tool | Scope | What it does |
|---|---|---|
| `memory_search` | read-only | `query` (max 500 chars), `limit` (1-50). Returns claim ids, text, source, confidence, and the search `mode`. |
| `memory_explain` | read-only | `claim_id`. Returns the cited evidence and any claim it supersedes. Superseded or staged claims read as not found. |
| `memory_note` | full | `content` (max 1,000 chars). Appends one note to today's daily notes and indexes it at once. It is screened by the MEM-05 scanner, refuses forgotten text, and is never shown in the prompt. Notes are not durable, so they stay out of `bank/` and the dreams synthesis. |
| `memory_consolidate` | full | Runs a consolidation pass now. Needs `memory.consolidation_enabled`. |
| `memory_forget` | full | `claim_id`, `action` (`stage` or `cancel`), optional `reason`. Needs `memory.forgetting_enabled`. |

"Read-only" tools are also available inside `execute_code` sandboxes and delegated child agents. "Full" tools are only
registered for the main agent, so a sandboxed script or a child agent can never trigger consolidation or stage a forget. The
tools belong to the `memory` toolset and are registered on every surface that registers it: CLI/TUI, web, gateway platforms,
cron and kanban workers.

## When Tier 1 fills up

With Tier 2 on, removing or condensing entries in `MEMORY.md` or `USER.md` is safe: the text moves to the daily notes and
`memory_search` finds it.

- Re-adding the same text, in either file, re-links the same claim rather than duplicating it.
- A claim staged for forgetting is not moved or copied while staged; it moves after a cancel.
- With `memory.tier2_enabled` on, every new session's Memory section ends with a `## Long-term memory` block, and the `memory`
  tool description says the same, pointing the agent at `memory_note` for day-to-day detail and `memory_search` for recall.
  With Tier 2 off neither text appears.

## Embeddings

`memory_search` ranks by meaning when an embedder is available and otherwise falls back to keyword (FTS5) search. The result
always says which one it used: `mode` is `semantic` or `keyword`, and a keyword result carries a `note` such as
`semantic search unavailable (embedding model is downloading/initialising); showing keyword matches`. Claims written while no
embedder is available are stored anyway and embedded in the background once one is.

### Built-in model (default)

The default build embeds in-process with `fastembed` and `all-MiniLM-L6-v2` (384 dimensions). The model is **not** shipped in
the binary or the image. On first use it downloads from `huggingface.co` into `<home>/tier2/models` (in the container,
`/opt/data/tier2/models`). The `HF_HOME` environment variable, if set, takes precedence over that directory. Until the download
finishes, searches run in keyword mode; a failed download is retried after about ten minutes. The machine therefore needs
outbound HTTPS to `huggingface.co` the first time.

Other built-in models can be chosen with `memory.embeddings.model`, but only 384-dimension ones are accepted:
`all-MiniLM-L6-v2`, `all-MiniLM-L12-v2`, `bge-small-en-v1.5`, `paraphrase-multilingual-MiniLM-L12-v2`,
`multilingual-e5-small`. Any other name is rejected and disables Tier 2.

Every stored vector records the model that made it, and search only compares vectors from the active model. Changing the model
therefore re-embeds every claim in the background (and rebuilds the index); searches use keyword mode until that is done.

### OpenAI-compatible endpoint (optional)

Set `memory.embeddings.base_url` to use an HTTP endpoint instead of the built-in model. IronHermes posts to
`<base_url>/v1/embeddings` (a base URL already ending in `/v1` is not doubled) with the configured `model`, and requires 384
numbers back per input. Redirects are not followed.

```yaml
memory:
  embeddings:
    base_url: http://127.0.0.1:8081
    model: all-minilm-l6-v2
    api_key_env: EMBEDDINGS_API_KEY   # optional; name of an environment variable
```

- `api_key_env` holds the **name** of an environment variable, never the key. It is read from the process environment only
  (not from the vault) and sent as a Bearer token when the variable is set and non-empty. A loopback sidecar needs none.
- **Privacy:** a configured endpoint receives every claim text and every search query. Point it only at a service you trust.
  A remote HTTPS endpoint means your memory text leaves the machine.
- If the endpoint is down, search degrades to keyword mode and writes still succeed.

## Optional sidecar: wtf_embedding_service

`wtf_embedding_service` is a separate repository (`~/code/wtf_embedding_service`), documented here as an optional embedding
server and as the reference for the in-process embedder. Nothing from it is vendored; you do not need it, because the built-in
model covers the same use.

**Run it on port 8081, bound to loopback.** Its shipped `config.toml` binds every network interface with no authentication and
uses port 8080, which belongs to the IronHermes web UI. Used as shipped it would both collide with the UI and expose an
unauthenticated service. Change its `config.toml` before starting it:

```toml
[server]
host = "127.0.0.1"
port = 8081

[[models]]
name = "all-minilm-l6-v2"
fastembed_model = "AllMiniLML6V2"
execution = "cpu"
default = true
```

Then point IronHermes at it:

```yaml
memory:
  embeddings:
    base_url: http://127.0.0.1:8081
    model: all-minilm-l6-v2
```

Check with `curl http://127.0.0.1:8081/health`. Keep the model at 384 dimensions (the sidecar's other bundled model,
`bge-small-en-v1.5`, also qualifies). Do not expose this service on a non-loopback address: it has no authentication, and
anyone who can reach it can read every claim and query that IronHermes sends.

## Consolidation

With `memory.consolidation_enabled`, a loop hosted by the **gateway** process runs once an hour and extracts claims from new
conversation turns. The CLI alone does not run schedulers, so consolidation only happens while `ironhermes gateway` is
running; you can also trigger a pass from any surface with the `memory_consolidate` tool. The gateway serves its own profile
home only.

- **Input:** only `user` and `assistant` messages from `state.db`, since a persisted watermark. Tool results and fetched web
  content are never sent to the model. The first time you enable it, it looks back 24 hours. Per run it takes at most 5 batches
  of 200 messages, each message cut to 2,000 characters.
- **Model:** the `model.roles.memory` role, resolved through the usual role cascade (`roles.memory`, then the `auxiliary` block
  if you have one, then the main model). Set it to a cheap model.
- **Output:** at most 50 claims per batch, each at most 300 characters, with a kind (`world`, `experience`, `opinion`,
  `reflection`), a subject, and a citation to the live log. Confidence is 0.95 for a direct user statement and 0.7 otherwise.
- **Supersession:** a new claim replaces an older active claim of the same subject and kind when their embedding similarity is
  above 0.85. This detects near-duplicates, not contradictions. Without an embedder, supersession is skipped and the report
  says so.
- **Safety:** every claim and every evidence quote is screened by the same injection/exfiltration scanner as Tier 1 writes.
  Rejected claims are not stored; an entry with `kind` `memory_claim_rejected` (carrying only a hash and the verdict) is
  appended to `<home>/logs/audit.jsonl`. An unparseable model reply is retried once, then audited as
  `memory_consolidation_parse_failed` and skipped.
- **Overlap safety:** a lease prevents two runs at once, and with nothing new to read a run makes no model call.

### Promotion into Tier 1

With `memory.promotion_enabled` (default off, needs `tier2_enabled` and `consolidation_enabled`), the gateway's hourly job
also moves durable user facts into the live memory files after each consolidation attempt.

- **Candidates:** active, durable, direct (confidence 0.95) `world` or `opinion` claims about the user that are not already
  in Tier 1, not staged and not forgotten. Daily notes never promote.
- **Cap:** at most 3 facts per hourly run.
- **Target order:** `USER.md` first, then `MEMORY.md` (also when `memory.user_profile_enabled` is false). A file takes a fact
  only while it stays at or below 80% of its char limit after the write, so promotion never fills Tier 1. With no room in
  either, the fact waits and is offered again on a later run.
- **Safety:** writes go through the same path as the `memory` tool: the prompt-injection scanner, duplicate check, char limit
  and file lock (safe alongside the web UI). A fact Tier 1 refuses is never offered again.
- **One result:** the promoted line links to the same claim, so `memory_search` shows one result citing `memory://tier1/...`.
  If you remove a promoted line it moves to the daily notes like any Tier 1 removal and is not promoted again.
- **Audit:** `memory_promoted`, `memory_promotion_deferred` and `memory_promotion_rejected` entries in
  `<home>/logs/audit.jsonl` carry the claim id and target, never the text.
- **Where it runs:** only in the gateway's hourly job. `memory_consolidate` called by hand does not promote.

## Dreams and standing guidance

With `memory.dreams_enabled`, the gateway runs a nightly job at **02:00 local time** (`agent.timezone` if set, otherwise the
host's zone; daylight-saving changes are handled). One model call (`model.roles.memory`) reviews the last day's claims plus up
to 200 durable ones and writes, under `<home>/tier2/dreams/`:

- `YYYY-MM-DD.md`: prose review, with `prompt_hoisted: false` in its front matter (this prose is never injected into prompts)
- `ALIGNMENT_SYNTHESIS.md`: standing guidance (preferences, boundaries, friction), regenerated each night
- `ALIGNMENT_STATE.yaml`, `REPAIR_THREADS.yaml`, `PROGRESSION_HISTORY.yaml`: companion state files

Every item must cite claims that exist; uncited items are dropped, and dropped items are audited as `memory_dream_rejected`
(hash only). A failed parse is audited as `memory_dream_parse_failed`.

At the start of each new session, the synthesis is added to the prompt's Memory section as a block titled
`Standing guidance from long-term memory`, labelled as non-authoritative, scanned again, and capped at **1,500 characters**.
The block is frozen for the life of the session: tonight's update only reaches sessions that start afterwards, so the prompt
cache is never disturbed mid-session.

## Forgetting

Forgetting has four steps: stage, hidden immediately, cancel, and purge. Only the last one deletes anything, and only a
human can run it.

1. **Stage.** `memory_forget` with `action: stage` (or **Forget** on the Memory > Tier 2 tab) marks the claim as staged and
   records it in `<home>/tier2/forget/pending.json`.
2. **Hidden immediately.** A staged claim disappears from `memory_search` (semantic and keyword), `memory_explain`,
   the Tier 2 claim list and the input to dreams. The bank/people/groups files are re-rendered at once without it, and
   any `ALIGNMENT_SYNTHESIS.md` line citing it is left out of the standing-guidance block for **new** sessions (a session
   that already started keeps the block it began with). Consolidation and the Tier 1 sync will not re-learn the exact
   same text while it is staged; a reworded version of the fact can still be extracted as a new claim.
3. **Cancel.** `memory_forget` with `action: cancel`, or **Undo** under "Staged to forget" on the Tier 2 tab, restores the
   claim exactly as it was. Until a purge runs, you can cancel at any time.
4. **Purge.** A human permanently deletes the staged claims (below).

`memory.forgetting_enabled` gates staging, cancelling and purging. Staging and cancelling are audited as
`memory_forget_staged` and `memory_forget_unstaged` (claim id only, never the text or your reason).

### Purging staged claims

The purge is **human-run only**. No agent tool can purge, and the tool-approval gate is not involved. Two entry points:

- **CLI:** `ironhermes memory purge`. It always shows a dry run first; `--dry-run` stops there. To delete, type `purge` at
  the prompt. It refuses without an interactive terminal.
- **Web UI:** **Purge now…** in the "Staged to forget" panel on the Memory > Tier 2 tab. It shows the preview, then needs
  you to type `purge`. It needs `security.web_config_write_enabled`; with web writes off the button is disabled and its
  tooltip says why. It also needs web login (`web_ui.auth`): with login off, any local process, an agent's `terminal`
  included, could replay the request, so the web purge refuses and you use the CLI. The preview issues a one-time
  ticket, valid for 5 minutes, for your login session and exactly the claims it showed; if it expires, open the
  preview again.

A purge deletes only what the preview showed. The preview shows counts, the exact `MEMORY.md` / `USER.md` entries and a fixed
list of the derived files; it does not list each daily-note line. A claim staged after you looked at the preview is not
purged (the result says it was kept).

What a purge removes:

- the claim and the older wordings it superseded, with their evidence, vectors and FTS entries;
- the exact `MEMORY.md` / `USER.md` entry for the claim;
- its daily-note lines (each is replaced by `<!-- retracted -->`);
- its lines in the bank/people/groups files (re-rendered) and in dreams and standing guidance (`ALIGNMENT_SYNTHESIS.md` and
  the YAML state files);
- its staging record.

It also rebuilds the vector index, keeps only a text hash and the source message ids so consolidation, the Tier 1 sync and
`memory_note` cannot learn the same text again, and redacts other claims' evidence quotes from the same messages to
`[retracted]`. Each purge is audited as `memory_forget_purged` (claim id, text hash and `cli` or `ui`, never the text).

If consolidation or dreams is running, the purge says to try again in a minute and deletes nothing.

**An incomplete scrub.** If other processes hold the database, the on-disk cleanup (VACUUM and WAL checkpoint) can be
incomplete, and the result says so. The same holds if a daily-note, dreams or YAML rewrite fails after the delete. The
claims are already gone; the rest of the scrub is not. The ids to scrub are written to `tier2/forget/scrub_pending` before
the delete, so the next run still retracts their lines. Stop the gateway and the web UI,
then run `ironhermes memory purge` again: with nothing staged it finishes the unfinished scrub. **Purge now** cannot do this
step: it is disabled when nothing is staged, and the web UI is one of the processes you have to stop.

**What a purge cannot reach.** Conversation transcripts in `state.db` (and so `session_search`), backups or snapshots of
`<home>`, and data left in the filesystem's free blocks. Do not treat Tier 2 forgetting as a privacy-erasure guarantee.
Never approve an agent `terminal` call that runs `memory purge`: the typed word is the only consent, and it must come from you.

## Build and deployment notes

- **C++17 compiler.** The vector index (`usearch`) is C++17 compiled through `cxx`, so building from source needs a working
  `g++`/`clang++`. The official builder image (`rust:1.96-trixie`) has one. The runtime image installs `libstdc++6`.
- **Network at build time.** The built-in embedder links a static ONNX Runtime that `ort` downloads from `cdn.pyke.io` during
  the build (verified against a pinned SHA-256). Offline or egress-restricted builds fail unless you disable the feature.
- **Compile it out.** `cargo build -p ironhermes-cli --no-default-features` drops the built-in embedder (and `fastembed`/ONNX
  Runtime) entirely; Tier 2 then runs in keyword mode unless `memory.embeddings.base_url` is set.
- **Intel macOS** (`x86_64-apple-darwin`) has no prebuilt ONNX Runtime from `ort`, so the default build does not work there;
  use `--no-default-features` and an endpoint, or build on Apple silicon or Linux.
- **Containers.** The image sets `IRONHERMES_HOME=/opt/data`, runs with a read-only root and a writable `/opt/data` volume,
  so all Tier 2 data and the model cache live under `/opt/data/tier2` and persist with the volume. The first-use model download
  needs outbound HTTPS from the container.
- **Where it runs.** Hourly and nightly jobs run in `ironhermes gateway`, not in the CLI or TUI.

## Troubleshooting

- **Every search says `mode: keyword`.** Read the `note`. `embedding model is downloading/initialising` means the first-use
  download is still running (or has not started; it begins when Tier 2 opens). `embedding model failed to load: ...` means the
  download or load failed: check outbound access to `huggingface.co`, free disk under `<home>/tier2/models`, and the logs; it
  retries about every ten minutes. `built-in embedder compiled out` means the binary was built with `--no-default-features`.
  A note naming an endpoint error means `memory.embeddings.base_url` is unreachable; check it with `curl`.
- **`memory_forget` / `memory_consolidate` refuse with a message naming a flag.** Enable `memory.forgetting_enabled` or
  `memory.consolidation_enabled` and restart.
- **Nothing gets consolidated.** Consolidation runs only inside the gateway, and only for the gateway's own profile home.
  Call `memory_consolidate` to run one pass and read its report (`claims_added`, `llm_calls`, `skipped`).
- **Forget button disabled or refused on the web UI.** Forgetting must be enabled, and web config writes must be allowed
  (`security.web_config_write_enabled`).
- **Rejected claims or dream items.** Look in `<home>/logs/audit.jsonl` and filter on the `kind` field:
  `memory_claim_rejected`, `memory_consolidation_parse_failed`, `memory_dream_rejected`, `memory_dream_parse_failed`,
  `memory_forget_staged`, `memory_forget_unstaged`.
- **Index looks stale or corrupt.** `index.usearch` is a cache. Delete it and it is rebuilt from `tier2.db` on next use.
