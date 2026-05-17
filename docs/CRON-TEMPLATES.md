# Cron Automation Templates

Copy-paste recipes for common automation patterns. Each template uses IronHermes's built-in cron scheduler for time-based triggers and the webhook platform for event-driven triggers.

Every template works with any model — not locked to a single provider. Swap `--model` or set `model:` in `~/.ironhermes/config.yaml` to use Anthropic, OpenAI, Gemini, Groq, or local Ollama.

---

## Three Trigger Types

| Trigger | How | Tool |
|---|---|---|
| Schedule | Runs on a cadence (hourly, nightly, weekly) | `ironhermes cron create` or `/cron` slash command |
| GitHub Event | Fires on PR opens, pushes, issues, CI results | Webhook platform (`ironhermes webhook subscribe`) |
| API Call | External service POSTs JSON to your endpoint | Webhook platform (config.yaml routes or `ironhermes webhook subscribe`) |

All three support delivery to Telegram, Discord, Slack, SMS, email, GitHub comments, or local files.

---

## Development Workflow

### Nightly Backlog Triage

Label, prioritize, and summarize new issues every night. Delivers a digest to your team channel.

**Trigger:** Schedule (nightly)

```bash
ironhermes cron create "0 2 * * *" \
  "You are a project manager triaging the bradwilson331/ironhermes GitHub repo.

1. Run: gh issue list --repo bradwilson331/ironhermes --state open --json number,title,labels,author,createdAt --limit 30
2. Identify issues opened in the last 24 hours
3. For each new issue:
   - Suggest a priority label (P0-critical, P1-high, P2-medium, P3-low)
   - Suggest a category label (bug, feature, docs, security)
   - Write a one-line triage note
4. Summarize: total open issues, new today, breakdown by priority

Format as a clean digest. If no new issues, respond with [SILENT]." \
  --name "Nightly backlog triage" \
  --deliver telegram
```

---

### Automatic PR Code Review

Review every pull request automatically when it's opened. Posts a review comment directly on the PR.

**Trigger:** GitHub webhook

**Option A — Dynamic subscription (CLI):**

```bash
ironhermes webhook subscribe github-pr-review \
  --events "pull_request" \
  --prompt "Review this pull request:
Repository: {repository.full_name}
PR #{pull_request.number}: {pull_request.title}
Author: {pull_request.user.login}
Action: {action}
Diff URL: {pull_request.diff_url}

Fetch the diff with: curl -sL {pull_request.diff_url}

Review for:
- Security issues (injection, auth bypass, secrets in code)
- Performance concerns (N+1 queries, unbounded loops, memory leaks)
- Code quality (naming, duplication, error handling)
- Missing tests for new behavior

Post a concise review. If the PR is a trivial docs/typo change, say so briefly." \
  --skill github-code-review \
  --deliver github_comment
```

**Option B — Static route (`~/.ironhermes/config.yaml`):**

```yaml
platforms:
  webhook:
    enabled: true
    extra:
      port: 8644
      secret: "your-global-secret"
      routes:
        github-pr-review:
          events: ["pull_request"]
          secret: "github-webhook-secret"
          prompt: |
            Review PR #{pull_request.number}: {pull_request.title}
            Repository: {repository.full_name}
            Author: {pull_request.user.login}
            Diff URL: {pull_request.diff_url}
            Review for security, performance, and code quality.
          skills: ["github-code-review"]
          deliver: "github_comment"
          deliver_extra:
            repo: "{repository.full_name}"
            pr_number: "{pull_request.number}"
```

In GitHub: Settings → Webhooks → Add webhook → Payload URL: `http://your-server:8644/webhooks/github-pr-review`, Content type: `application/json`, Events: Pull requests.

---

### Docs Drift Detection

Weekly scan of merged PRs to find API changes that need documentation updates.

**Trigger:** Schedule (weekly)

