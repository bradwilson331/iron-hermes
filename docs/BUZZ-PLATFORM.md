# Buzz Platform (Nostr)

IronHermes can run as a native member of a **Buzz** workspace — Buzz is a chat client built on
the **Nostr** protocol. Instead of a shared bot token, each Hermes profile signs every message
with its own cryptographic keypair, so anyone on the relay can verify who actually sent it.

This guide covers everything an operator needs: standing up an identity, wiring `config.yaml`,
choosing a relay, understanding the access model, and recognizing this integration's current
limitations. For the other gateway platforms (Telegram/Discord/Slack) see
[`MULTI-PLATFORM-GATEWAY.md`](MULTI-PLATFORM-GATEWAY.md) — this document is its Buzz sibling and
follows the same structure.

Buzz support is compiled behind a Cargo feature flag (`buzz`) on `ironhermes-cli` and
`ironhermes-gateway`. If your build was produced without `--features buzz`, none of the `buzz`
CLI subcommands or the Buzz gateway adapter exist in the binary.

## Relay membership (read this before anything else)

Being on the whitelist is not enough. Most Buzz community relays are **membership-gated**:
the agent's Nostr identity must be added as a relay member before any of this works — before
whitelist checks, before mention gating, before anything else. A gateway with a perfectly
correct `config.yaml` and a perfectly correct whitelist entry will still have every publish
and every subscription rejected by the relay, with:

```
restricted: not a relay member
```

if the agent's pubkey has not been granted membership.

Two ways to grant membership (both proven live against a hosted Buzz community relay):

1. **Via the Buzz UI.** A relay/community admin adds the agent's npub as a member through the
   workspace's member-management screen (a `kind:9030` relay-admin add-member event under the
   hood).
2. **Via a NIP-98 signed invite claim**, if the relay/workspace issues invite links.

Run `ironhermes buzz keygen` (or `ironhermes buzz pubkey`) first, hand the printed npub to
whoever administers the relay, and confirm membership before debugging anything else —
`restricted: not a relay member` is the single most common first-run failure and has nothing
to do with `config.yaml`.

## Quick start

1. Generate an identity for the active profile:

   ```bash
   ironhermes buzz keygen
   ```

   This prints something like:

   ```
   Generated a new Buzz identity.
     npub: npub1exampleexampleexampleexampleexampleexampleexampleexampleexamplex
     written to: /home/you/.ironhermes/.env

   Put this npub in the OTHER profile's whitelist so the two-profile UAT can see it.
   ```

   **Convert the printed npub to its 64-character hex form before adding it to the
   whitelist.** This is the single step operators get backwards: whitelist matching compares
   entries against the sender's **hex** pubkey with an exact, case-sensitive string match — it
   does NOT decode `npub1…` bech32 strings. Run `ironhermes buzz pubkey` to print both forms,
   and copy the hex one into `whitelist:`. Pasting the bare `npub1…` string into the whitelist
   looks correct but silently matches nothing; every message from that identity is then
   dropped with a `not in whitelist` warning.

2. Add a `buzz` platform section to `config.yaml`:

   ```yaml
   gateway:
     platforms:
       buzz:
         enabled: true
         relay_url: "wss://relay.example.org"
         channels: ["<channel-id-from-your-buzz-workspace>"]
         whitelist: ["<your-own-64-char-hex-pubkey>"]
   ```

3. Start the gateway:

   ```bash
   ironhermes gateway run
   ```

4. In the Buzz app, @-mention the agent in the configured channel. Expect a signed reply.

## Configuration reference

Every Buzz-relevant key lives under `gateway.platforms.buzz` in `config.yaml`, alongside
Telegram/Discord/Slack's sections. The whitelist field is now **shared canonical
infrastructure** across every platform — see below.

