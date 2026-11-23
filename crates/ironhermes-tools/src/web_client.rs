//! Phase 54 Plan 03 (D-13/D-14): the ONE shared HTTP client construction path for
//! every outbound fetch in `ironhermes-tools`.
//!
//! `web_read.rs`, `web_extract/backends/local.rs` and `web_extract/pdf.rs` each
//! used to build their own bare `reqwest::Client` and re-validate only the
//! *final* URL, after the response was already in hand (D-13): a
//! `302 Location: http://169.254.169.254/...` meant the metadata request had
//! already fired before any check ran. A per-call-site fix would also drift
//! (D-14's naming) — three independent client constructions, three independent
//! chances to forget the fix.
//!
//! The fix here is a single custom [`reqwest::dns::Resolve`] implementation,
//! [`SsrfSafeResolver`], installed on every client this module builds. reqwest
//! consults its installed resolver for EVERY hostname resolution it performs —
//! including a redirect's NEW host, before connecting to it — so one resolver
//! closes both D-13 (redirect to an internal address) and D-14 (the DNS-rebinding
//! TOCTOU `ssrf.rs`'s own module doc names) with one mechanism. The resolver
//! itself carries no copy of the block predicates: it delegates to
//! [`ironhermes_core::ssrf::resolve_safe_addrs`], the SAME function `is_safe_url`
//! uses (task 1 of this plan), so the two can never independently drift.
//!
//! ## Redirect policy: left at reqwest's DEFAULT (follow)
//!
//! Contrast this with `crates/ironhermes-restgw/src/webhook/deliver.rs`'s
//! `Policy::none()`: that module validates a single, OPERATOR-CONFIGURED
//! `deliver: url` target that never legitimately needs to follow a redirect, so
//! disabling redirects entirely is strictly safer there — it removes a
//! capability that target never uses.
//!
//! Here the URL is AGENT-SUPPLIED, and legitimately redirects: ordinary web
//! pages behind a CDN, a URL shortener, or a canonicalizing redirect all rely on
//! it, and disabling redirects would break normal `web_read`/`web_extract`
//! fetches outright. Because [`SsrfSafeResolver`] validates every hop BEFORE
//! reqwest connects to it, redirects can keep working while remaining fully
//! protected — D-13's fix is "validate at resolution time", not "stop following
//! redirects".
//!
//! ## Client construction: one client per call, not a shared static
//!
//! The three call sites this module serves already pass different
//! configuration into their client: `web_read.rs` and
//! `web_extract/backends/local.rs` set a configured user agent and timeout from
//! `WebConfig`, while `web_extract/pdf.rs`'s `fetch_pdf_bytes` sets no user
//! agent at all today and reads only a timeout. A shared, process-lifetime
//! static client (behind a `OnceLock`/`OnceCell`) would have to pick ONE
//! caller's configuration and silently hand it to every later caller — exactly
//! the kind of drift this module exists to prevent. [`build_ssrf_safe_client`]
//! is therefore a cheap per-call constructor: each call site builds its own
//! client with its own existing inputs, routed through the one resolver. The
//! cost is a fresh connection pool per call rather than shared keep-alive
//! connections; outbound fetches in this crate (agent-triggered `web_read` /
//! `web_extract` calls) are not high-frequency enough for that to matter.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// A [`reqwest::dns::Resolve`] implementation whose only resolution source is
/// [`ironhermes_core::ssrf::resolve_safe_addrs`] — the SAME predicate
/// `ironhermes_core::ssrf::is_safe_url` uses. It carries no independent copy of
/// the block predicates; if it did, the two could drift, and a drifted resolver
/// copy is exactly the kind of silently-reopened hole this module exists to
/// close.
#[derive(Debug, Default, Clone, Copy)]
struct SsrfSafeResolver;

impl Resolve for SsrfSafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // `resolve_safe_addrs` performs synchronous (blocking) DNS resolution
            // via `ToSocketAddrs`, so it is run on the blocking thread pool —
            // never inline on the async runtime that drives this future.
            //
            // Port 0: reqwest replaces a `0` port in a returned `SocketAddr` with
            // the scheme's conventional port, and an explicit port in the
            // original URL always wins over both — see
            // `ironhermes_core::ssrf::resolve_safe_addrs`'s own doc comment.
            let addrs: Vec<SocketAddr> = tokio::task::spawn_blocking(move || {
                ironhermes_core::ssrf::resolve_safe_addrs(&host, 0)
            })
            .await
            .map_err(|join_err| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(std::io::Error::other(format!(
                    "SSRF-safe DNS resolution task panicked: {join_err}"
                )))
            })?
            .map_err(|e: anyhow::Error| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Build an SSRF-safe [`reqwest::Client`]: every hostname resolution this client
/// performs — including a redirect's new host, before the connection to it is
/// made — is routed through [`SsrfSafeResolver`]. See this module's doc comment
/// for the redirect-policy and construction-strategy rationale.
///
/// `user_agent`: `Some(ua)` sets the `User-Agent` header exactly as the
/// call site's pre-existing construction did; `None` omits it entirely,
/// preserving `web_extract/pdf.rs::fetch_pdf_bytes`'s current behavior of
/// sending no `User-Agent` header at all.
pub fn build_ssrf_safe_client(
    user_agent: Option<&str>,
    timeout: Duration,
) -> reqwest::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .dns_resolver(Arc::new(SsrfSafeResolver));
    if let Some(ua) = user_agent {
        builder = builder.user_agent(ua);
    }
    builder.build()
}

