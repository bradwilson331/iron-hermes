//! Loopback / LAN endpoint classification (Phase 58.3.1, D-11).
//!
//! An unknown model served from a local endpoint is assumed to have a small
//! window; the same unknown model on a hosted endpoint keeps the large default.
//! `provider_key_guard_allows` is loopback-only, so LAN needs its own helper.

use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

fn is_local_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || ip.is_link_local()
}

fn is_local_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        || first & 0xfe00 == 0xfc00 // unique-local fc00::/7
        || first & 0xffc0 == 0xfe80 // link-local fe80::/10
        || ip.to_ipv4_mapped().is_some_and(is_local_v4)
}

fn is_local_domain(domain: &str) -> bool {
    let d = domain.to_ascii_lowercase();
    d == "localhost"
        || !d.contains('.') // single label: container / LAN name such as `ollama`
        || [".local", ".lan", ".internal", ".home.arpa"]
            .iter()
            .any(|suffix| d.ends_with(suffix))
}

/// True when `base_url` points at a loopback, private-range, link-local or
/// LAN-style host. Parse failure is not local; `localhost.evil.com` is not local.
pub fn is_local_endpoint(base_url: &str) -> bool {
    let Ok(url) = Url::parse(base_url.trim()) else {
        return false;
    };
    match url.host() {
        Some(Host::Ipv4(ip)) => is_local_v4(ip),
        Some(Host::Ipv6(ip)) => is_local_v6(ip),
        Some(Host::Domain(d)) => is_local_domain(d),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_class_loopback_private_and_ula_are_local() {
        for u in [
            "http://127.0.0.1:1337/v1",
            "http://localhost:8080",
            "http://LOCALHOST:1/v1",
            "http://[::1]:8080",
            "http://127.0.0.1:8080/v1",
            "http://10.0.0.5",
            "http://172.16.3.4",
            "http://169.254.1.1",
            "http://[fd00::1]:8080",
            "http://[fe80::1]",
            "http://ollama:11434/v1",
            "http://box.local:8080",
            "http://gpu.lan",
            "http://x.internal",
            "http://h.home.arpa",
        ] {
            assert!(is_local_endpoint(u), "{u} should be local");
        }
    }

    #[test]
    fn net_class_public_and_lookalike_hosts_are_not_local() {
        for u in [
            "https://openrouter.ai/api/v1",
            "https://api.anthropic.com",
            "http://localhost.evil.com",
            "http://8.8.8.8",
            "http://[2001:4860::8888]",
            "not a url",
            "",
        ] {
            assert!(!is_local_endpoint(u), "{u:?} should not be local");
        }
    }
}
