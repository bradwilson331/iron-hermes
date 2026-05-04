//! E-04 / S-11 / S-12 — watch-pattern rate limiter correctness under the
//! tokio paused-time harness.
//!
//! Semantics under test:
//!  - 8 fires per 10s window, 9th+ drop with `tracing::warn`.
//!  - 45s sustained at cap latches `AutoDisable` once; subsequent calls Drop.
//!  - Per-process scope: disabling `proc_a` does not silence `proc_b`.
//!  - After the window rolls, new matches fire again (while not disabled).

use ironhermes_exec::process_registry::*;
use std::time::Duration;
use tracing_test::traced_test;

#[tokio::test(start_paused = true)]
#[traced_test]
async fn twelve_matches_in_10s_fires_eight_drops_four_with_warn() {
    let mut reg = ProcessRegistry::new_for_session("t-rl");
    reg.insert_fake_running_minimal("proc_test1", 12345);

    let mut fire_count = 0;
    let mut drop_count = 0;
    for _ in 0..12 {
        match reg.rate_limit_check("proc_test1") {
            RateDecision::Fire => fire_count += 1,
            RateDecision::Drop => drop_count += 1,
            RateDecision::AutoDisable => panic!("must not disable in single 10s burst"),
        }
    }
    assert_eq!(fire_count, 8, "S-11: first 8 fire");
    assert_eq!(drop_count, 4, "S-11: next 4 drop");
    assert!(
        logs_contain("watch pattern backpressure"),
        "E-04 / Pitfall 7: every capped drop MUST emit tracing::warn"
    );
}

#[tokio::test(start_paused = true)]
async fn window_rolls_after_10s_and_allows_new_matches() {
    let mut reg = ProcessRegistry::new_for_session("t-rl2");
    reg.insert_fake_running_minimal("proc_a", 1);
    // Burn the window.
    for _ in 0..8 {
        let _ = reg.rate_limit_check("proc_a");
    }
    // Next one drops.
    assert_eq!(reg.rate_limit_check("proc_a"), RateDecision::Drop);

    // Advance past window.
    tokio::time::advance(Duration::from_secs(WATCH_WINDOW_SECONDS + 1)).await;
    // New window — must fire again.
    assert_eq!(reg.rate_limit_check("proc_a"), RateDecision::Fire);
}

#[tokio::test(start_paused = true)]
#[traced_test]
async fn sustained_overload_45s_disables_watch_for_that_process_only() {
    let mut reg = ProcessRegistry::new_for_session("t-rl3");
    reg.insert_fake_running_minimal("proc_hot", 99);
    reg.insert_fake_running_minimal("proc_cool", 100);

    // Sustained overload: every tick, fire 9 matches (8 Fire + 1 Drop),
    // then advance 1s. The 10-second window boundary will roll periodically
    // but the overload_since anchor persists across rolls when we stay at
    // cap, so after WATCH_OVERLOAD_KILL_SECONDS the AutoDisable fires.
    let mut disabled_seen = false;
    for _second in 0..=WATCH_OVERLOAD_KILL_SECONDS {
        // 9 calls → 8 Fire + 1 Drop within the current window.
        for _ in 0..9 {
            if matches!(reg.rate_limit_check("proc_hot"), RateDecision::AutoDisable) {
                disabled_seen = true;
            }
        }
        tokio::time::advance(Duration::from_secs(1)).await;
    }
    assert!(
        disabled_seen,
        "S-12: 45s sustained cap must trigger AutoDisable once"
    );
    let st = reg.watch_state_for("proc_hot").expect("state");
    assert!(
        st.disabled,
        "watch_state.disabled must latch true after AutoDisable"
    );

    // Other process's watch still fires (per-process scope).
    let d = reg.rate_limit_check("proc_cool");
    assert_eq!(
        d,
        RateDecision::Fire,
        "S-12: disable is per-process — proc_cool's watch must still fire"
    );

    assert!(
        logs_contain("watch overload disabled"),
        "E-04: AutoDisable must emit tracing::warn"
    );
}

#[tokio::test(start_paused = true)]
async fn already_disabled_returns_drop_every_time() {
    let mut reg = ProcessRegistry::new_for_session("t-rl4");
    reg.insert_fake_running_minimal("proc_x", 1);
    // Force sustained overload to reach disable.
    for _ in 0..=WATCH_OVERLOAD_KILL_SECONDS {
        for _ in 0..9 {
            let _ = reg.rate_limit_check("proc_x");
        }
        tokio::time::advance(Duration::from_secs(1)).await;
    }
    // Now the watch is disabled; every call must be Drop.
    for _ in 0..5 {
        assert_eq!(reg.rate_limit_check("proc_x"), RateDecision::Drop);
    }
}

#[tokio::test]
async fn unknown_id_returns_drop_silently() {
    let mut reg = ProcessRegistry::new_for_session("t-rl5");
    // No insert — the id is unknown.
    assert_eq!(reg.rate_limit_check("proc_nonexistent"), RateDecision::Drop);
}