// ---------------------------------------------------------------------------
// Falsification tests (Phase 54 Plan 03 Task 3, D-13/D-14) — proving the
// RESOLVER, not the retained second-line `validate_url_async` re-check, is
// what blocks a redirect to an internal address.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::str::FromStr;
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// `IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK` short-circuits `validate_url_async`
    /// in `web_local.rs` for loopback hosts, BEFORE `is_safe_url` and therefore
    /// before anything this module adds (54-RESEARCH.md Pitfall 3). This
    /// resolver module never calls `validate_url_async`, so the variable has no
    /// effect on it either way -- but a developer shell that exported it
    /// globally would make every OTHER loopback-dependent assertion in this
    /// crate unreliable, so every test below defeats it defensively rather than
    /// trusting the ambient environment. Matches this crate's existing
    /// convention (see `browser_session.rs`'s env-var tests) of calling
    /// `std::env::remove_var` directly, without an `unsafe` wrapper.
    fn clear_loopback_bypass() {
        // SAFETY: this crate's tests are run with `--test-threads=1` for this
        // module's filter (per this task's own <precondition>/<action>), so
        // there is a single mutator of process env at a time (Phase 21.6 Rust
        // 2024 pattern, mirrored from `browser_session.rs`'s env-var tests).
        unsafe {
            std::env::remove_var("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK");
        }
    }

    /// Test-only resolver composing the REAL production [`SsrfSafeResolver`]
    /// with (a) address overrides so this test's own loopback `wiremock`
    /// servers are reachable at all -- production `resolve_safe_addrs` would
    /// otherwise refuse loopback outright, which would make it impossible to
    /// stand up any local test server -- and (b) an additional test-supplied
    /// blocklist for the pretend-metadata hostname. This is approach (b) from
    /// the plan's `<action>`: the production predicate is still the thing
    /// under test for every hostname NOT explicitly overridden below, and the
    /// override/blocklist entries exist solely to make the test's own harness
    /// servers reachable and the pretend-metadata host refusable without
    /// touching `IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK` at all.
    struct TestOverrideResolver {
        overrides: HashMap<String, Vec<SocketAddr>>,
        extra_blocked: HashSet<String>,
    }

    impl Resolve for TestOverrideResolver {
        fn resolve(&self, name: Name) -> Resolving {
            let host = name.as_str().to_string();
            if self.extra_blocked.contains(&host) {
                return Box::pin(async move {
                    Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                        "test override: {host} is blocked by the test-supplied blocklist"
                    )))
                });
            }
            if let Some(addrs) = self.overrides.get(&host) {
                let addrs = addrs.clone();
                return Box::pin(async move {
                    let iter: Addrs = Box::new(addrs.into_iter());
                    Ok(iter)
                });
            }
            // Fall through to the REAL production predicate for anything else.
            SsrfSafeResolver.resolve(name)
        }
    }

    /// A resolver that delegates to a fixed, test-supplied address table (NOT
    /// the production predicate -- this test is about reqwest's OWN resolver
    /// consultation behavior, not about which addresses are safe) while
    /// recording exactly how many times each hostname was resolved.
    struct CountingResolver {
        addrs: HashMap<String, Vec<SocketAddr>>,
        calls: Arc<Mutex<HashMap<String, usize>>>,
    }

    impl Resolve for CountingResolver {
        fn resolve(&self, name: Name) -> Resolving {
            let host = name.as_str().to_string();
            *self.calls.lock().unwrap().entry(host.clone()).or_insert(0) += 1;
            let addrs = self.addrs.get(&host).cloned().unwrap_or_default();
            Box::pin(async move {
                let iter: Addrs = Box::new(addrs.into_iter());
                Ok(iter)
            })
        }
    }

    /// D-13 falsification target. A wiremock "origin" server 302-redirects to a
    /// "pretend-metadata" server (a real cloud-metadata IP is not reachable
    /// from a test, so this stands in for it). The load-bearing assertion is
    /// the pretend-metadata server's request COUNT, not just the error: an
    /// error alone would also be produced by the retained second-line
    /// `validate_url_async` re-check, so only a zero request count proves the
    /// RESOLVER -- not the re-check -- is what blocked the connection before
    /// it happened.
    #[tokio::test]
    async fn redirect_to_metadata_blocked_before_connect() {
        clear_loopback_bypass();

        let origin = MockServer::start().await;
        let metadata = MockServer::start().await;

        let origin_host = "ssrf-test-origin.invalid";
        let metadata_host = "ssrf-test-metadata.invalid";
        let origin_port = origin.address().port();
        let metadata_port = metadata.address().port();

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "Location",
                format!("http://{metadata_host}:{metadata_port}/").as_str(),
            ))
            .mount(&origin)
            .await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("metadata leaked"))
            .mount(&metadata)
            .await;

        // Both hostnames are registered in `overrides` (so BOTH are actually
        // reachable at the DNS-resolution level) -- `extra_blocked` is the ONLY
        // thing standing between the redirect and the pretend-metadata server.
        // This is deliberate: it is what makes the SC-8 mutation below (deleting
        // the `extra_blocked` check) equivalent to "the resolver's validation
        // was removed" rather than merely "the host became unresolvable" --
        // exactly matching the plan's described signal (the metadata counter
        // becomes 1, not that the request errors for an unrelated reason).
        let mut overrides = HashMap::new();
        overrides.insert(
            origin_host.to_string(),
            vec![SocketAddr::new(
                "127.0.0.1".parse().expect("valid IP literal"),
                origin_port,
            )],
        );
        overrides.insert(
            metadata_host.to_string(),
            vec![SocketAddr::new(
                "127.0.0.1".parse().expect("valid IP literal"),
                metadata_port,
            )],
        );
        let mut extra_blocked = HashSet::new();
        extra_blocked.insert(metadata_host.to_string());

        let resolver = TestOverrideResolver {
            overrides,
            extra_blocked,
        };

        let client = reqwest::Client::builder()
            .dns_resolver(Arc::new(resolver))
            .build()
            .expect("client must build");

        let result = client
            .get(format!("http://{origin_host}:{origin_port}/"))
            .send()
            .await;

        assert!(
            result.is_err(),
            "a redirect to a blocked host must surface as a fetch error, got: {result:?}"
        );

        let hits = metadata.received_requests().await.unwrap_or_default().len();
        assert_eq!(
            hits, 0,
            "the pretend-metadata server must receive ZERO requests -- this is the \
             load-bearing assertion: `result.is_err()` alone would also be true if the \
             resolver did nothing and only the retained second-line re-check fired \
             AFTER the connection was already made"
        );
    }

    /// D-14 falsification target: the resolver is consulted exactly once per
    /// hostname per request -- there is no second, unvalidated resolution
    /// between check and connect.
    #[tokio::test]
    async fn ip_pinned_no_second_resolve() {
        clear_loopback_bypass();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let host = "ssrf-test-pinned.invalid";
        let port = server.address().port();
        let addr = SocketAddr::new("127.0.0.1".parse().expect("valid IP literal"), port);

        let mut addrs = HashMap::new();
        addrs.insert(host.to_string(), vec![addr]);
        let calls: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));

        let resolver = CountingResolver {
            addrs,
            calls: calls.clone(),
        };

        let client = reqwest::Client::builder()
            .dns_resolver(Arc::new(resolver))
            .build()
            .expect("client must build");

        let response = client
            .get(format!("http://{host}:{port}/"))
            .send()
            .await
            .expect("request through the overridden resolver must succeed");
        assert!(response.status().is_success());

        let call_count = *calls.lock().unwrap().get(host).unwrap_or(&0);
        assert_eq!(
            call_count, 1,
            "the resolver must be consulted EXACTLY ONCE per hostname per request -- a \
             second, unvalidated resolution between check and connect would show up as a \
             second call here"
        );

        // Peer-address observability: reqwest's public `Response` API does not expose the
        // underlying peer `SocketAddr` the connection actually used, so the "peer address
        // matches the resolver's returned address" half of this test's <action> could not
        // be observed from outside reqwest's internals. Recorded explicitly here (and in
        // SUMMARY.md's "Test-approach decisions") rather than silently dropped: the
        // call-count assertion above is what this test actually proves.
    }

    /// D-14 falsification target: driving the resolver directly with a
    /// hostname that resolves only to blocked addresses must error, never
    /// return an empty success.
    #[tokio::test]
    async fn resolver_refuses_host_resolving_only_to_blocked_addresses() {
        clear_loopback_bypass();

        // "localhost" is an ordinary DNS name (not an IP literal, so no
        // uncertainty about `Name`'s parser accepting IP-literal syntax) that
        // resolves to loopback on every test machine -- entirely blocked.
        let name = Name::from_str("localhost").expect("localhost must parse as a Name");
        let result = SsrfSafeResolver.resolve(name).await;
        // `Addrs` (`Box<dyn Iterator<...> + Send>`) does not implement `Debug`, so the
        // `Result` as a whole cannot be formatted directly — assert on `is_err()` and
        // report only the error side, which does implement `Display`.
        match result {
            Err(_) => {}
            Ok(_) => panic!(
                "a hostname resolving only to blocked addresses must error, not return an \
                 empty success"
            ),
        }
    }
}