| Key | Type | Default | Effect |
|---|---|---|---|
| `enabled` | bool | `false` | Buzz is the only platform where this flag is a REAL gate. Telegram/Discord/Slack treat it as informational (their credential resolving is the actual gate); Buzz with `enabled: false` is skipped even if `relay_url`/`channels` are otherwise valid. |
| `relay_url` | string, optional | `None` | The single Nostr relay this Buzz section connects to (e.g. `wss://relay.example.org`). One relay per gateway process — no multi-relay discovery this phase. |
| `channels` | list of strings | `[]` | Buzz channel/group identifiers (NIP-29 `#h` tag values) the adapter subscribes to and posts into. |
| `channel_trust` | `closed` \| `open` | `closed` | See **Access control** below. |
| `whitelist` | list of strings | `[]` | The **canonical cross-platform allowlist** — holds Telegram numeric chat IDs, Slack member IDs, Discord numeric user IDs (all as strings), and Buzz hex pubkeys, all in the same list. Empty = deny all. |
| `home_channel_id` | string, optional | `None` | Disambiguates which channel/chat a bare platform target (e.g. `deliver=buzz` with no explicit channel) resolves to, when the whitelist doesn't have exactly one entry. |
| `display_name` | string, optional | active profile name | Not a dedicated config field — accepted through the section's flatten catch-all (any extra YAML key you add under `buzz:` that isn't one of the fields above is preserved and readable by the adapter). Sets the readable name published in the agent's `kind:0` profile metadata, so it appears in the Buzz member list as a name rather than a bare npub. Falls back to the active profile's name if unset or empty. |

**Existing configs keep working.** `whitelist` used to be Telegram-only integers
(`whitelist: [12345, 67890]`). It is now a shared `Vec<String>` field used by every platform,
but the deserializer accepts either bare YAML integers or quoted strings per entry and coerces
both to the same canonical string form — a pre-migration config with bare numbers loads with
zero changes required. A Buzz hex pubkey is simply another string in the same list.

**Whitelist matching is hex-only, exact string match — confirmed live.** `whitelist_allows`
compares each inbound sender's hex pubkey against the whitelist with `==`; it never decodes
`npub1…` bech32 strings. Convert every Buzz identity to its 64-character hex form (via
`ironhermes buzz pubkey`) before adding it to `whitelist:`. Accepting bech32 npub entries
directly is a candidate future enhancement, not shipped behavior.

## Identity and key handling

The Nostr secret key (`nsec`) is resolved through **one seam**, in this order:

1. The `BUZZ_NSEC` environment variable.
2. The active profile's `.env` file (same file Telegram's bot token and provider API keys live
   in), also under the key `BUZZ_NSEC`.

It is **never read from `config.yaml`** — there is no `nsec` field on the Buzz platform section,
by design, so a committed or shared `config.yaml` cannot leak it.

`ironhermes buzz keygen` and `ironhermes buzz import` both write to the profile's `.env`,
single-quoted (`BUZZ_NSEC='nsec1…'`), through the shared writer that verifies the value
round-trips through the real `.env` parser before considering the write successful. Never paste
a real secret into a shared file or chat to "test" a config — every example in this document
uses an obviously fake placeholder.

```bash
ironhermes buzz keygen            # generate a fresh identity, refuses if one already exists
ironhermes buzz keygen --force    # replace an existing identity (see rotation below)
ironhermes buzz import            # paste an externally-generated nsec1... or 64-char hex, via a masked prompt
ironhermes buzz import --force    # same, replacing an existing identity
ironhermes buzz pubkey            # print the active profile's npub + hex form, no secret shown
```

None of these subcommands accept the secret as a command-line argument or flag — argv is
visible in the process table to every user on the machine. `import`'s only input path is an
interactive masked prompt.

**Key rotation.** To rotate an identity: run `ironhermes buzz keygen --force`, then update
every peer's whitelist with the new npub. The old identity's signed history stays on the relay
permanently — Nostr events are immutable, so rotation does not erase the old identity's past
messages, it only stops the old key from being trusted going forward.

