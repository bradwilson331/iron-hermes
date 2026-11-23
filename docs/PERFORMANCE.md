# Performance

Measured findings from profiling runs. The numbers below were taken with the opt-in `hotpath` profiler, which was removed on 2026-11-16 (it inflated async future sizes enough to overflow test-thread stacks under `--all-features`). To re-measure, add a profiler on a throwaway branch.

## Findings so far (2026-11-06, macOS, release build)

**1. Skill loading dominated start-up. Fixed.** `SkillRegistry` load sent each installed skill to the tirith security sidecar as a separate synchronous request, about 4 ms each. That was 1,160 skills, 1,042 of them from the shared `~/.agents/skills`.

| | Before | After (`a1ebf1fd7`) |
|---|---|---|
| Gateway `app_runtime::skill_registry_load` | 4.92 s | 0.32 s |
| Kanban worker `skill_registry_load` | 7.9–8.8 s (19–28% of each task) | not yet re-measured |

Start-up now runs only the local regex scan. The sidecar screens each skill when it is handed to the model (`skills::screen_skill_delivery`), on the exact content delivered. Community skills that the scan flags are blocked; builtin, official, trusted and self-created skills log a warning but are still delivered. The catalog's names and descriptions now get only the regex check. See `docs/SECURITY.md` for the screening policy.

**2. LLM latency now dominates turn time.** Representative runs:

| Run | Total | LLM | Tools | Notes |
|---|---|---|---|---|
| Kanban worker, news research | 26.9 s | 11.1 s (5 streaming calls, 2.2 s avg) | 10.6 s (one `web_extract` about 9.8 s) | About 5 s of pre-fix start-up |
| Kanban worker, long task | 161 s | 151 s (22 streaming calls, 6.9 s avg, P95 25.9 s) | 2.1 s (21 calls) | |
| Two cron jobs | 67 s | 44 s (6 non-streaming calls, 7.4 s avg) | 23 s | |
| Telegram turns with 3 delegated sub-agents | 2 turns at 131 s avg | 17 non-streaming child calls at 19 s avg, P95 79 s | | Children run in parallel |

Average CPU stayed near 0% throughout, and peak memory was 105–220 MB. The levers left are on the provider side: a faster model for sub-agents and cron jobs, smaller prompts and fewer round trips. Non-streaming calls (cron jobs and sub-agents) were consistently slower per call than streaming ones.

**3. Telegram is cheap.** `sendMessage` takes about 400 ms and `stream_consumer::flush` is sub-microsecond except for the occasional message edit.
