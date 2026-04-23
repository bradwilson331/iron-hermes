//! E-05 / S-13 — 10k-loop concurrent-consume stress test proving
//! that parent + 4 subagent tasks hammering the SAME BudgetHandle
//! sum to exactly max_iterations without double-decrement.

use ironhermes_agent::budget::BudgetHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_consume_sums_to_max_iterations() {
    const MAX: usize = 10_000;
    const TASKS: usize = 5; // parent + 4 subagents

    let budget = BudgetHandle::new(MAX);
    let total_consumed = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(TASKS);
    for _ in 0..TASKS {
        let b = budget.clone();
        let t = total_consumed.clone();
        handles.push(tokio::spawn(async move {
            // Hammer until exhausted.
            while b.consume().is_some() {
                t.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await; // encourage interleaving
            }
        }));
    }

    for h in handles {
        h.await.expect("task panic");
    }

    assert_eq!(
        total_consumed.load(Ordering::SeqCst),
        MAX,
        "E-05 / S-13: sum of successful consumes must equal max_iterations exactly"
    );
    assert_eq!(budget.remaining(), 0, "counter must land at 0");
    assert!(budget.consume().is_none(), "further consumes stay None");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reader_never_observes_over_max_decrements() {
    const MAX: usize = 1_000;
    let budget = BudgetHandle::new(MAX);

    // Writer: drain the budget
    let writer = {
        let b = budget.clone();
        tokio::spawn(async move {
            while b.consume().is_some() {
                tokio::task::yield_now().await;
            }
        })
    };

    // Reader: repeatedly sample used(); assert it never exceeds MAX.
    let reader = {
        let b = budget.clone();
        tokio::spawn(async move {
            let mut max_seen = 0;
            for _ in 0..MAX * 2 {
                let u = b.used();
                assert!(u <= MAX, "E-05: reader observed used={} > max={}", u, MAX);
                max_seen = max_seen.max(u);
                tokio::task::yield_now().await;
            }
            max_seen
        })
    };

    writer.await.expect("writer");
    let max_seen = reader.await.expect("reader");
    assert_eq!(
        max_seen, MAX,
        "reader should eventually observe full exhaustion"
    );
}