```bash
ironhermes cron create "0 9 * * 1" \
  "Scan the bradwilson331/ironhermes repo for documentation drift.

1. Run: gh pr list --repo bradwilson331/ironhermes --state merged --json number,title,files,mergedAt --limit 30
2. Filter to PRs merged in the last 7 days
3. For each merged PR, check if it modified:
   - Cron schemas (crates/ironhermes-cron/src/) — may need docs/CRON-TEMPLATES.md or docs/crates.md update
   - CLI commands (crates/ironhermes-cli/) — may need docs/GETTING-STARTED.md update
   - Config options — may need docs/CONFIGURATION.md update
   - Environment variables — may need docs/CONFIGURATION.md update
4. Cross-reference: for each code change, check if the corresponding docs page was also updated in the same PR

Report any gaps where code changed but docs didn't. If everything is in sync, respond with [SILENT]." \
  --name "Docs drift detection" \
  --deliver telegram
```

---

### Dependency Security Audit

Daily scan for known vulnerabilities in project dependencies.

**Trigger:** Schedule (daily)

```bash
ironhermes cron create "0 6 * * *" \
  "Run a dependency security audit on the ironhermes project.

1. cd ~/code/ironhermes && cargo audit 2>&1
2. Check for any CVEs with CVSS score >= 7.0
3. Run: cargo outdated --depth 1 2>/dev/null | head -30

If vulnerabilities found:
- List each one with crate name, version, CVE ID, severity
- Check if an upgrade is available
- Note if it's a direct dependency or transitive

If no vulnerabilities, respond with [SILENT]." \
  --name "Dependency audit" \
  --deliver telegram
```

---

## DevOps & Monitoring

### Deploy Verification

Trigger smoke tests after every deployment. Your CI/CD pipeline POSTs to the webhook when a deploy completes.

**Trigger:** API call (webhook)

```bash
ironhermes webhook subscribe deploy-verify \
  --events "deployment" \
  --prompt "A deployment just completed:
Service: {service}
Environment: {environment}
Version: {version}
Deployed by: {deployer}

Run these verification steps:
1. Check if the service is responding: curl -s -o /dev/null -w '%{http_code}' {health_url}
2. Search recent logs for errors: check the deployment payload for any error indicators
3. Verify the version matches: curl -s {health_url}/version

Report: deployment status (healthy/degraded/failed), response time, any errors found.
If healthy, keep it brief. If degraded or failed, provide detailed diagnostics." \
  --deliver telegram
```

Trigger from CI/CD:

```bash
curl -X POST http://your-server:8644/webhooks/deploy-verify \
  -H "Content-Type: application/json" \
  -H "X-Hub-Signature-256: sha256=$(echo -n '{"service":"ironhermes","environment":"prod","version":"0.9.0","deployer":"ci","health_url":"https://api.example.com/health"}' | openssl dgst -sha256 -hmac 'your-secret' | cut -d' ' -f2)" \
  -d '{"service":"ironhermes","environment":"prod","version":"0.9.0","deployer":"ci","health_url":"https://api.example.com/health"}'
```

---

### Uptime Monitor

Check endpoints every 30 minutes. Only notify when something is down.

**Trigger:** Schedule (every 30 min)

```python
# ~/.ironhermes/scripts/check-uptime.py
import urllib.request, json, time

ENDPOINTS = [
    {"name": "API", "url": "https://api.example.com/health"},
    {"name": "Web", "url": "https://www.example.com"},
]

results = []
for ep in ENDPOINTS:
    try:
        start = time.time()
        req = urllib.request.Request(ep["url"], headers={"User-Agent": "IronHermes-Monitor/1.0"})
        resp = urllib.request.urlopen(req, timeout=10)
        elapsed = round((time.time() - start) * 1000)
        results.append({"name": ep["name"], "status": resp.getcode(), "ms": elapsed})
    except Exception as e:
        results.append({"name": ep["name"], "status": "DOWN", "error": str(e)})

down = [r for r in results if r.get("status") == "DOWN" or (isinstance(r.get("status"), int) and r["status"] >= 500)]
if down:
    print("OUTAGE DETECTED")
    for r in down:
        print(f"  {r['name']}: {r.get('error', f'HTTP {r[\"status\"]}')} ")
    print(f"\nAll results: {json.dumps(results, indent=2)}")
else:
    print("NO_ISSUES")
```

