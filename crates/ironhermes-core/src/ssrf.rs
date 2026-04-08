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
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
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
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || is_cgnat(v4)
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_multicast() || v6.is_unspecified(),
    }
}

/// Check whether an IPv4 address falls within the CGNAT range (100.64.0.0/10).
fn is_cgnat(ip: Ipv4Addr) -> bool {
    let bits: u32 = ip.into();
    bits >= CGNAT_START && bits <= CGNAT_END
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
        assert!(!is_safe_url("https://192.168.1.1"));
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
}