**Key-ceremony note** (from the ADR's key-compromise mitigation): treat the `nsec` like any
other credential — store it in the profile `.env`, never commit it, and rotate immediately on
suspected compromise. There is no vault integration yet; a future per-profile vault migration
(a separate, unexecuted phase) is expected to pick the `nsec` up alongside other profile
secrets through this same one-seam resolver, with no change needed on the operator's side.

## Access control

Buzz has two trust modes, set via `channel_trust`:

- **`closed`** (the default). The whitelist gates all inbound interaction — channel messages,
  DMs, and approval commands. An empty whitelist denies everyone.
- **`open`**. Channel membership itself implies interaction rights **in channels only**.
  Enabling this is an explicit operator opt-in, and the gateway logs a startup warning naming
  the relay whenever it is active:

  ```
  Buzz channel_trust is OPEN — channel membership alone now implies interaction rights (D-08)
  ```

  `open` does **not** widen three things, regardless of setting it:
  1. **DMs** — the whitelist gates every direct message in both trust modes.
  2. **Approvals** — the pending-approval `/approve`/`/deny` flow always re-validates the
     responding sender's pubkey against the whitelist, in both trust modes.
  3. **Approval commands posted in a channel** — `/approve`/`/deny` is rejected outright if it
     arrives on a channel event rather than a DM, regardless of trust mode. A rejection here is
     silently dropped (never answered) and logged with the marker `T-47.6-06-SPOOF`.

**Security note: whitelist humans only.** The Buzz dispatch path does not yet route through
the session-queue caps that gate Telegram, and there is no per-sender rate limit or loop
guard. If an auto-replying bot identity were ever added to the whitelist — and its own replies
mention the agent back — the two could ping-pong indefinitely (mutual whitelist + mutual
mention). Today this is bounded only by the deny-all-by-default whitelist and relay membership
gating; treat both as load-bearing, and do not whitelist any identity you do not control and
trust to behave like a human correspondent. Routing Buzz dispatch through the session-queue
caps and/or adding an explicit per-sender rate limit is deferred hardening for a follow-up
phase.

## Threading and mentions

Buzz threads are **flat** (Slack-style), not nested. This produces two behaviors an operator
should expect, both confirmed against a live hosted Buzz relay:

- **Every message needs an explicit @-mention, including in-thread follow-ups.** The mention
  gate checks for a `p`-tag naming the agent's own pubkey on EVERY inbound channel event —
  there is no "the agent already replied in this thread, so it's listening" exception. An
  un-mentioned follow-up inside an existing thread is silently ignored, exactly like an
  un-mentioned message anywhere else in the channel. This is expected behavior, not a bug;
  mention-free thread-following is a candidate future enhancement, not shipped.
- **Replies always target the thread ROOT, never a mid-thread parent.** Because Buzz's
  threading is flat, an `e`-tag pointing at anything other than the thread's root event is
  rejected by the relay with:

  ```
  invalid: root tag does not match thread ancestry
  ```

  The adapter resolves and tags the thread root automatically — an operator does not need to
  do anything here, but this is the failure signature to recognize if a fork or downstream
  change ever regresses it.

## Multi-profile workspaces

The intended shape (from the ADR) is one Nostr keypair, one gateway process, one channel, per
profile — e.g.:

```
#research channel:  researcher profile (own keypair, own SOUL.md, own skills)
#content channel:   content profile   (own keypair, own SOUL.md, own skills)
#infra channel:     devops profile    (own keypair, own SOUL.md, own skills)
#general channel:   chief-of-staff profile (own keypair, own SOUL.md, own skills)
```

`--profile` pivots the home directory (`IRONHERMES_HOME`), which is what makes each profile's
identity, memory, and skills separate with no extra configuration — one profile equals one
keypair equals one gateway process.

**This phase proves the two-profile version live.** The four-profile workspace sketched above
is documentation only — it has not been run or verified end-to-end in this phase. Only the
two-profile case (two `--profile`-pivoted gateway processes, two keypairs, two channels, on one
relay) has been proven with a live human verification.

## Relay selection criteria

No recommended relay list ships with this release, and that question stays open. Instead,
evaluate a candidate relay against these criteria:

- **Does it implement NIP-42 authentication?** This adapter authenticates to the relay via
  NIP-42; a relay without it may reject the connection or behave unpredictably.
- **Does it accept the NIP-29 group/channel kinds and NIP-17 gift-wrap kinds this adapter
  uses?** Some relays restrict which event kinds they will store or relay.
- **Is it self-hosted or third-party — and who can see event metadata as a result?** Channel
  content is NOT end-to-end encrypted (only DM content is, via NIP-17 gift-wrap); anyone
  operating the relay can see channel messages and metadata. Self-hosting removes this
  question entirely.
  - Buzz's own relay: the same relay a Buzz workspace already routes through.
- **What is its retention policy?** This adapter does no backfill of events published while the
  gateway was offline (see Limitations below) — a relay that discards old events on its own
  schedule compounds this gap.
- **What happens to the deployment if the relay disappears?** A relay outage surfaces as a
  logged degraded state (connection lost, exponential-backoff reconnect); there is no automatic
  failover to a second relay this phase (one relay per gateway process).

## Cron and kanban delivery

`deliver=buzz` is a first-class cron delivery target, alongside `telegram`. A cron job's TEXT
output posts to the configured Buzz channel or DM the same way a Telegram cron delivery does.
Confirmed live against a hosted Buzz relay:

```bash
ironhermes cron create --name buzz-daily-standup \
  --schedule "0 9 * * *" \
  --prompt "Summarize yesterday's kanban activity" \
  --deliver "buzz:<channel-id>"
```

The home-channel environment variables for cron follow the existing per-platform naming
convention:

- `BUZZ_HOME_CHANNEL` — overrides which channel/chat a bare `deliver=buzz` target resolves to.
- `BUZZ_HOME_CHANNEL_THREAD_ID` — an optional thread/reply-target qualifier.

Kanban board events with a Buzz subscription post through the same notifier path Telegram
uses, configured with a `default_notify` block under `kanban:` in `config.yaml`. Also
confirmed live:

```yaml
kanban:
  default_notify:
    platform: buzz
    chat_id: "<channel-id>"
```

**Media on Buzz (D-15):** Nostr has no native media upload in this release. When a cron job's
output or a kanban event carries a media reference, Buzz delivery sends a **text message**
naming the artifact and its local path, immediately after the body text — for example:

```
Media attached (not embeddable on Buzz yet):
- /home/you/.ironhermes/artifacts/chart.png
```

The image never silently vanishes; it is always named in a follow-up text message. The same is
true when the agent itself generates media mid-turn on Buzz.

## What does not work yet

- **No message editing.** Nostr events are immutable once published, so `edit_message` is a
  logged no-op on Buzz. Responses post once, when the agent's turn completes, rather than
  growing incrementally the way a Telegram reply does. See "Responses arrive all at once"
  below.
- **No reactions or typing indicator.** Both remain unimplemented (the trait's default no-op
  applies, same posture as Discord/Slack) — reaction-based approvals in particular need
  spoof-hardening research not yet done.
- **No media upload.** Buzz has no native media transport in this release; see the D-15 text
  notice above.
- **No group administration from the gateway.** Operators create and administer Buzz channels
  in the Buzz app itself; the gateway only joins and posts to channels it is configured with.
- **A single relay per process, no multi-relay discovery.** One `relay_url` per gateway
  process; there is no recommended-relay list and no automatic multi-relay failover.
- **WebSocket-only transport, no CLI-polling fallback.** Unlike Telegram (which can fall back
  to polling), Buzz connectivity is WebSocket-only this release. A relay outage surfaces as a
  logged degraded state with exponential-backoff reconnect, not a transport switch.
- **The Buzz desktop client's DMs do not reach this adapter at all (confirmed live).** Live
  UAT against a hosted Buzz relay showed the Buzz desktop app does NOT send NIP-17
  gift-wrapped DMs — it sends hidden NIP-29 group channels carrying `kind:9` events (the
  desktop client's own internal stream-message kind). This adapter's DM path subscribes on
  NIP-17 (`kind:1059` with a `#p` filter for the agent's own pubkey) and never sees these
  events. A DM sent from the Buzz desktop app to the agent gets no reply — not because
  whitelist or mention gating failed, but because the message never arrives on the wire this
  adapter listens to. **Consequence: DM chat and DM-only approval commands (`/approve`,
  `/deny`) do not work with Buzz desktop users today.** A hidden-group DM path is required to
  close this gap and is deferred to a follow-up phase. (This adapter's NIP-17 path itself is
  correct and works with any client that actually sends standard NIP-17 DMs — it is
  specifically the Buzz desktop app's own DM implementation that does not use NIP-17.)
- **No backfill of events published while the gateway was down.** Inbound Buzz messages flow
  into the existing session-storage machinery exactly like Telegram — there is no
  relay-history re-fetch or since-filter replay on reconnect. A mention sent while the gateway
  was offline is simply missed; restarting does not retroactively deliver it.
- **No ACP multi-agent interop yet.** Buzz's own multi-agent vision (Hermes + Claude Code +
  Codex + Goose all in one channel) requires an ACP (Agent Communication Protocol) layer that
  has not been ported to Rust or bridged in this release — only a findings-only validation
  spike exists.

Three further limitations are not visible in the capability list above and are easy to mistake
for bugs:

- **The agent cannot proactively send a Buzz message.** The `send_message` tool the agent has
  access to is wired for Telegram only; on Buzz the agent can reply to a mention or DM, but it
  cannot start a Buzz conversation on its own initiative (e.g. an autonomous notification with
  no prior inbound message to reply to). This is deferred — it is not in this release's
  change list — and will be revisited alongside a future platform-taxonomy unification.
- **Deleting an old message can fail.** The adapter tracks which event IDs it has itself
  published in an in-memory bounded set — capped at the 512 most recent events, and only for
  the current process's lifetime. Asking it to delete a message it authored before a restart,
  or one that has aged out of that 512-event window, returns an error rather than silently
  doing nothing or deleting the wrong thing. This is a deliberate fail-closed design, not a
  bug.
- **Responses arrive all at once, not progressively.** On Telegram, a reply visibly grows as
  the agent thinks, because the same message is repeatedly edited. On Buzz, nothing appears
  until the agent's turn is completely finished, because a Nostr event cannot be edited after
  it is published — the whole answer is buffered and sent then, chunked across multiple
  consecutive messages for long answers. An operator watching an empty channel for the first
  thirty seconds of a turn is not looking at a broken deployment; this is expected on Buzz.

## Logging and diagnosis

The gateway does not write a log file for Buzz (or any platform) — logs go to stderr only,
and the default filter (`ironhermes=info`) will not show the Nostr SDK's own connection,
auth, and subscription chatter. When diagnosing a live issue, restart with:

```bash
RUST_LOG="info,nostr_sdk=debug" ironhermes gateway run 2>&1 | tee buzz-debug.log
```

so you have both the filtered gateway-level `info!`/`warn!` lines and the SDK's low-level
protocol trace, captured to a file you can search afterward. Three failure signatures worth
grepping for first — each confirmed live and each pointing at a different section of this
document:

- `restricted: not a relay member` — the agent's pubkey has not been granted relay
  membership. See **Relay membership** above.
- `invalid: root tag does not match thread ancestry` — a threaded reply's `e`-tag did not
  point at the thread root. Should self-correct automatically; see **Threading and mentions**
  above.
- `not in whitelist` — the sender's hex pubkey is not present in `whitelist:`. Check for a
  pasted `npub1…` string where a hex string was required; see **Quick start** above.

## Troubleshooting

- **The gateway refuses to boot and lists platforms.** If no configured platform's credentials
  resolve, the gateway logs and exits with a message like:

  ```
  No usable messaging platform is configured. Configure at least one of telegram, discord,
  slack, or buzz under gateway.platforms in config.yaml with a resolvable credential.
  ```

  or, if at least one platform was attempted but failed:

  ```
  No usable messaging platform is configured. Attempted: [buzz: Buzz identity not resolved: ...]
  ```

  For Buzz specifically, this means `BUZZ_NSEC` did not resolve from either the environment or
  the profile `.env` — run `ironhermes buzz keygen` or `ironhermes buzz import` first.

- **The agent connects but never replies in a channel.** Three likely causes, confirmed live:
  (0) the agent is not yet a relay member — check the logs for `restricted: not a relay
  member` (see **Relay membership** above); (1) the message did not @-mention the agent — the
  mention gate requires an explicit @-mention/`p`-tag on EVERY message, including follow-ups
  inside a thread the agent already replied in (see **Threading and mentions** above); or (2)
  the sender's pubkey is not in the whitelist under `channel_trust: closed` — an unauthorized
  sender's message is silently dropped with a `not in whitelist` `warn!`-level log line, never
  a visible error to the sender.

- **The agent never sees DMs.** Two distinct causes. First, check whether the sender is using
  the **Buzz desktop client** — its DMs do not reach this adapter at all today; see the
  desktop-DM limitation above before debugging anything else. If the sender is using a
  standard NIP-17 client, the DM subscription is constrained by a `#p` tag naming the agent's
  own pubkey — Buzz's relay enforces this constraint at the wire level and rejects a
  subscription that omits it, returning zero events, which looks identical to "nobody DMed us"
  unless you check the logs for a DM subscription rejection warning. If DMs never arrive, check
  the gateway's logs for a relay rejection message at startup.

- **Approvals arrive but `/approve` does nothing.** Approvals are DM-only by design — an
  `/approve <id>` or `/deny <id>` posted in a channel is rejected and logged with the marker
  `T-47.6-06-SPOOF`, even from an otherwise-whitelisted sender. Reply to the approval DM
  directly, from the same whitelisted pubkey the prompt was sent to.

- **A cron job reports no adapter available.** `deliver=buzz` (or `deliver=telegram`) requires
  the gateway itself to be running — the gateway process is what hosts the cron dispatch loop
  and constructs the delivery registry each tick. A cron job created but never run through a
  live gateway process has nothing to deliver through.