```bash
ironhermes cron create "every 30m" \
  "If the script reports OUTAGE DETECTED, summarize which services are down and suggest likely causes. If NO_ISSUES, respond with [SILENT]." \
  --script ~/.ironhermes/scripts/check-uptime.py \
  --name "Uptime monitor" \
  --deliver telegram
```

---

## Parity Project Automation Templates

These templates are specific to the IronHermes parity work — tracking feature coverage between the Python `hermes-agent` reference implementation and the Rust `ironhermes-cron` crate.

### Hermes-Agent Drift Scanner

Watches `NousResearch/hermes-agent` for changes to cron-related code and flags anything that may need to be ported to `ironhermes-cron`.

**Trigger:** Schedule (daily)

```bash
ironhermes cron create "0 7 * * *" \
  "Check the NousResearch/hermes-agent repository for cron-related changes in the last 24 hours.

1. Run: gh pr list --repo NousResearch/hermes-agent --state merged --json number,title,files,mergedAt --limit 20
2. Filter to PRs merged in the last 24 hours
3. For each merged PR, check if it touched:
   - cron/scheduler.py
   - cron/jobs.py
   - tools/cronjob_tools.py
   - Any file whose path contains 'cron', 'delivery', 'schedule', or 'scanner'
4. For any matching PR:
   - Summarize what changed
   - Cross-reference against crates/ironhermes-cron/PARITY.md to see if the feature is marked ✅, ⚠️, or ❌
   - Note whether this widens or narrows the parity gap

If no cron-related changes, respond with [SILENT].
Otherwise, format as: [PARITY DRIFT] followed by the findings." \
  --name "Hermes-agent drift scanner" \
  --deliver telegram
```

---

### Weekly Parity Gap Report

Every Monday, produce a structured summary of what remains unimplemented in `ironhermes-cron` compared to the Python reference.

**Trigger:** Schedule (weekly)

```bash
ironhermes cron create "0 9 * * 1" \
  "Generate a parity gap report for the ironhermes-cron crate.

1. Read: cat ~/code/ironhermes/crates/ironhermes-cron/PARITY.md
2. Count and list all rows marked ❌ (missing in Rust) and ⚠️ (partial)
3. For each ❌ item: name the missing feature, its Python location, and estimated complexity (simple/medium/complex)
4. For each ⚠️ item: describe the behavioral difference
5. Produce a summary table: total features, ✅ covered, ⚠️ partial, ❌ missing, parity %

Format as a clean weekly status. Aim for actionable output — identify the top 3 gaps worth closing next sprint." \
  --name "Weekly parity gap report" \
  --deliver telegram
```

---

### Parity CI Failure Watcher

When a CI check fails on the IronHermes repo, automatically analyze the failure and cross-reference against known parity gaps.

**Trigger:** GitHub webhook

```bash
ironhermes webhook subscribe parity-ci-watcher \
  --events "check_run" \
  --prompt "CI check result for ironhermes:
Repository: {repository.full_name}
Check: {check_run.name}
Status: {check_run.conclusion}
PR: #{check_run.pull_requests[0].number}
Details: {check_run.details_url}

If conclusion is 'failure' and the PR touches crates/ironhermes-cron/:
1. Describe the likely failure cause from the check name and PR context
2. Read crates/ironhermes-cron/PARITY.md and check if the failure relates to a known ⚠️ or ❌ parity gap
3. Suggest a fix or flag for manual review

If conclusion is 'success' or the PR does not touch ironhermes-cron, respond with [SILENT]." \
  --deliver github_comment
```

---

### Nightly Parity Test Runner

Run the cron crate's test suite every night and report failures.

**Trigger:** Schedule (nightly)

