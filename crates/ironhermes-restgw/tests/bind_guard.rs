//! D-07 fail-closed bind guard — the four-cell predicate matrix
//! (loopback×unauthed, loopback×authed, non-loopback×unauthed,
//! non-loopback×authed). Pure function, no socket involved.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ironhermes_restgw::bind_guard::bind_guard_allows;

#[test]
fn loopback_ipv4_unauthed_allowed() {
    assert!(bind_guard_allows(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        false
    ));
}

#[test]
fn loopback_ipv4_authed_allowed() {
    assert!(bind_guard_allows(IpAddr::V4(Ipv4Addr::LOCALHOST), true));
}

#[test]
fn non_loopback_ipv4_unauthed_refused() {
    assert!(!bind_guard_allows(
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
        false
    ));
}

#[test]
fn non_loopback_ipv4_authed_allowed() {
    assert!(bind_guard_allows(
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
        true
    ));
}

#[test]
fn loopback_ipv6_unauthed_allowed() {
    assert!(bind_guard_allows(IpAddr::V6(Ipv6Addr::LOCALHOST), false));
}

#[test]
fn non_loopback_ipv6_unauthed_refused() {
    assert!(!bind_guard_allows(
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        false
    ));
}

#[test]
fn non_loopback_ipv6_authed_allowed() {
    assert!(bind_guard_allows(IpAddr::V6(Ipv6Addr::UNSPECIFIED), true));
}

/// D-07 falsifiable test, named exactly as PLAN.md's must_haves cites it:
/// the full four-cell matrix (loopback×unauthed, loopback×authed,
/// non-loopback×unauthed, non-loopback×authed) in one assertion group.
#[test]
fn bind_guard_allows_matrix() {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let non_loopback = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));

    assert!(bind_guard_allows(loopback, false), "loopback + unauthed");
    assert!(bind_guard_allows(loopback, true), "loopback + authed");
    assert!(
        !bind_guard_allows(non_loopback, false),
        "non-loopback + unauthed must be refused"
    );
    assert!(
        bind_guard_allows(non_loopback, true),
        "non-loopback + authed allowed"
    );
}
