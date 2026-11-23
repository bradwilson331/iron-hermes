//! Live-log writer contract: one line format, and a cross-process lock so every returned
//! `#L<n>` is the caller's own line (T-58.2-58).

use std::fs::OpenOptions;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use chrono::NaiveDate;
use ironhermes_tier2::live::{append_lines, claim_line};

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 12).unwrap()
}

#[test]
fn live_claim_line_is_one_line() {
    assert_eq!(
        claim_line("claim:0123456789ab", "a\n  b\tc"),
        "- [claim:0123456789ab] a b c"
    );
}

#[test]
fn live_append_waits_for_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let held = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join(".append.lock"))
        .unwrap();
    held.lock().unwrap();

    let (tx, rx) = mpsc::channel();
    let d2 = dir.clone();
    let t = std::thread::spawn(move || {
        tx.send(append_lines(&d2, date(), &["- one".to_string()]))
            .unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    drop(held);
    let got = rx.recv_timeout(Duration::from_secs(5)).expect("returns");
    assert_eq!(got.unwrap(), vec![2]);
    t.join().unwrap();
}

#[test]
fn live_append_concurrent_writers_cite_their_own_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|t| {
            let (dir, barrier) = (dir.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                (0..50)
                    .map(|i| {
                        let text = format!("w{t}-{i}");
                        let n = append_lines(&dir, date(), std::slice::from_ref(&text)).unwrap()[0];
                        (n, text)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let all: Vec<(usize, String)> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();

    let log = std::fs::read_to_string(dir.join(format!("{}.md", date()))).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 101);
    for (n, text) in &all {
        assert_eq!(lines[n - 1], text, "line {n} is not the writer's own");
    }
    let mut numbers: Vec<usize> = all.iter().map(|(n, _)| *n).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 100);
}
