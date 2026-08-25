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
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
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

    let addrs: Vec<_> = addrs.collect();
    if addrs.is_empty() {
        warn!("SSRF blocked: no addresses resolved for: {}", host);
        return false;
    }

    // Check EVERY resolved IP -- block if ANY is unsafe
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            warn!(
                "SSRF blocked: {} resolved to blocked IP {}",
                url_str,
                addr.ip()
            );
            return false;
        }
    }

    true
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
}
