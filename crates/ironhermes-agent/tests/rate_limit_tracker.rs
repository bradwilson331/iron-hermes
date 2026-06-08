//! Phase 36.2 Plan 06: RateLimitTracker behavior tests.
//!
//! Covers behaviors 1-13 from 36.2-06-PLAN.md:
//!   1. hash_api_key determinism
//!   2. hash_api_key distinctness
//!   3. Anthropic header parser
//!   4. x-ratelimit header parser
//!   5. Provider routing (anthropic vs x-ratelimit)
//!   6. Key isolation (provider, key_hash, model triple)
//!   7. Broadcast emission on record_headers
//!   8. Reactive429 path
//!   9. Severity Warning at <10%
//!  10. Severity Critical at 0% + reset >= 60s
//!  11. Severity NOT Critical at 0% + reset < 60s (Warning)
//!  12. Broadcast capacity 64 — no panic on overflow
//!  13. Debug output bounded (entry count only)

use ironhermes_agent::{
    RateLimitEvent, RateLimitKey, RateLimitSeverity, RateLimitSource, RateLimitTracker,
    hash_api_key,
};
use std::time::{Duration, SystemTime};

// ── Tests 1-2: hash_api_key ─────────────────────────────────────────────────

#[test]
fn hash_api_key_is_deterministic() {
    let a = hash_api_key("test-key-123");
    let b = hash_api_key("test-key-123");
    assert_eq!(a, b);
}

#[test]
fn hash_api_key_distinct_inputs_produce_distinct_hashes() {
    // Using non-vendor-prefix strings to avoid tripping the "no cleartext key"
    // invariant which scans for "sk-" literals.
    let a = hash_api_key("test-key-AAA");
    let b = hash_api_key("test-key-BBB");
    assert_ne!(a, b);
}

#[test]
fn hash_api_key_returns_16_bytes() {
    let h = hash_api_key("anything");
    assert_eq!(h.len(), 16);
}

// ── Test 3: Anthropic header parser ──────────────────────────────────────────

#[tokio::test]
async fn anthropic_header_parser_extracts_state() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-1");
    let headers = vec![
        (
            "anthropic-ratelimit-requests-remaining".to_string(),
            "5".to_string(),
        ),
        (
            "anthropic-ratelimit-requests-limit".to_string(),
            "60".to_string(),
        ),
        (
            "anthropic-ratelimit-requests-reset".to_string(),
            "2027-05-25T15:00:00Z".to_string(),
        ),
    ];
    tracker.record_headers("anthropic", key_hash, "claude-opus-4-7", headers);

    let key = RateLimitKey {
        provider: "anthropic".to_string(),
        api_key_hash: key_hash,
        model: "claude-opus-4-7".to_string(),
    };
    let snap = tracker.snapshot(&key).expect("snapshot exists");
    assert_eq!(snap.remaining_requests, Some(5));
    assert_eq!(snap.limit_requests, Some(60));
    assert!(snap.reset_at.is_some());

    // Event was emitted.
    let event = rx.try_recv().expect("event emitted");
    assert_eq!(event.provider, "anthropic");
    assert_eq!(event.remaining_requests, Some(5));
}

// ── Test 4: x-ratelimit parser ───────────────────────────────────────────────

#[tokio::test]
async fn x_ratelimit_header_parser_extracts_state() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-2");
    let before = SystemTime::now();
    let headers = vec![
        (
            "x-ratelimit-remaining-requests".to_string(),
            "10".to_string(),
        ),
        ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
        ("x-ratelimit-reset-requests".to_string(), "45.5".to_string()),
    ];
    tracker.record_headers("openrouter", key_hash, "gpt-4o", headers);
    let after = SystemTime::now();

    let key = RateLimitKey {
        provider: "openrouter".to_string(),
        api_key_hash: key_hash,
        model: "gpt-4o".to_string(),
    };
    let snap = tracker.snapshot(&key).expect("snapshot exists");
    assert_eq!(snap.remaining_requests, Some(10));
    assert_eq!(snap.limit_requests, Some(100));

    let reset = snap.reset_at.expect("reset_at present");
    let lower_bound = before + Duration::from_secs(40);
    let upper_bound = after + Duration::from_secs(50);
    assert!(
        reset >= lower_bound && reset <= upper_bound,
        "reset_at must be ~now+45.5s (got {:?}; expected between {:?} and {:?})",
        reset,
        lower_bound,
        upper_bound,
    );

    let _ = rx.try_recv().expect("event emitted");
}

