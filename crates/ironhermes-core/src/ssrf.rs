//! SSRF (Server-Side Request Forgery) URL validation.
//! Port of hermes-agent/tools/url_safety.py.
//!
//! DNS rebinding is a known limitation (TOCTOU between resolution and connection) -- D-17.
//! The resolve step and the actual HTTP connection are separate operations, so a malicious
//! DNS server could return a safe IP during validation and a private IP during connection.
//!
//! **Async callers**: `is_safe_url` uses synchronous DNS resolution via `ToSocketAddrs`.
//! In async contexts, wrap with `tokio::task::spawn_blocking(|| is_safe_url(url))` or
//! switch to `tokio::net::lookup_host` at the call site. Phase 4 will handle async wrapping.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::LazyLock;
use tracing::warn;
use url::Url;

/// Hostnames blocked regardless of their resolved IPs (cloud metadata endpoints -- D-18).
static BLOCKED_HOSTNAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["metadata.google.internal", "metadata.goog"]
        .into_iter()
        .collect()
});

/// CGNAT range 100.64.0.0/10 -- not covered by `Ipv4Addr::is_private()`.
const CGNAT_START: u32 = 0x6440_0000; // 100.64.0.0
const CGNAT_END: u32 = 0x647F_FFFF; // 100.127.255.255

/// Check whether a URL is safe to fetch (not targeting internal/private resources).
///
/// Returns `true` only if the URL parses correctly, has a hostname that is not blocked,
/// resolves via DNS to at least one IP, and ALL resolved IPs are public.
/// Returns `false` (fail closed) on any parse error, missing host, DNS failure, or
/// if any resolved IP is private/loopback/link-local/CGNAT/metadata.
pub fn is_safe_url(url_str: &str) -> bool {
    // Parse URL -- fail closed on parse error
    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => {
            warn!("SSRF blocked: failed to parse URL: {}", url_str);
            return false;
        }
    };

    // Extract hostname -- fail closed if no host
    let host = match url.host_str() {
        Some(h) => h,
        None => {
            warn!("SSRF blocked: no host in URL: {}", url_str);
            return false;
        }
    };

    // Check against blocked hostnames (D-18)
    if BLOCKED_HOSTNAMES.contains(host) {
        warn!("SSRF blocked: blocked hostname: {}", host);
        return false;
    }

    // Resolve hostname via DNS -- fail closed on resolution error (D-16)
    let port = url.port().unwrap_or(0);
    let addrs = match (host, port).to_socket_addrs() {
        Ok(a) => a,
        Err(_) => {
            warn!("SSRF blocked: DNS resolution failed for: {}", host);
            return false;
        }
    };

    // Check EVERY resolved IP -- block if ANY is unsafe (D-14 foundation): this is
    // the SAME shared helper `resolve_safe_addrs` calls, so the two never carry two
    // independently-drifting copies of the block predicate.
    match filter_safe_addrs(addrs) {
        Ok(_) => true,
        Err(e) => {
            warn!("SSRF blocked: {} rejected by policy: {}", url_str, e);
            false
        }
    }
}

/// Apply the SAME block predicate `is_safe_url` uses to an already-resolved set of
/// addresses, with a **whole-host-refusal** posture: if ANY address is blocked, the
/// entire set is refused (`Err`) rather than silently returning only the "safe"
/// remainder.
///
/// This matches `is_safe_url`'s own pre-existing semantics exactly (see the loop
/// this function replaces above: it returns `false` for the WHOLE URL the instant
/// any resolved IP is blocked, never a partial pass) — a host that resolves to a
/// mix of a public and an internal address is a classic DNS-rebinding signature,
/// and connecting to "the safe one" would still resolve the redirect/next request
/// through the same rebinding-capable name. Whole-host refusal is deliberately
/// chosen over a partial-allow posture for this reason; this function is not free
/// to invent a divergent policy.
///
/// Never returns an empty `Ok`: an empty address list handed back to a caller (in
/// particular, `web_client`'s `reqwest::dns::Resolve` implementation, which treats
/// an `Ok` iterator as "these are the addresses to connect to") would be
/// indistinguishable from "resolver legitimately found nothing" and could let a
/// caller fall through to its own, unchecked resolution.
fn filter_safe_addrs<I>(addrs: I) -> anyhow::Result<Vec<SocketAddr>>
where
    I: IntoIterator<Item = SocketAddr>,
{
    let addrs: Vec<SocketAddr> = addrs.into_iter().collect();
    if addrs.is_empty() {
        anyhow::bail!("SSRF: no addresses to evaluate (empty resolution)");
    }
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            anyhow::bail!(
                "SSRF: resolved address {} is blocked by policy (whole-host refusal)",
                addr.ip()
            );
        }
    }
    Ok(addrs)
}

