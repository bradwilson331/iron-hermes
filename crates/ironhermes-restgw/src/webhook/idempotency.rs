//! Self-pruning per-route idempotency cache (D-15) so a retried delivery
//! runs exactly one agent turn.
//!
//! D-12 makes the listener ack with 202 and run the turn in the background
//! because senders time out around ten seconds — a sender that has already
//! timed out and retried the exact same delivery is the normal case, not the
//! exceptional one. Without a claim check, one logical delivery becomes
//! several agent turns and several outbound deliveries.
//!
//! This is also the defence-in-depth layer for the in-window replay case the
//! signature schemes' timestamp binding cannot cover: two valid, distinctly
//! signed-but-identical requests delivered twice inside the skew window.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Header a sender may send to label a delivery. **It does not influence the
/// idempotency key** — see [`derive_key`] for why. Retained as a constant
/// because it is still read for logging/correlation, and because naming it
/// here keeps the "why not" documented next to the thing it refers to.
///
/// Before the 36.7.1 code review (CR-02) this header WAS the key when
/// present. No signature scheme covers it (`generic_v2` signs `{ts}.{body}`,
/// Telnyx signs `{ts}|{body}`, Twilio signs URL + sorted params), so a single
/// captured valid request could be replayed with a fresh key each time —
/// every replay claiming a distinct cache entry and running a full agent
/// turn — or replayed with a key the genuine sender was about to use, which
/// then silently swallowed the real delivery.
pub const HEADER_IDEMPOTENCY_KEY: &str = "X-Webhook-Idempotency-Key";

/// Injectable time source (source_facts #7): this platform has no GNU
/// `timeout` and fresh test binaries can stall in the dynamic loader, so
/// TTL/window-dependent rail tests advance a [`FakeClock`] deterministically
/// instead of sleeping in real time. [`crate::webhook::rate_limit::FixedWindowLimiter`]
/// (Task 3) reuses this exact abstraction rather than inventing a second
/// notion of time for the same handler.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// The real system clock — what [`crate::webhook::WebhookAdapter::new`]
/// wires in by default.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Test-only clock: starts at construction time and only moves forward when
/// [`FakeClock::advance`] is called explicitly — never on its own, and never
/// by sleeping.
#[derive(Debug)]
pub struct FakeClock(Mutex<Instant>);

impl FakeClock {
    pub fn new() -> Self {
        Self(Mutex::new(Instant::now()))
    }

    /// Move the clock forward by `by`. Does not sleep — the next call to
    /// [`Clock::now`] simply returns the advanced instant.
    pub fn advance(&self, by: Duration) {
        let mut guard = self.0.lock().unwrap();
        *guard += by;
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}

/// Derive the delivery key for a request from **authenticated material only**
/// (CR-02).
///
/// A signed route keys on its signature: that value is unique per signed
/// delivery, is produced by a party holding the route secret, and — for
/// `generic_v2` and Telnyx — commits to the timestamp as well as the body, so
/// two legitimately-distinct deliveries with identical bodies still key
/// apart. A `signature: none` route has no authenticated material, so it
/// falls back to a digest of the route name and the raw body, unchanged.
///
/// Deliberately ignores [`HEADER_IDEMPOTENCY_KEY`]. Letting an unauthenticated,
/// client-chosen value widen the key space is what made one captured request
/// replayable into unbounded agent turns, and letting it *narrow* the key
/// space let an attacker pre-claim a key the genuine sender would later use.
/// A caller-controlled input cannot be allowed to decide whether work runs.
pub fn derive_key(route: &str, signature: Option<&str>, raw_body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(route.as_bytes());
    hasher.update(b"\0");
    match signature {
        // Domain-separated so a signature value can never collide with a
        // body digest on the same route.
        Some(sig) if !sig.is_empty() => {
            hasher.update(b"sig\0");
            hasher.update(sig.as_bytes());
        }
        _ => {
            hasher.update(b"body\0");
            hasher.update(raw_body);
        }
    }
    hex::encode(hasher.finalize())
}

/// One route's idempotency cache. Scoped to a single route by construction
/// (`WebhookAdapter::new` builds one instance per route from that route's own
/// `rails.idempotency_ttl_secs`) — the same key value on two different
/// routes therefore claims independently without this type needing to know
/// route identity itself. The TTL is read once at construction and held
/// immutably thereafter (D-17): nothing reconfigures it while the listener
/// runs.
pub struct IdempotencyCache {
    ttl: Duration,
    clock: Arc<dyn Clock>,
    entries: Mutex<HashMap<String, Instant>>,
}

impl IdempotencyCache {
    /// Production constructor — the real system clock.
    pub fn new(ttl_secs: u64) -> Self {
        Self::with_clock(ttl_secs, Arc::new(SystemClock))
    }

