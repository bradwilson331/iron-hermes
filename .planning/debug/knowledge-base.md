# GSD Debug Knowledge Base

Resolved debug sessions. Used by `gsd-debugger` to surface known-pattern hypotheses at the start of new investigations.

---

## gateway-jobstore-stale — Gateway tick task holds stale JobStore; CLI-added jobs invisible until restart
- **Date:** 2026-04-16
- **Error patterns:** stale JobStore, jobs.json, cron tick, CLI jobs not seen, burst fire on restart, reload, get_due_jobs, silent staleness
- **Root cause:** Gateway loaded JobStore once at startup and the tick task held a stale in-memory reference. CLI writes to jobs.json (separate process, separate JobStore instance) were invisible to the running gateway because no reload ever happened. Additionally, on restart, jobs whose next_run_at drifted past while the gateway was down would burst-fire on the first tick.
- **Fix:** (1) tick.rs::run_tick_check() calls store_guard.reload()? on every tick (under tick-lock + store-mutex), picking up all CLI disk writes before evaluating due jobs. (2) runner.rs::fast_forward_backlog() runs once before the first tick (first_tick guard), reloads jobs.json, and advances all past-due Scheduled+enabled jobs to their next future cadence — preventing burst-fire on restart.
- **Files changed:** crates/ironhermes-cron/src/tick.rs, crates/ironhermes-cron/src/store.rs, crates/ironhermes-gateway/src/runner.rs
---