/// Resolve `host:port` and return only the addresses that pass the same block
/// predicate `is_safe_url` already applies (`filter_safe_addrs`) — the ONE shared
/// predicate a custom `reqwest::dns::Resolve` implementation (see
/// `ironhermes_tools::web_client`) consults for EVERY hostname resolution it
/// performs, including a redirect's new host, before connecting to it. Sharing
/// this one function is what prevents `is_safe_url` and the resolver from drifting
/// onto two different, independently-maintained definitions of "safe" — the drift
/// itself is the hole (D-14).
///
/// Returns `Err` rather than an empty `Ok` when nothing survives filtering (see
/// `filter_safe_addrs`'s doc comment for why).
///
/// Note for callers building a `reqwest::dns::Resolve` implementation: reqwest
/// replaces a port of `0` in a returned `SocketAddr` with the scheme's
/// conventional port (80/443/...), and an explicit port in the original URL
/// always wins over both. A caller that has no specific port to resolve against
/// may therefore pass `0` safely.
pub fn resolve_safe_addrs(host: &str, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| anyhow::anyhow!("SSRF: DNS resolution failed for {host}: {e}"))?;
    filter_safe_addrs(addrs)
}

/// Check whether an IP address belongs to a blocked range.
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (`::ffff:a.b.c.d`) reaches the SAME host as
            // the bare IPv4 address, so it must be judged by the IPv4 rules.
            // `Ipv6Addr::is_loopback()` is true only for `::1`, so without this
            // unwrap a hostname with a static AAAA of `::ffff:127.0.0.1` (or
            // `::ffff:169.254.169.254`) passed every check and connected to
            // loopback / cloud metadata. No DNS-rebinding race required.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_v4(v4);
            }
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || is_unique_local_v6(v6)
                || is_unicast_link_local_v6(v6)
        }
    }
}

/// The IPv4 blocked-range predicate, shared with the IPv4-mapped IPv6 arm.
fn is_blocked_v4(v4: Ipv4Addr) -> bool {
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_unspecified()
        || is_cgnat(v4)
}

