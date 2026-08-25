# Agent Client Protocol (ACP)

IronHermes speaks the [Agent Client Protocol](https://agentclientprotocol.com/) — a
JSON-RPC 2.0 wire protocol, spoken over stdio, that lets an editor drive an agent inside
its own agent panel instead of a terminal. ACP is maintained by Zed and implemented by a
growing set of editors; **Zed** is the reference client and this phase's acceptance bar
(see [Zed live UAT](#zed-live-uat--the-acceptance-bar) below). **VS Code** and
**JetBrains** IDEs (via their own ACP-speaking extensions) can also drive `ironhermes acp`
— those configurations are best-effort here, since ACP client maturity varies editor to
editor.

The server is the `ironhermes acp` subcommand on the same binary you already build or
install — there is no separate ACP binary and no daemon. The editor spawns
`ironhermes acp` as a child process, talks JSON-RPC over its stdin/stdout, and the process
exits when the editor closes the connection.

## First run: authentication

Before configuring an editor, make sure IronHermes itself is set up — the same setup a
terminal user would run:

```bash
ironhermes setup
```

This walks you through selecting a provider and storing credentials in
`~/.ironhermes/.env` (or a profile-scoped equivalent, see [Profiles](#profiles) below).
When an editor connects and no provider credentials resolve yet, the ACP `initialize`
handshake still advertises a usable auth method — `terminal`, whose description is
literally "Run `ironhermes setup`" — so a fresh install is never stuck with zero usable
auth methods. Once a provider is configured, `initialize` also advertises a
`resolved-provider` method that authenticates silently using those already-resolved
credentials.

## Zed configuration

Zed is configured through the `agent_servers` section of Zed's `settings.json` (open with
`Cmd+,` / `Ctrl+,`, or the command palette → "zed: open settings"). Point it at the
**absolute path** to your locally built or installed `ironhermes` binary with the `acp`
subcommand:

```json
{
  "agent_servers": {
    "IronHermes": {
      "type": "custom",
      "command": "/absolute/path/to/ironhermes",
      "args": ["acp"]
    }
  }
}
```

`"type": "custom"` is required by current Zed releases for any externally-defined stdio
agent server — omitting it fails with `property "type" is missing`. Older Zed releases
accepted an `agent_servers` entry without a `type` field; if you're on a recent Zed build
(as most users will be), include it.

Find the absolute path with `which ironhermes` (installed via the one-line installer) or,
for a locally built binary, the path under `target/release/ironhermes` /
`target/debug/ironhermes` relative to your `iron-hermes` checkout.

Restart Zed, open a project directory, open the agent panel, and select "IronHermes" as
the agent.

### Profiles

To run a non-default profile (an isolated `IRONHERMES_HOME` under
`~/.ironhermes/profiles/<name>/`), add the global `--profile` flag to `args`, **before**
`acp`:

```json
{
  "agent_servers": {
    "IronHermes (work)": {
      "type": "custom",
      "command": "/absolute/path/to/ironhermes",
      "args": ["--profile", "work", "acp"]
    }
  }
}
```

## VS Code configuration (best-effort)

VS Code does not speak ACP natively; it requires an ACP-bridging extension (client
maturity and configuration UI vary by extension and are outside IronHermes's control).
The shape is the same as Zed's: point the extension's agent-server command at the
absolute path to `ironhermes`, with `acp` as the sole argument (plus `--profile <name>`
before `acp` if you need a non-default profile). Consult your specific extension's
documentation for where that command/args pair is configured — some use a JSON settings
key, others a UI form.

## JetBrains configuration (best-effort)

Similarly, JetBrains IDEs support ACP through a plugin, not a built-in client. Configure
the plugin's custom agent entry with the same command shape: absolute path to
`ironhermes`, `acp` as the argument (with `--profile <name>` prepended for a non-default
profile). Refer to your JetBrains ACP plugin's own settings documentation for the exact
configuration surface — it is expected to accept an arbitrary command + args pair, the
same contract Zed and VS Code use.

## Buzz as ACP client

[Buzz](https://buzz.communities.xyz) — the Nostr-based multi-agent chat platform (see
[`docs/BUZZ-PLATFORM.md`](BUZZ-PLATFORM.md) for the native gateway integration) — can also
drive `ironhermes acp` as one of the agents in its `AgentPool`, the same slot Goose or
Claude Code occupy. This is a different integration than the native `buzz` gateway adapter
that doc covers: here **Buzz spawns and owns the IronHermes process**, the way Zed does,
not the other way around.

### Configuration

Point `buzz-acp` at the `ironhermes` binary either through environment variables:

```bash
export BUZZ_ACP_AGENT_COMMAND=/absolute/path/to/ironhermes
export BUZZ_ACP_AGENT_ARGS="--profile,buzz-agent,acp"
```

or the equivalent CLI flags:

```bash
buzz-acp --agent-command /absolute/path/to/ironhermes \
         --agent-args=--profile,buzz-agent,acp
```

On the CLI, the value **must use the attached `--agent-args=...` form** (confirmed in the
2026-08-11 live UAT): because the value itself starts with `--`, the space-separated form
(`--agent-args "--profile,..."`) fails clap parsing on the Buzz side.

`--profile` is a **global** flag and must precede the `acp` subcommand — exactly like the
[Profiles](#profiles) example above, just comma-delimited instead of a JSON array
(`--agent-args`/`BUZZ_ACP_AGENT_ARGS` splits on commas). Reversing the order
(`"acp,--profile,buzz-agent"`) does not parse the way you might expect from CLIs where
subcommand flags follow the subcommand — it fails clap parsing, and on the Buzz side this
shows up as the spawned process exiting immediately, not as a clear argument-order error.

**Who the bridge answers is gated twice.** buzz-acp's `--respond-to` defaults to
`owner-only`, and with no owner configured that default **silently drops every inbound
event** (a WARN in buzz-acp's log is the only trace). And DMs are gated separately: DM
authors are accepted only if they match the configured owner, **regardless of
`--respond-to anyone`** — set `BUZZ_ACP_AGENT_OWNER=<hex pubkey>` or DMs never reach the
agent. Both behaviors were confirmed in the live UAT; "the agent never answers" with a
clean agent log is usually one of these two, not an agent failure.

### Getting a reply back into Buzz

**buzz-acp does not publish the agent's streamed text.** It logs `agent_message_chunk`
frames; nothing you stream over ACP reaches the channel. The reply contract (from
buzz-acp's own base prompt) is that the **agent self-publishes** by running
`buzz messages send` as a terminal command. Two consequences for IronHermes operators:

- The spawned profile needs `terminal_env_allowlist: [BUZZ_RELAY_URL, BUZZ_PRIVATE_KEY,
  BUZZ_AUTH_TAG]` so the `buzz` CLI can authenticate — IronHermes's terminal env scrub
  strips those variables otherwise, and `buzz messages send` fails with
  `BUZZ_PRIVATE_KEY is required`. **Security caveat:** with that allowlist, *every*
  terminal command the agent runs can read the Buzz private key. That exposure is
  inherent to buzz-acp's self-publish design, not something IronHermes can scope
  tighter on this path.
- Delivery depends on the model actually following the self-publish instruction. In the
  live UAT, `claude-haiku-4.5` chatted over ACP but never ran `buzz messages send`
  (replies never appeared in Buzz); `claude-opus-4.8` complied. Prefer a stronger model
  tier for a Buzz-facing profile until buzz-acp bridges replies itself.

### Dedicated profile

Give the Buzz agent its own profile — `buzz-agent` in the example above — for the same
one-profile-per-identity reason as [Profiles](#profiles): its own memory, sessions, and
credentials, with no cross-surface bleed from Zed or CLI use on the same machine. Keep that
profile's MCP server list short and fast-starting, and its toolset and approval policy
conservative (tie this to the trust boundary below) — a minimal, deliberate set of tools,
not a blanket-approved profile.

**Set the profile's model at the provider level.** A per-provider
`providers.<name>.default_model` silently overrides the top-level `model.default`
(provider overlay precedence, `provider.rs:296` vs `:262`) — editing the obvious
top-level key is a no-op if the provider overlay carries its own. In the live UAT the
boot model kept resolving to the overlay's value until
`providers.openrouter.default_model` itself was changed. Check the provider block first
when the running model is not the one you configured.

**buzz-acp wraps `session/new` in a fixed, non-overridable 60-second timeout**, during which
IronHermes starts that profile's configured MCP servers. There is no flag or env var on the
Buzz side to raise this ceiling. If the Buzz profile's MCP server list is heavy,
`session/new` can time out before IronHermes ever gets a turn started — keep it lean.

### Trust boundary

**IronHermes's Buzz pubkey whitelist does not run for ACP-spawned sessions.** That gate
belongs to the native gateway adapter documented in
[`docs/BUZZ-PLATFORM.md`](BUZZ-PLATFORM.md#access-control); on the ACP path, Buzz owns who
can reach the agent, end to end — the whitelist you configure under
`gateway.platforms.buzz` has no effect here at all. Treat any community-exposed Buzz
profile as if it has no whitelist, and compensate with a conservative toolset and approval
policy for that profile, rather than assuming access control that does not hold on this
path.

buzz-acp resolves sender identity into free text baked into the prompt content, not a
structured or authenticated field — see [Sender identity over the ACP
path](#approval-trust-and-edit-review) above for the source-verified finding: it is
model-visible text, and IronHermes does not (and this phase does not add anything that
would) treat it as proof of who is actually asking.

### Approvals

buzz-acp has no human permission prompt in its harness, and it auto-denies every
`session/request_permission` IronHermes sends — it selects the `reject_once` option
IronHermes already offers (the same request/response shape Zed's interactive prompt uses).
So on the Buzz ACP path, every approval-gated tool call stays denied by default; the
deliberate opt-in is the Buzz profile's own toolset and approval configuration, not an
interactive approve/deny in the moment.

When a call is denied this way, it still reaches the model: the tool result carries
`not run - denied by IronHermes approval policy: ` as a prefix, and the terminal
`tool_call_update` is headlined `BLOCKED - this tool call was not executed.` — but buzz-acp
logs `tool_call_update` frames (`tracing::info!`) rather than rendering them into the Buzz
channel, so the channel-visible explanation is the assistant's own reply text. The
denial-marked tool result is exactly the material the model has to write that explanation
from — "why didn't it run the command?" is answerable from the assistant's reply, not from
a separate UI element Buzz doesn't show.

### Long-running turns

IronHermes emits a periodic keepalive `tool_call_update` on the in-flight tool call
whenever 60 seconds (the default) pass with no other traffic during a turn — well under
buzz-acp's default idle timeout. Tune it with `IRONHERMES_ACP_KEEPALIVE_SECS` (seconds; `0`
disables the heartbeat entirely).

This interacts with two knobs on the Buzz side:

- `--idle-timeout` / `BUZZ_ACP_IDLE_TIMEOUT` (default `900` seconds) — how long buzz-acp
  will wait with no traffic at all before giving up on a turn.
- `--max-turn-duration` / `BUZZ_ACP_MAX_TURN_DURATION` (default `7200` seconds, hard
  operator ceiling `604800` seconds / 7 days) — the absolute cap on one turn, regardless of
  traffic.

buzz-acp itself validates that the idle timeout is strictly less than the max turn
duration — raising one without checking the other can fail Buzz's own config validation,
not IronHermes's. An operator with an especially long-running workload (a very slow tool,
or a very long agent turn) should raise the Buzz-side idle timeout and/or max turn duration
together, and can lower `IRONHERMES_ACP_KEEPALIVE_SECS` if they want the heartbeat itself
to fire more often as a safety margin.

### Capability boundary specific to Buzz

Beyond the general [Capability
boundary](#capability-boundary--what-ironhermes-does-not-implement-over-acp) above, a few
things an operator would otherwise discover by surprise when driving IronHermes from Buzz
specifically:

- **No `session/set_config_option`.** buzz-acp tries to set permission mode right after
  `session/new`; IronHermes doesn't implement the method, so buzz-acp gets a standard
  "method not found" response, logs a warning, and falls back to per-request rejection —
  not an error, and not something you need to fix.
- **No `agent_thought_chunk`.** There is no separate reasoning stream to expose.
- **No Goose-namespaced `_meta` fields** (e.g. `_meta.goose.activeRunId`). Those are a
  Goose-specific vendor extension buzz-acp also understands; emitting them from IronHermes
  would be impersonating a different agent, not compatibility.
- **No `session/load` on this path.** buzz-acp never calls it — it doesn't reload sessions
  the way Zed's "reopen a conversation" flow does.

### Tested-against versions

This crate pins `agent-client-protocol = "2.0.0"` and
`agent-client-protocol-schema = "1.5.0"` (see the workspace `Cargo.lock`).

**Buzz version/commit tested:** local `buzz` checkout at commit `7f7db63db`
(2026-08-06), run via `cargo run -p buzz-acp`, live UAT on 2026-08-11 against
`wss://ironhermes.communities.buzz.xyz` with the Buzz desktop client. (The `buzz` CLI
exposes no `--version` flag; the checkout commit is the version identifier.) UAT scope:
the mention → streamed-reply path and an in-session context follow-up were confirmed
live end-to-end; cancellation, long-running-tool keepalive, policy-denial visibility and
the Zed re-check were deliberately deferred — see
`.planning/phases/47.7-buzz-using-ih-acp/47.7-UAT-EVIDENCE.md`.

---

See [`docs/BUZZ-PLATFORM.md`](BUZZ-PLATFORM.md#two-ways-to-connect-ironhermes-to-buzz) for
how this path compares to the native gateway adapter, and which one to pick for a given use
case.

## Install story: this phase is docs-only

There is no one-click "install IronHermes into Zed's agent registry" button yet. This
phase's install story is exactly the settings snippet above — copy it into your editor's
config and point it at your own binary. Registry publication (a Zed agent-registry
listing, or an equivalent `acp_registry`-style one-click install) is deferred to the
packaging phase; nothing here should be read as a promise of a discoverable listing.

## Capability boundary — what IronHermes does NOT implement over ACP

IronHermes implements the full session lifecycle (create/load/fork/list/cleanup),
streamed prompts, tool-call rendering with diffs, and a fail-closed permission bridge.
The following ACP capabilities are explicit, deliberate opt-outs — not gaps that were
missed, not bugs, and not silent failures. If you hit one of these, this is why:

| Capability | Why IronHermes does not implement it |
|---|---|
| **Client-supplied MCP servers** (`session/new`'s `mcpServers`) | Acknowledged in the handshake, logged, and never spawned. IronHermes keeps using its own configured MCP servers instead. Accepting a client-supplied server list is a deferred follow-up, not shipped this phase. |
| **Editor-side filesystem methods** (`fs/read_text_file`, `fs/write_text_file`) | These let the agent ask the *editor* to read/write files on its behalf. IronHermes's own file tools already do this directly against the filesystem — the editor-mediated path is out of scope. |
| **Editor-side terminal methods** (`terminal/create`, `terminal/output`, `terminal/wait_for_exit`, `terminal/kill`, `terminal/release`) | Same story as the filesystem methods — IronHermes runs shell commands through its own sandboxed `terminal`/`execute_code` tools, not by asking the editor to open a terminal panel for it. |
| **Mode selection** (`session/set_mode`, `current_mode_update`) | IronHermes has no agent-mode concept (e.g. "plan mode" vs "act mode") to expose. |
| **Model selection** (`session/set_model`) | There is no provider-scoped model catalog in IronHermes yet to enumerate and offer as a picker — model selection stays a `config.yaml` concern. |
| **Session resume via `session/resume`** | This is a v2 draft RFC method, not the stable v1 method IronHermes implements. `session/load` (the stable v1 resume path) is what reopens a previous conversation — see [Session reload](#session-reload) below. |
| **Multiple additional directories per session** (`session/additional_directories`) | Every ACP session is bound to exactly one editor-supplied working directory, which drives both tool execution and project-context discovery. Multi-root sessions would fork that single-root contract and aren't supported. |
| **Plan/todo updates** (`plan` notifications) | IronHermes has no plan/task decomposition surface on the agent runtime to source structured plan entries from. |
| **Live reasoning stream** (`agent_thought_chunk`) | The backend has no distinct "thinking" stream separate from the final response — sending thought-chunk updates here would mean fabricating reasoning content that doesn't exist. |
| **Slash-command exposure** (`available_commands_update`) | IronHermes has an internal command router but exposing it as ACP slash commands is an open, undecided idea, not something shipped this phase. |
| **Session titles/summaries** (`session_info_update`) | No session title/summary generation exists yet to populate this. |
| **Credential revocation** (`logout` / `AgentAuthCapabilities.logout`) | ACP itself owns no credential store — authentication delegates to your existing provider credential configuration, so there's nothing ACP-specific for the agent to revoke. |

None of these are silent — if your editor invokes one of them, it gets a standard
JSON-RPC "method not found" response (or the capability flag is simply absent from
`initialize`'s response), never a hang or a crash.

## Approval, trust, and edit review

Dangerous operations — shell commands the guardrail classifies as risky, and **every**
`execute_code` call regardless of its content — raise an ACP `session/request_permission`
request before running. You'll see a permission prompt in your editor with three choices:
allow this time, allow always (for the rest of this session), or reject.

**Choosing "allow always" only lasts for the current editor session.** It suppresses
further prompts for that same operation class for as long as the session stays open —
nothing is ever written to disk, and there is no persistent grant file to inspect or
delete. Closing the editor (or restarting it, or reconnecting) resets trust back to zero;
the very next matching operation prompts again.

**If your editor cannot show permission prompts at all** — either because it doesn't
support `session/request_permission` or the request times out with no answer — every
dangerous operation is refused with an explanatory tool error. IronHermes never falls back
to running it anyway. This is a deliberate fail-closed posture: silence is treated the
same as an explicit "no."

File edits (`write_file`/`patch`) always show a reviewable diff before writing, whether
the target is inside or outside the session's working directory — a write that resolves
outside the workspace root (via `..` traversal or a symlink) is additionally treated as a
dangerous operation and routed through the same permission path, with the reason stated
explicitly in the prompt.

**Sender identity over the ACP path — a dated finding, not a promise (2026-08-10).**
`buzz-acp` (upstream `block/buzz`, `crates/buzz-acp/src/queue.rs`, function
`format_event_block`, the `From:` line around lines 1105–1112) resolves the triggering
Nostr sender's Buzz profile display name — or NIP-05 handle, or the bare npub/hex as a
last resort — and writes it into the `[Event]` block's `From:` line, which is plain text
folded directly into the `session/prompt` content IronHermes receives. The actual request
builder (`crates/buzz-acp/src/acp.rs`, around lines 1938–1946) sends only `sessionId` and
`prompt: [{type, text}]` — there is no separate structured sender-identity field, and no
`_meta` carrying it either. That means sender identity is model-visible free text folded
into the prompt, not an authentication signal: a Nostr `kind:0` profile's display name is
self-reported by whoever holds that key, so a channel member can set it to anything,
including another member's name. IronHermes does not treat this text as proof of who is
asking, and this phase does not add any sender-based access control derived from it —
access control on the ACP path is Buzz's responsibility (see the trust boundary in
[Buzz as ACP client](#buzz-as-acp-client) below), not something this free-text field could
safely provide even if IronHermes tried to parse it.

## Session reload

`session/load` rehydrates a previously-recorded ACP session's conversation history from
IronHermes's state store (bounded to the most recent 200 messages, head-aligned to a user
turn) — this is what lets you close and reopen a Zed conversation, or restart Zed
entirely, and still see the earlier turns. Reloading rebinds the session to whatever
working directory the editor supplies on the load request.

## Debugging

**Stdout carries JSON-RPC protocol frames only. All logging goes to stderr.** This is a
hard invariant, not a configuration option — if a single non-protocol byte reaches stdout,
your editor's JSON-RPC parser breaks. Because of this, you cannot just watch stdout to see
what the agent is doing; redirect stderr instead:

```bash
/absolute/path/to/ironhermes acp 2>/tmp/ironhermes-acp.log
```

(In an editor context you can't easily redirect stderr yourself — if you need to see logs
while an editor drives the session, launch the exact command from `docs/ACP.md`'s
settings snippet manually in a terminal with `2>` redirection, feed it the same NDJSON
frames your editor would, and watch the log file grow.)

Control the log verbosity with the standard `RUST_LOG` environment variable, e.g.:

```bash
RUST_LOG=ironhermes_acp=debug /absolute/path/to/ironhermes acp 2>/tmp/ironhermes-acp.log
```

**Symptom of a stdout-corruption regression:** a frozen/stuck agent panel in your editor
that never progresses past "connecting," or a visible JSON parse error surfaced by the
editor itself. If you see either, capture stderr as above and check for anything that
looks like it might have written to stdout instead (a stray `println!`, a panic message
without a caught unwind, a dependency that logs to stdout by default).

## Zed live UAT — the acceptance bar

Zed is the one editor this phase's acceptance requires a human to actually drive a
conversation through — prompt, tool call, permission decision, and a file edit — since no
automated test can substitute for a real editor process speaking real stdio framing to a
real `ironhermes acp` process. See this phase's `36.8-06-PLAN.md` Task 3 for the exact
verification checklist.
