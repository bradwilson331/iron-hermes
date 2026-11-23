# Site deploys from chat

Ask the bot in Telegram to change a website, look at the result on a staging URL, and say "ship it" to publish it.
This guide covers the safety model, the one-time operator setup, the day-to-day loop and what to do when it goes
wrong. All names below (`demo-site`, `example-org/demo-site`, `*.example.workers.dev`) are placeholders.

## What it does

1. You ask for a change in a chat: "make the footer say 2027".
2. The agent works in a git clone of the site's repository, on a new branch, and commits the change.
3. It opens a pull request and deploys the branch to **staging** with `wrangler`. You tap `/approve` once for that
   deploy (approval 1 of 2).
4. It replies with the PR link and the staging URL.
5. You look at staging. When you are happy, you reply `ship it`.
6. The agent merges the PR, deploys the **merge commit** to production (you tap `/approve` again, approval 2 of 2),
   and reports the production URL.

One round trip is therefore **two `/approve` taps plus the words `ship it`**. The procedure the agent follows is the
bundled `site-deploy` skill (`skills/devops/site-deploy/SKILL.md`); it is visible by default and needs no MCP server.

## Safety model: what is enforced and what is instructed

Be precise about this, because the two kinds of control fail differently.

**Deterministic controls** — enforced by code or by GitHub, whatever the model does:

- **The production `wrangler deploy` approval.** Every `wrangler deploy` is parked by the wrangler mutation guardrail
  until you tap `/approve` in the chat where the turn runs. This is unchanged by this feature.