```bash
ironhermes cron create "0 3 * * *" \
  "Run the ironhermes-cron test suite and report results.

1. cd ~/code/ironhermes && cargo test -p ironhermes-cron 2>&1
2. Count: total tests, passed, failed, ignored
3. For any failing test: show the test name and failure message

If all tests pass, respond with [SILENT].
If any test fails, report: [CRON TEST FAILURE] followed by the failing test names and a one-line diagnosis for each." \
  --name "Nightly cron tests" \
  --deliver telegram
```

---

### Script-Only: Memory Watchdog (No-Agent Mode)

A lightweight watchdog that fires only when RAM crosses a threshold — no LLM call per tick.

```bash
# ~/.ironhermes/scripts/memory-watchdog.sh
#!/usr/bin/env bash
RAM_PCT=$(free | awk '/^Mem:/ {printf "%d", $3 * 100 / $2}')
if [ "$RAM_PCT" -ge 85 ]; then
  echo "RAM ${RAM_PCT}% on $(hostname)"
fi
# Empty stdout = silent tick; no message sent.
```

```bash
ironhermes cron create "every 5m" \
  --no-agent \
  --script ~/.ironhermes/scripts/memory-watchdog.sh \
  --deliver telegram \
  --name "memory-watchdog"
```

No prompt, no model call. The scheduler runs the script; non-empty stdout goes to Telegram.

---

## Quick Reference

### Cron Schedule Syntax

| Expression | Meaning |
|---|---|
| `every 30m` | Every 30 minutes |
| `every 2h` | Every 2 hours |
| `0 2 * * *` | Daily at 2:00 AM |
| `0 9 * * 1` | Every Monday at 9:00 AM |
| `0 9 * * 1-5` | Weekdays at 9:00 AM |
| `0 3 * * 0` | Every Sunday at 3:00 AM |
| `0 */6 * * *` | Every 6 hours |

### Delivery Targets

| Target | Flag | Notes |
|---|---|---|
| Same chat | `--deliver origin` | Default — delivers to where the job was created |
| Local file | `--deliver local` | Saves output, no notification |
| Telegram | `--deliver telegram` | Home channel; set `TELEGRAM_HOME_CHANNEL` |
| Discord | `--deliver discord` | Home channel; set `DISCORD_HOME_CHANNEL` |
| Slack | `--deliver slack` | Home channel; set `SLACK_HOME_CHANNEL` |
| Specific Telegram chat | `--deliver telegram:-100123` | Target chat ID |
| Telegram topic thread | `--deliver telegram:-100123:456` | Telegram forum topic |

### Webhook Template Variables

| Variable | Description |
|---|---|
| `{pull_request.title}` | PR title |
| `{issue.number}` | Issue number |
| `{repository.full_name}` | `owner/repo` |
| `{action}` | Event action (`opened`, `closed`, etc.) |
| `{__raw__}` | Full JSON payload (truncated at 4000 chars) |
| `{sender.login}` | GitHub user who triggered the event |

### The `[SILENT]` Pattern

When a cron job's response contains `[SILENT]`, delivery is suppressed. Use this to avoid notification spam on quiet runs:

```
If nothing noteworthy happened, respond with [SILENT].
```

### Job Lifecycle Commands

```bash
ironhermes cron list                          # list all active jobs
ironhermes cron run <job_id>                  # trigger immediately (for testing)
ironhermes cron pause <job_id>                # pause without deleting
ironhermes cron resume <job_id>               # resume paused job
ironhermes cron edit <job_id> --schedule "every 4h"   # change cadence
ironhermes cron remove <job_id>               # delete permanently
```

### Per-Job Model Override

```bash
ironhermes cron create "0 8 * * *" "..." \
  --model claude-opus-4-7 \
  --name "Heavy analysis"
```

The `--model`, `--provider`, and `--base-url` flags override the global config for a single job. Useful for routing expensive analysis jobs to a capable model while keeping routine watchdogs on a cheaper one.

---

> Scripts must live in `~/.ironhermes/scripts/`. Absolute paths and `../` traversal are rejected at job-creation and run time. `.sh`/`.bash` files run under `/bin/bash`; all other extensions run under the current Python interpreter.