// ── Test 5: Provider routing ─────────────────────────────────────────────────

#[tokio::test]
async fn provider_routing_anthropic_ignores_x_ratelimit_headers() {
    let (tracker, _rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-3");
    // x-ratelimit headers under anthropic provider name → no state update.
    let headers = vec![(
        "x-ratelimit-remaining-requests".to_string(),
        "10".to_string(),
    )];
    tracker.record_headers("anthropic", key_hash, "claude-opus-4-7", headers);
    let key = RateLimitKey {
        provider: "anthropic".to_string(),
        api_key_hash: key_hash,
        model: "claude-opus-4-7".to_string(),
    };
    assert!(tracker.snapshot(&key).is_none());
}

#[tokio::test]
async fn provider_routing_openrouter_ignores_anthropic_headers() {
    let (tracker, _rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-4");
    // anthropic headers under openrouter provider name → no state update.
    let headers = vec![(
        "anthropic-ratelimit-requests-remaining".to_string(),
        "5".to_string(),
    )];
    tracker.record_headers("openrouter", key_hash, "gpt-4o", headers);
    let key = RateLimitKey {
        provider: "openrouter".to_string(),
        api_key_hash: key_hash,
        model: "gpt-4o".to_string(),
    };
    assert!(tracker.snapshot(&key).is_none());
}

// ── Test 6: Key isolation ────────────────────────────────────────────────────

#[tokio::test]
async fn key_isolation_three_axis() {
    let (tracker, _rx) = RateLimitTracker::new();
    let key_a = hash_api_key("test-key-A");
    let key_b = hash_api_key("test-key-B");
    let headers = vec![
        (
            "x-ratelimit-remaining-requests".to_string(),
            "1".to_string(),
        ),
        ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
    ];

    // Same provider+model, different api_key_hash → 2 entries
    tracker.record_headers("openrouter", key_a, "gpt-4o", headers.clone());
    tracker.record_headers("openrouter", key_b, "gpt-4o", headers.clone());

    let entry_a = RateLimitKey {
        provider: "openrouter".to_string(),
        api_key_hash: key_a,
        model: "gpt-4o".to_string(),
    };
    let entry_b = RateLimitKey {
        provider: "openrouter".to_string(),
        api_key_hash: key_b,
        model: "gpt-4o".to_string(),
    };
    assert!(tracker.snapshot(&entry_a).is_some());
    assert!(tracker.snapshot(&entry_b).is_some());

    // Same provider+key, different model → distinct entry
    tracker.record_headers("openrouter", key_a, "gpt-4o-mini", headers.clone());
    let entry_a_mini = RateLimitKey {
        provider: "openrouter".to_string(),
        api_key_hash: key_a,
        model: "gpt-4o-mini".to_string(),
    };
    assert!(tracker.snapshot(&entry_a_mini).is_some());

    // Same all three → updates the existing entry, no new entry
    tracker.record_headers("openrouter", key_a, "gpt-4o", headers.clone());
    // (assert snapshot still has remaining=1 — single entry, replaced not duplicated)
    let snap = tracker.snapshot(&entry_a).unwrap();
    assert_eq!(snap.remaining_requests, Some(1));
}

// ── Test 7: Broadcast emission ───────────────────────────────────────────────

#[tokio::test]
async fn record_headers_emits_event_to_subscriber() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-5");
    let headers = vec![
        (
            "x-ratelimit-remaining-requests".to_string(),
            "50".to_string(),
        ),
        ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
    ];
    tracker.record_headers("openrouter", key_hash, "gpt-4o", headers);

    let event: RateLimitEvent = rx.try_recv().expect("event was emitted");
    assert_eq!(event.provider, "openrouter");
    assert_eq!(event.model, "gpt-4o");
    assert_eq!(event.key_hash, key_hash);
    assert_eq!(event.remaining_requests, Some(50));
    assert_eq!(event.source, RateLimitSource::Headers);
}

// ── Test 8: Reactive429 path ─────────────────────────────────────────────────

#[tokio::test]
async fn reactive_429_emits_critical_event() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-6");
    tracker.record_429(
        "ollama",
        key_hash,
        "llama-3",
        Some(Duration::from_secs(120)),
    );

    let event: RateLimitEvent = rx.try_recv().expect("event was emitted");
    assert_eq!(event.provider, "ollama");
    assert_eq!(event.model, "llama-3");
    assert_eq!(event.severity, RateLimitSeverity::Critical);
    assert_eq!(event.source, RateLimitSource::Reactive429);
    assert_eq!(event.remaining_requests, Some(0));
    assert!(event.reset_at.is_some());
}