- **The base-branch rule.** The bot's GitHub token must not be able to push to the base branch or merge around a
  required pull request. See [Protect the base branch](#protect-the-base-branch). `ironhermes doctor` checks it.
- **GitHub's head-commit match.** The skill merges with
  `gh pr merge <n> --squash --match-head-commit <head_sha>`. If the PR head changed after staging, GitHub refuses the
  merge, so what you approved on staging is exactly what merges.
- **The per-turn deadline.** A turn that hangs is cut off by the gateway, not left running.

**Skill instructions** — followed by the model, not enforced by code:

- recognizing "ship it" (and only for a change that this chat staged);
- the deploy-record checks: freshness, and that the record's session is this chat's session;
- one change in flight per site;
- running the steps in order, one simple command per call, using the installed `wrangler` only.

If you do not trust a model to follow instructions for a given site, do not register it. In particular:

> **`git_connected` mode.** When a site's `deploy_mode` is `git_connected`, Cloudflare builds production from the
> merge, so **the merge is the production deploy**, and nothing approval-gates `gh pr merge` except your own
> `ship it`. In `wrangler` mode production sits behind the deterministic `/approve`. Prefer `wrangler` mode until a
> third approval is added for the merge.

## Prerequisites

- **Host install:** the agent uses the host's tools. You need `git`, `gh`, Node 22 or newer with `npm`, and the pinned
  `wrangler` installed globally (the version in the `# approved-wrangler:` header of `docker/dev-tools.Dockerfile`).
  `ironhermes doctor` reports which are missing.
- **Container:** use the `-dev-tools` image variant, which adds exactly those tools to the base image. See the
  [dev-tools section of docs/CONTAINER.md](CONTAINER.md#dev-tools-variant-site-deploys-from-chat) for building or
  pulling it and for the `podman exec --user ironhermes` form.

## Credentials

Three values, all single-quoted in the `.env` of your `IRONHERMES_HOME` (the volume's `.env` in a container):

```sh
CLOUDFLARE_API_TOKEN='...'
CLOUDFLARE_ACCOUNT_ID='...'
GITHUB_TOKEN='...'
```

- **Cloudflare API token:** scoped to Workers on **one account**. Start from the "Edit Cloudflare Workers" template.
  Add D1 edit only if the Worker binds a D1 database and a deploy reports a permission error. Do not use the global
  API key.
- **GitHub token:** a fine-grained token limited to **the one repository**, with **Contents: Read and write** and
  **Pull requests: Read and write**. Nothing else.
- **Account id:** not secret, but it must be present so wrangler targets the right account.

The agent's `terminal` tool clears its environment, so all three names must also be listed in the allowlist:

```yaml
terminal:
  terminal_env_allowlist: [CLOUDFLARE_API_TOKEN, CLOUDFLARE_ACCOUNT_ID, GITHUB_TOKEN]
```

Read the trade-off before you add them: **any command the agent runs can read an allowlisted value** (`env`, a
script, a build step). That is why the tokens are minimum-scope. Values go in `.env` only — never in `config.yaml`,
never in logs, never in a chat reply.

## Protect the base branch

The deterministic guarantee that nothing reaches the base branch without a pull request comes from GitHub, not from
the bot. Set it up once on the repository:

1. Create an **Active branch ruleset** for the base branch (usually `main`) that **requires a pull request**.
2. Do **not** list the token's owner (the account that issued the bot's token) as a **bypass** actor, and do not give
   the token's owner repository-admin rights that bypass rulesets.

`ironhermes doctor` prints a `base branch <b>: protected | NOT protected | unknown` line in its `Site deploy
preflight` section. It reads rulesets together with whether the token can bypass them, using the repo-scoped token. A
**classic** branch protection rule needs the Administration read permission to inspect, which the minimum-scope
token deliberately lacks, so a classic rule is reported `unknown`. Use a ruleset if you want a definite
`protected`.

## Register the site

Add an entry to `config.yaml` (see the commented `sites:` example in `cli-config.yaml.example`):

```yaml
sites:
  demo-site:
    repo: example-org/demo-site
    clone_path: workspace/demo-site
    base_branch: main
    deploy_mode: wrangler
    staging:
      deploy_args: ["--env", "staging"]
      url: "https://demo-site-staging.example.workers.dev"
    production:
      deploy_args: []
      url: "https://demo-site.example.workers.dev"
```

Staging must select its **own** target (`--env`, `-e` or `--name`) and its own URL, both different from production;
the registry refuses an entry where they match. The clone path must stay inside the workspace (no symlinks).

Inspect and validate with:

```sh
ironhermes sites list
ironhermes sites show demo-site          # add --json for the machine-readable form the skill reads
ironhermes doctor                        # the "Site deploy preflight" section
```

`sites show` prints the resolved clone directory, whether the clone exists, the deploy-record path, and the deploy
arguments and URLs. Exit code 1 means an unknown site; 2 means an invalid entry or a clone path that escapes the
workspace.

## Headless MCP authorization

Some Cloudflare MCP servers (builds, bindings, observability) use OAuth. On a machine with no browser:

```sh
ironhermes mcp connect <name> --no-browser
```

It prints an authorization link. Open it on any device, approve, and paste the address you are redirected to (it will
not load; copy it from the address bar) back into the terminal. Restart the gateway or container afterwards so it
loads the new token. Writes to the auth store are merged safely while other processes are running.

Inside the container, run it as the application user. A plain `exec` runs as root and can leave root-owned files on
the volume:

```sh
podman exec --user ironhermes -e HOME=/opt/data -it <container> ironhermes mcp connect <name> --no-browser
```

Alternatively, send `/mcp connect <name>` to the bot in a **private** Telegram chat. It replies with the link; paste
the redirected URL back into the same chat. The bot deletes your message and takes effect immediately, with no
restart. This path is limited to the owner, to direct messages, and to the baseline set of authorization servers.

## Header tokens instead of OAuth (research recommendation, D-16)

`mcp.cloudflare.com/mcp` accepts a Cloudflare API token as a bearer, so for that server you can skip OAuth:

```yaml
mcp_servers:
  cloudflare_docs_api:
    url: "https://mcp.cloudflare.com/mcp"
    headers:
      Authorization: "Bearer ${CLOUDFLARE_MCP_TOKEN}"
```

with `CLOUDFLARE_MCP_TOKEN='...'` in `.env`. The builds, bindings and observability servers are not documented to
accept a bearer token, so keep OAuth for them. An unset `${VAR}` in a header, url or auth field now fails the connect
with the variable's name rather than sending the literal text.

## Dependency installs and builds

Site repositories run code on the machine: dependency lifecycle scripts, the project's build script, and wrangler's
own build. The environment of the agent's terminal holds your tokens, so the skill limits what runs with them:

- Dependencies install with `npm ci --ignore-scripts`, so no dependency's install or postinstall script runs.
- The project's build or check script runs with **both** `CLOUDFLARE_API_TOKEN` and `GITHUB_TOKEN` removed from its
  environment.
- If a build fails because a dependency really needs its install script (a native module or a generated client), the
  agent stops and names the package. You allow a single `npm rebuild <package>` in the chat, and that also runs with
  the tokens removed.

What is left, plainly: `wrangler deploy` needs `CLOUDFLARE_API_TOKEN`, and it runs the wrangler config's custom
`build` command (if the config sets one) and bundles project code with `CLOUDFLARE_API_TOKEN` and `GITHUB_TOKEN` in
its environment. A malicious build step in the repository could therefore read them (accepted residual risk
T-54.8-84). So **register only repositories whose build you control**, and keep both tokens at the minimum scope
above.

## The deploy record

For each site the agent keeps one JSON record at `<workspace>/.site-deploys/<site>.json` (the path is the
`deploy_record` value from `sites show`). It holds the PR, the staged head commit, the merge commit, the staging and
production versions, the chat session that started the change, and a state:

| State | Meaning |
|---|---|
| `staged` | PR open, deployed to staging, waiting for `ship it`. |
| `merged` | PR merged; production not yet deployed (for example the production `/approve` timed out). A later `ship it` deploys the recorded merge commit without merging again. |
| `deployed` | Production deployed. Kept as evidence of what shipped. |

A site with a `staged` or `merged` record from a **different** chat is refused: finish or abandon the change in the
chat that started it. To abandon a stuck change, check the PR's state on GitHub (close it, or confirm it is merged),
then delete the record file.

## Upgrading

An image upgrade refreshes bundled skills on the volume: files you never touched are upgraded to the new version,
files you edited are kept, and skills that do not exist yet are added. This is how the `wrangler` skill (no longer
hidden behind an MCP server) and the `site-deploy` skill reach an existing volume. Both entrypoints use the same
seeding function as the installer.

## Troubleshooting

- **"not authenticated" / the agent cannot see a token:** the name is missing from `terminal.terminal_env_allowlist`,
  or from `.env`. `ironhermes doctor` lists which, by name only.
- **A command is killed after 30 seconds:** the skill passes a 600 second timeout on npm and wrangler calls. Make
  sure `tools.timeout_overrides.terminal` is not set below 300.
- **An approval times out:** approvals wait 120 seconds and then count as denied. Nothing is retried silently. Say
  `ship it` again and the agent resumes from the record's state.
- **The merge was refused because the head changed:** someone pushed to the PR after staging. The agent stops and
  offers to re-stage; ship the new head after looking at the new staging deploy.
- **A CLI-authorized MCP server is not picked up:** restart the gateway or container.
- **`base branch ...: NOT protected`:** add or fix the ruleset, as above, before relying on this flow.
