<!-- generated-by: gsd-doc-writer -->
# `ironhermes skills` — CLI reference

User-facing reference for the skill management CLI shipped in Phase 21.8. Manages remote skill download, install, list, update, and removal against [skills.sh](https://skills.sh) and any GitHub-hosted source.

Binary: `ironhermes` (built from `crates/ironhermes-cli`).

---

## Synopsis

```
ironhermes skills <command>

  install <identifier> [--yes] [--skip-audit]
  search <query> [--source <github|well-known|skills-sh>] [--format text|json] [--limit N]
  update [<name>]
  remove <name>                          # alias: uninstall
  list [--format text|json]
  trust add <repo>
  trust remove <repo>
  trust list [--format text|json]
```

Run `ironhermes skills --help` or `ironhermes skills <verb> --help` for the live clap-generated help.

---

## install — fetch and install a skill

```
ironhermes skills install <identifier> [--yes] [--skip-audit]
```

`<identifier>` accepts:
- `<owner>/<repo>` — pulls from skills.sh's blob API (3-hop pipeline: GitHub Trees → raw.githubusercontent → skills.sh `/api/download/<owner>/<repo>/<slug>`)
- `<owner>/<repo>/<skill-slug>` — same pipeline, scoped to a specific skill within a multi-skill repo (this session's example: `foo/bar/ascii-art`)
- `well-known:<name>` — the bundled well-known catalog
- `https://…` — direct GitHub source

**Flags:**
- `--yes` — reserved for future interactive prompts (no-op today)
- `--skip-audit` — bypass the pre-install audit endpoint (`add-skill.vercel.sh/audit`). Use in air-gapped environments or when audit is degraded.

**What it does:**
1. **Migrate** any existing 19.1 manifest at `$HERMES_HOME/skills/.hub/lock.json` to the 21.8 lock at `$HERMES_HOME/skills-lock.json` (idempotent).
2. **Fetch** the skill files via the configured source.
3. **Audit** the source against the audit endpoint (3 s timeout, soft-fail — install proceeds if audit is unavailable).
4. **Quarantine** the bundle in a temp directory; sanitize every file path against traversal (`..`, NUL, absolute, drive-prefix) and YAML-only frontmatter (`---js`/`---javascript` rejected).
5. **Scan** for trust-gate violations (community sources require explicit trust).
6. **Atomically rename** the quarantine directory into `$HERMES_HOME/skills/<name>/`.
7. **Observe** the post-install hash: `compute_folder_hash(install_dir)` is compared to the server's opaque `snapshotHash` — divergence is logged at warn level only (D-14: snapshotHash is opaque, not a strict integrity check).
8. **Write** a new entry into `$HERMES_HOME/skills-lock.json`.

**Stdout shape (D-21 5-line progress + D-23 restart message):**

```
Resolving skills.sh/<identifier>...
Discovering skills in <identifier>...
Downloading <N> bytes...
Scanning for threats...
Installed '<name>' [<trust>] — hash: <12-char-prefix>
Installed. Restart the agent or start a new session to use <name>.
```

Every line that quotes server data passes through a terminal-escape stripper before printing (defense against malicious frontmatter or repo names).

**Exit codes:** `0` on success, `1` on any error.

**Example:**
```
$ ironhermes skills install owner/repo/ascii-art
Resolving skills.sh/owner/repo/ascii-art...
Discovering skills in owner/repo/ascii-art...
Downloading 0 bytes...
Scanning for threats...
Installed 'ascii-art' [community] — hash: 4fe9ab1c00f3
Installed. Restart the agent or start a new session to use ascii-art.
```

---

## search — find skills across configured adapters

```
ironhermes skills search <query> [--source <github|well-known|skills-sh>] [--format text|json] [--limit N]
```

- `--source` restricts the search to one adapter; default is all configured.
- `--format text` (default) prints a list; `json` prints a structured array.
- `--limit` caps results (default 20).

---

## update — pull latest version of installed skills

```
ironhermes skills update [<name>]
```

- With `<name>` — updates that one skill.
- Without — updates every entry in `skills-lock.json`.

Drift detection: compares the bundle's D-13 folder hash against the lock entry's `computedHash`. If unchanged, returns `AlreadyInstalled` and exits without touching disk. If changed, replaces the install dir atomically.

The post-rename hash compare is also advisory (same as `install` Step 7) — server/client `snapshotHash` divergence is logged but does not abort.

---

## remove (alias: uninstall) — delete an installed skill

```
ironhermes skills remove <name>
ironhermes skills uninstall <name>     # alias retained for one release cycle (D-04)
```

Both verbs route to the same handler. Errors with `hub error (NotFound): skill '<name>' is not installed` if the skill isn't in `skills-lock.json`.

---

## list — show installed skills

```
ironhermes skills list [--format text|json]
```

Reads `$HERMES_HOME/skills-lock.json` (the 21.8 lock; the 19.1 `~/.hub/lock.json` is automatically migrated on first invocation).

**Text format:** `<name> [<trust>] (<source>) — hash: <12-char>`

**JSON format:** array of full lock entries:
```json
[
  {
    "name": "ascii-art",
    "source": "skills-sh",
    "identifier": "owner/repo/ascii-art",
    "trust_level": "community",
    "repoPath": "ascii-art/SKILL.md",
    "snapshotHash": "<server hash, opaque>",
    "computedHash": "<D-13 client folder hash>",
    "installedAt": "2026-04-22T19:05:00Z"
  }
]
```

Empty lock returns `[]` (json) / `No skills installed.` (text). Never panics on missing lock file.

---

## trust — manage the hub.trusted_repos allowlist

```
ironhermes skills trust add <repo>
ironhermes skills trust remove <repo>
ironhermes skills trust list [--format text|json]
```

`<repo>` is `<owner>/<repo>` shape. Persists to `~/.ironhermes/config.yaml` under `hub.trusted_repos`. Idempotent (adding a repo already on the list is a no-op; removing an absent repo is a no-op).

Community sources from skills.sh require their `<owner>/<repo>` to be on this allowlist — otherwise install fails with `ScanHit` from the trust gate.

---

## State files

| Path | Purpose | Owner |
|------|---------|-------|
| `$HERMES_HOME/skills-lock.json` | 21.8 install lock — camelCase, alphabetically sorted, sibling of `skills/` | written by every install/update/remove |
| `$HERMES_HOME/skills/<name>/` | Installed skill files (SKILL.md + helpers) | written by install; deleted by remove |
| `$HERMES_HOME/skills/.hub/lock.json` | **Legacy 19.1 manifest.** Auto-migrated to `skills-lock.json` on first 21.8 invocation; the legacy file is deleted after successful migration. | read-only after migration |
| `~/.ironhermes/config.yaml` | `hub.trusted_repos` allowlist | written by `trust add`/`trust remove` |

`HERMES_HOME` defaults to `~/.ironhermes`; override with the `HERMES_HOME` env var.

---

## Environment variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `HERMES_HOME` | Root for all skill state | `~/.ironhermes` |
| `SKILLS_DOWNLOAD_URL` | skills.sh blob API base | `https://skills.sh` |
| `SKILLS_AUDIT_URL` | Audit endpoint base | `https://add-skill.vercel.sh` |
| `GITHUB_API_BASE` | GitHub Trees API base (test/dev) | `https://api.github.com` |
| `GITHUB_RAW_CONTENT_BASE` | Raw content base (test/dev) | `https://raw.githubusercontent.com` |

`SkillsShBlobSource` enforces `https_only(true)` on its HTTP client by default. The TLS-only enforcement is **only** relaxed if at least one of the three override URLs above starts with `http://` — intended for test rigs and wiremock backends, not production. Production HTTPS is preserved unless you explicitly point at an `http://` mirror.

---

## Authentication

GitHub Trees API and raw content fetches reuse Phase 19.1's auth machinery (`GitHubAuth`):
- `HERMES_GITHUB_TOKEN` env var (highest priority)
- `GITHUB_TOKEN` env var
- `gh auth token` shell-out fallback
- Anonymous (rate-limited) if neither is configured

skills.sh blob API and audit endpoint require no auth.

---

## User-Agent

Every outbound request advertises:
```
ironhermes/<crate-version> (via openclaw)
```

Captured in integration test `user_agent_advertises_openclaw_ride`.

---

## Errors

`HubErrorKind` variants you may see:

| Variant | Meaning |
|---------|---------|
| `NotFound` | Identifier didn't resolve to a tree entry, blob, or installed skill |
| `PathTraversal` | A bundle file path escaped its install root (`..`, NUL, absolute, drive-prefix) — install aborted, zero filesystem state |
| `ScanHit` | Community-source trust-gate rejection (add the repo with `skills trust add`) |
| `Audit` | Audit endpoint plumbing error (rare — most audit failures soft-fail to None) |
| `ShaMismatch` | **Local drift** — the disk folder hash differs from `skills-lock.json::computedHash`. Not raised by install/update post-rename anymore (D-14 advisory); reserved for future `list --verify` and strict modes. |

All error messages route through the same terminal-escape stripper as success output.

---

## Migration from Phase 19.1

If you have a legacy `$HERMES_HOME/skills/.hub/lock.json`, the first invocation of any `skills` subcommand triggers `migrate_from_hub_manifest()`:

1. Detects `.hub/lock.json` exists and `skills-lock.json` is empty/missing.
2. Reads each `ManifestEntry`, maps it to a `SkillLockEntry`:
   - `content_hash → computedHash`
   - `snapshotHash` initialized empty (backfilled on next `update`)
   - `repoPath` derived from the first file path
3. Atomically writes `skills-lock.json` (sorted alphabetically).
4. Deletes the old `.hub/lock.json` and the now-empty `.hub/` directory.

Idempotent: a second invocation observes the already-migrated lock and skips. Safe under concurrent invocations (atomic tmp+rename + guard checks).

---

## Source pointers

| Concern | File |
|---------|------|
| CLI surface (clap enums, dispatch) | `crates/ironhermes-cli/src/skills_cmd.rs` |
| Three-hop blob adapter | `crates/ironhermes-hub/src/blob.rs` |
| Lock schema + folder hash | `crates/ironhermes-hub/src/lock.rs` |
| Audit client | `crates/ironhermes-hub/src/audit.rs` |
| Install pipeline (8 steps) | `crates/ironhermes-hub/src/installer.rs` |
| Sanitize primitives | `crates/ironhermes-hub/src/sanitize.rs` |
| Error taxonomy | `crates/ironhermes-hub/src/error.rs` |
| Phase artifacts | `.planning/phases/21.8-skill-remote-download-and-install-from-skills-sh/` |

---

## Related decisions (D-numbers)

The implementation locks 25 design decisions (D-01..D-25). The full table with test mappings lives in `.planning/phases/21.8-skill-remote-download-and-install-from-skills-sh/21.8-VALIDATION.md`. Most user-visible:

- **D-04** — `remove` is the canonical verb; `uninstall` is a clap alias for one release cycle
- **D-13** — folder hash uses no separators (matches reference TS `local-lock.ts`)
- **D-14** — `snapshotHash` is opaque server data; never recomputed client-side; divergence is advisory
- **D-19** — pre-install audit soft-fails on every error path; `--skip-audit` bypasses
- **D-21** — install emits exactly 5 progress lines on success
- **D-23** — successful install ends with a restart prompt