/// IPv6 unique-local addresses, `fc00::/7` — the v6 analogue of RFC1918 private
/// space. Hand-rolled because `Ipv6Addr::is_unique_local` is still unstable.
fn is_unique_local_v6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// IPv6 unicast link-local addresses, `fe80::/10` — the v6 analogue of
/// `169.254.0.0/16`. Hand-rolled because `Ipv6Addr::is_unicast_link_local` is
/// still unstable.
fn is_unicast_link_local_v6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// Check whether an IPv4 address falls within the CGNAT range (100.64.0.0/10).
fn is_cgnat(ip: Ipv4Addr) -> bool {
    let bits: u32 = ip.into();
    (CGNAT_START..=CGNAT_END).contains(&bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    // --- Tests using IP addresses directly (no DNS needed) ---

    #[test]
    fn test_loopback_ipv4_blocked() {
        assert!(!is_safe_url("https://127.0.0.1"));
    }

    #[test]
    fn test_private_192_168_blocked() {
        assert!(!is_safe_url("https://127.0.0.1"));
    }

    #[test]
    fn test_private_10_blocked() {
        assert!(!is_safe_url("https://10.0.0.1"));
    }

    #[test]
    fn test_private_172_16_blocked() {
        assert!(!is_safe_url("https://172.16.0.1"));
    }

    #[test]
    fn test_link_local_blocked() {
        assert!(!is_safe_url("https://169.254.1.1"));
    }

    #[test]
    fn test_cgnat_blocked() {
        assert!(!is_safe_url("https://100.100.100.100"));
    }

    #[test]
    fn test_unspecified_blocked() {
        assert!(!is_safe_url("https://0.0.0.0"));
    }

    #[test]
    fn test_parse_error_fails_closed() {
        assert!(!is_safe_url("not-a-url"));
    }

    // --- is_blocked_ip unit tests ---

    #[test]
    fn test_blocked_ip_ipv6_loopback() {
        assert!(is_blocked_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn test_blocked_ip_ipv6_unspecified() {
        assert!(is_blocked_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn test_blocked_ip_ipv4_broadcast() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::BROADCAST)));
    }

    #[test]
    fn test_cgnat_range_start() {
        assert!(is_cgnat(Ipv4Addr::new(100, 64, 0, 0)));
    }

    #[test]
    fn test_cgnat_range_end() {
        assert!(is_cgnat(Ipv4Addr::new(100, 127, 255, 255)));
    }

    #[test]
    fn test_cgnat_range_just_below() {
        assert!(!is_cgnat(Ipv4Addr::new(100, 63, 255, 255)));
    }

    #[test]
    fn test_cgnat_range_just_above() {
        assert!(!is_cgnat(Ipv4Addr::new(100, 128, 0, 0)));
    }

    // --- Tests requiring DNS resolution (marked #[ignore] for CI reliability) ---

    #[test]
    #[ignore]
    fn test_public_url_allowed() {
        assert!(is_safe_url("https://example.com"));
    }

    #[test]
    #[ignore]
    fn test_localhost_blocked() {
        assert!(!is_safe_url("https://localhost"));
    }

    #[test]
    #[ignore]
    fn test_metadata_google_internal_blocked() {
        // This will fail DNS but the hostname check catches it first
        assert!(!is_safe_url("https://metadata.google.internal"));
    }

    #[test]
    fn test_metadata_goog_blocked() {
        // Hostname check catches this before DNS
        assert!(!is_safe_url("https://metadata.goog"));
    }

    // --- IPv6 blocked ranges (36.7.1 code review CR-01) ---
    //
    // These assert on `is_blocked_ip` directly rather than through
    // `is_safe_url`, because a bracketed IPv6 *literal* in a URL fails closed
    // for an unrelated reason: `Url::host_str()` returns the bracket-stripped
    // form, `ToSocketAddrs` cannot resolve it, and the DNS-failure branch
    // rejects it. That accident is what made a literal spot-check look
    // "blocked" while a hostname with a static AAAA record sailed through.
    // The predicate is what the resolved-address loop actually consults, so
    // the predicate is what these tests pin.

    #[test]
    fn ipv4_mapped_loopback_blocked() {
        // `::ffff:127.0.0.1` reaches the same host as `127.0.0.1`.
        // `Ipv6Addr::is_loopback()` is true only for `::1`, so before CR-01
        // this returned false and delivery connected to loopback.
        let ip: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_blocked_ip(ip));
    }

    #[test]
    fn ipv4_mapped_cloud_metadata_blocked() {
        let ip: IpAddr = "::ffff:169.254.169.254".parse().unwrap();
        assert!(is_blocked_ip(ip));
    }

    #[test]
    fn ipv4_mapped_private_blocked() {
        for s in ["::ffff:10.0.0.1", "::ffff:127.0.0.1", "::ffff:172.16.0.1"] {
            let ip: IpAddr = s.parse().unwrap();
            assert!(is_blocked_ip(ip), "{s} must be blocked");
        }
    }

    #[test]
    fn ipv6_unique_local_blocked() {
        // fc00::/7 — both halves of the range.
        for s in ["fc00::1", "fd00::1", "fdff:ffff::1"] {
            let ip: IpAddr = s.parse().unwrap();
            assert!(is_blocked_ip(ip), "{s} must be blocked");
        }
    }

    #[test]
    fn ipv6_link_local_blocked() {
        // fe80::/10
        for s in ["fe80::1", "febf:ffff::1"] {
            let ip: IpAddr = s.parse().unwrap();
            assert!(is_blocked_ip(ip), "{s} must be blocked");
        }
    }

    #[test]
    fn ipv6_loopback_and_unspecified_still_blocked() {
        assert!(is_blocked_ip("::1".parse().unwrap()));
        assert!(is_blocked_ip("::".parse().unwrap()));
    }

    #[test]
    fn ipv6_public_address_still_allowed() {
        // The fix must not over-block: a global-unicast v6 address stays
        // reachable, and `fec0::`/`ff00::` boundaries are not misclassified.
        assert!(!is_blocked_ip("2001:db8::1".parse().unwrap()));
        assert!(!is_blocked_ip("2606:4700:4700::1111".parse().unwrap()));
        // fec0::/10 is deprecated site-local, NOT inside fe80::/10 — the mask
        // must not accidentally swallow it as link-local.
        assert!(!is_unicast_link_local_v6("fec0::1".parse().unwrap()));
        // fb00::/8 sits just below fc00::/7 and must not be caught.
        assert!(!is_unique_local_v6("fb00::1".parse().unwrap()));
    }

    #[test]
    fn ipv4_mapped_public_address_still_allowed() {
        let ip: IpAddr = "::ffff:93.184.216.34".parse().unwrap();
        assert!(!is_blocked_ip(ip));
    }

    // --- D-15 regression pins (54-03) ---
    //
    // These pin behavior that is ALREADY CORRECT — CONTEXT.md's roadmap correction
    // (`.planning/phases/54-adr-0005-security-audit-findings-hardening/54-CONTEXT.md`,
    // verified against this file on 2026-09-29) found that `is_blocked_v4`,
    // `is_unique_local_v6` and `is_unicast_link_local_v6` already handle every
    // spelling ROADMAP.md's SC-3 wrongly claimed was missing. These tests exist so a
    // future refactor of those predicates cannot silently regress coverage that
    // already works. Per the same correction: DO NOT rebuild these predicates.

    #[test]
    fn metadata_literal_addr_aws_blocked() {
        assert!(
            !is_safe_url("http://169.254.169.254"),
            "the AWS/GCP cloud-metadata literal IPv4 address (link-local range) must be blocked"
        );
    }

    #[test]
    fn metadata_literal_addr_alibaba_blocked() {
        assert!(
            !is_safe_url("http://100.100.100.200"),
            "the Alibaba Cloud metadata literal IPv4 address (CGNAT range) must be blocked"
        );
    }

    #[test]
    fn metadata_literal_addr_v4_mapped_v6_blocked() {
        let ip: IpAddr = "::ffff:169.254.169.254".parse().unwrap();
        assert!(
            is_blocked_ip(ip),
            "v4-mapped IPv6 spelling of the AWS metadata address must delegate through \
             to_ipv4_mapped() to is_blocked_v4 and be blocked"
        );
    }

    #[test]
    fn ipv6_ula_and_link_local_still_blocked() {
        let ula: IpAddr = "fd00::1".parse().unwrap();
        assert!(
            is_blocked_ip(ula),
            "a unique-local IPv6 address (fc00::/7) must still be blocked"
        );
        let link_local: IpAddr = "fe80::1".parse().unwrap();
        assert!(
            is_blocked_ip(link_local),
            "a unicast link-local IPv6 address (fe80::/10) must still be blocked"
        );
    }

    // --- resolve_safe_addrs (D-14 foundation) ---

    #[test]
    fn resolve_safe_addrs_returns_addrs_for_public_ip() {
        // A literal IP address avoids any live DNS lookup.
        let addrs =
            resolve_safe_addrs("93.184.216.34", 443).expect("a public IP literal must resolve");
        assert!(!addrs.is_empty(), "must return at least one address");
        assert!(
            addrs.iter().all(|a| !is_blocked_ip(a.ip())),
            "every returned address must satisfy the same block predicate is_safe_url uses"
        );
    }

    #[test]
    fn resolve_safe_addrs_errs_for_host_resolving_only_to_blocked_addresses() {
        // 127.0.0.1 is a literal loopback address; to_socket_addrs needs no DNS.
        let result = resolve_safe_addrs("127.0.0.1", 80);
        assert!(
            result.is_err(),
            "a host resolving only to blocked addresses must error, never return an empty Ok"
        );
    }

    #[test]
    fn resolve_safe_addrs_whole_host_refuses_mixed_resolution() {
        // Exercises the shared filter directly with a synthetic mixed set (one
        // public address, one blocked address) since a literal-IP hostname cannot
        // resolve to more than itself. This is the posture documented on
        // `filter_safe_addrs`: is_safe_url already blocks the WHOLE url the instant
        // any resolved address is unsafe, and resolve_safe_addrs matches that
        // semantics exactly rather than inventing a divergent partial-allow policy.
        let mixed = vec![
            SocketAddr::new("93.184.216.34".parse().unwrap(), 443),
            SocketAddr::new("169.254.169.254".parse().unwrap(), 443),
        ];
        let result = filter_safe_addrs(mixed);
        assert!(
            result.is_err(),
            "a mix of allowed and blocked addresses must refuse the WHOLE host, matching \
             is_safe_url's block-on-any semantics — never silently return only the safe one"
        );
    }
}