// ── Test 9: Severity Warning at <10% ─────────────────────────────────────────

#[tokio::test]
async fn severity_warning_at_under_ten_percent() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-7");
    // 5/100 = 5% → Warning
    let headers = vec![
        (
            "x-ratelimit-remaining-requests".to_string(),
            "5".to_string(),
        ),
        ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
    ];
    tracker.record_headers("openrouter", key_hash, "gpt-4o", headers);
    let event = rx.try_recv().expect("event emitted");
    assert_eq!(event.severity, RateLimitSeverity::Warning);
}

#[tokio::test]
async fn severity_info_at_above_ten_percent() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-8");
    // 15/100 = 15% → Info
    let headers = vec![
        (
            "x-ratelimit-remaining-requests".to_string(),
            "15".to_string(),
        ),
        ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
    ];
    tracker.record_headers("openrouter", key_hash, "gpt-4o", headers);
    let event = rx.try_recv().expect("event emitted");
    assert_eq!(event.severity, RateLimitSeverity::Info);
}

// ── Tests 10, 11: Severity at remaining=0 ────────────────────────────────────

#[tokio::test]
async fn severity_critical_at_zero_remaining_and_reset_far() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-9");
    let headers = vec![
        (
            "x-ratelimit-remaining-requests".to_string(),
            "0".to_string(),
        ),
        ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
        (
            "x-ratelimit-reset-requests".to_string(),
            "120.0".to_string(),
        ),
    ];
    tracker.record_headers("openrouter", key_hash, "gpt-4o", headers);
    let event = rx.try_recv().expect("event emitted");
    assert_eq!(event.severity, RateLimitSeverity::Critical);
}

#[tokio::test]
async fn severity_warning_at_zero_remaining_and_reset_near() {
    let (tracker, mut rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-10");
    let headers = vec![
        (
            "x-ratelimit-remaining-requests".to_string(),
            "0".to_string(),
        ),
        ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
        ("x-ratelimit-reset-requests".to_string(), "10.0".to_string()),
    ];
    tracker.record_headers("openrouter", key_hash, "gpt-4o", headers);
    let event = rx.try_recv().expect("event emitted");
    // remaining = 0 but reset < 60s → Warning, not Critical
    assert_eq!(event.severity, RateLimitSeverity::Warning);
}

// ── Test 12: Broadcast capacity 64 — no panic ────────────────────────────────

#[tokio::test]
async fn broadcast_capacity_overflow_does_not_panic() {
    // Drop the receiver first so send returns Err (no panic).
    let (tracker, rx) = RateLimitTracker::new();
    drop(rx);

    for _ in 0..100 {
        // No subscriber → send returns Err but does not panic.
        tracker.record_429("ollama", [0u8; 16], "llama-3", None);
    }

    // If we got here without panicking, the test passes.
}

// ── Test 13: Debug output bounded ────────────────────────────────────────────

#[test]
fn debug_output_shows_only_entry_count() {
    let (tracker, _rx) = RateLimitTracker::new();
    let key_hash = hash_api_key("test-key-13");
    tracker.record_headers(
        "openrouter",
        key_hash,
        "gpt-4o",
        vec![
            (
                "x-ratelimit-remaining-requests".to_string(),
                "1".to_string(),
            ),
            ("x-ratelimit-limit-requests".to_string(), "100".to_string()),
        ],
    );
    let debug = format!("{:?}", tracker);
    assert!(debug.contains("RateLimitTracker"));
    assert!(debug.contains("entries"));
    // Make sure no header content / no key data / no provider strings leak
    assert!(!debug.contains("openrouter"));
    assert!(!debug.contains("gpt-4o"));
    assert!(!debug.contains("x-ratelimit"));
}
