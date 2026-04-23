//! M-01 — cascade-cancel p95 < 500ms CI gate.
//!
//! Derived from S-01 / S-02 / S-04 class behavior: parent cancels → all child
//! tokens observe `is_cancelled()`. The bench runs 10 sampled cascades after
//! a warm-up run, sorts the samples, and gates on p95.
//!
//! Threat mitigation (T-21.7-10-02): wall-clock p95 is resilient to 1-2
//! outliers. Warm-up iteration discarded for cold-start variance. If flakes
//! observed in CI, raise to 20 samples or revisit the gate.

use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Single cascade: spawn `n_children` subagent child tokens, cancel parent,
/// poll until every child observes `is_cancelled()`. Returns wall-clock
/// elapsed.
async fn single_cascade_run(n_children: usize) -> Duration {
    let parent = CancellationToken::new();
    let children: Vec<CancellationToken> =
        (0..n_children).map(|_| parent.child_token()).collect();

    let start = std::time::Instant::now();
    parent.cancel();
    for _ in 0..200 {
        if children.iter().all(|c| c.is_cancelled()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    start.elapsed()
}

#[tokio::test]
async fn cascade_cancel_p95_under_500ms() {
    // Warm-up (discarded): amortizes first-allocation cost on cold runners.
    let _ = single_cascade_run(3).await;

    let mut samples: Vec<Duration> = Vec::with_capacity(10);
    for _ in 0..10 {
        samples.push(single_cascade_run(3).await);
    }
    samples.sort();

    // p95 of 10 samples = the 10th value (index 9 after sort).
    // `(len as f64 * 0.95) as usize` = 9 for len=10. Guard with `- 1`.
    let p95 = samples[((samples.len() as f64 * 0.95) as usize).saturating_sub(1)];
    let p50 = samples[samples.len() / 2];
    let min = samples.first().copied().unwrap_or_default();
    let max = samples.last().copied().unwrap_or_default();

    eprintln!(
        "M-01 cascade-cancel n=3 samples={} min={:?} p50={:?} p95={:?} max={:?}",
        samples.len(),
        min,
        p50,
        p95,
        max
    );

    assert!(
        p95 < Duration::from_millis(500),
        "M-01 CI gate: cascade-cancel p95 must be < 500ms. \
         p95={:?} samples={:?}",
        p95,
        samples
    );
}