    /// Test/advanced constructor accepting an injectable clock.
    pub fn with_clock(ttl_secs: u64, clock: Arc<dyn Clock>) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            clock,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Claim `key`. Returns `true` when THIS caller is the first to claim it
    /// within the TTL window — the handler must spawn an agent turn only on
    /// `true`. Returns `false` when an unexpired claim already exists — the
    /// handler still acks 202 either way, but must not spawn a second turn.
    ///
    /// Prunes every entry older than the TTL as a side effect of this same
    /// call — lazily, on access, never via a background task. A background
    /// sweeper is a second thing to schedule, supervise and shut down for a
    /// cache whose own access pattern already provides the sweep
    /// opportunity, and lazy pruning is what makes the entry count track
    /// live traffic rather than lifetime request count.
    pub fn claim(&self, key: &str) -> bool {
        let now = self.clock.now();
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, inserted_at| now.duration_since(*inserted_at) < self.ttl);

        if entries.contains_key(key) {
            return false;
        }
        entries.insert(key.to_string(), now);
        true
    }

    /// Live entry count — used by tests to assert the cache prunes itself
    /// without an external sweeper.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_claim_wins_second_is_refused_within_ttl() {
        let cache = IdempotencyCache::new(3600);
        assert!(cache.claim("k1"));
        assert!(!cache.claim("k1"));
    }

    #[test]
    fn distinct_keys_both_claim() {
        let cache = IdempotencyCache::new(3600);
        assert!(cache.claim("k1"));
        assert!(cache.claim("k2"));
    }

    #[test]
    fn claim_after_ttl_elapses_succeeds_again_and_prunes() {
        let clock = Arc::new(FakeClock::new());
        let cache = IdempotencyCache::with_clock(60, clock.clone());
        assert!(cache.claim("k1"));
        assert_eq!(cache.len(), 1);

        clock.advance(Duration::from_secs(61));
        assert!(cache.claim("k1"));
        assert_eq!(cache.len(), 1, "the expired entry was pruned, not accumulated");
    }

    #[test]
    fn derive_key_binds_to_the_signature_not_the_body() {
        // A signed route keys on its signature, so two deliveries that happen
        // to carry the same body but were signed separately (different
        // timestamps) key apart, and a replay of the SAME signed request
        // keys together.
        let a = derive_key("route-a", Some("sig-aaa"), b"same body");
        let replay = derive_key("route-a", Some("sig-aaa"), b"same body");
        let distinct = derive_key("route-a", Some("sig-bbb"), b"same body");
        assert_eq!(a, replay, "the same signed request must claim one key");
        assert_ne!(a, distinct, "separately signed deliveries must key apart");
    }

    #[test]
    fn derive_key_never_returns_caller_controlled_input_verbatim() {
        // CR-02 regression: `derive_key` once returned the sender's
        // `X-Webhook-Idempotency-Key` verbatim. Nothing it returns may ever be
        // a value a caller supplied — the output is always a digest.
        let key = derive_key("route-a", Some("sender-key-123"), b"body");
        assert_ne!(key, "sender-key-123");
        assert_eq!(key.len(), 64, "output must be a hex SHA-256 digest");
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn derive_key_domain_separates_signature_from_body() {
        // A signature value must never collide with a body digest on the same
        // route, even if the bytes coincide.
        let by_sig = derive_key("route-a", Some("collide"), b"unused");
        let by_body = derive_key("route-a", None, b"collide");
        assert_ne!(by_sig, by_body);
    }

    #[test]
    fn derive_key_falls_back_to_digest_when_header_absent() {
        let a = derive_key("route-a", None, b"same body");
        let b = derive_key("route-a", None, b"same body");
        assert_eq!(a, b, "identical route + body must derive the same digest");

        let different_body = derive_key("route-a", None, b"different body");
        assert_ne!(a, different_body);

        let different_route = derive_key("route-b", None, b"same body");
        assert_ne!(a, different_route);
    }

    #[test]
    fn derive_key_falls_back_to_digest_when_header_empty() {
        let a = derive_key("route-a", Some(""), b"body");
        let b = derive_key("route-a", None, b"body");
        assert_eq!(a, b, "an empty header value must not be treated as a real key");
    }
}
