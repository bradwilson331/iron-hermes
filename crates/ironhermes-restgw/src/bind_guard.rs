//! D-07 fail-closed bind guard, shared by every `ironhermes-restgw` adapter
//! (the webhook adapter today; the REST API server adapter in a later
//! plan). A direct port of the Phase 47.3 D-10 predicate at
//! `crates/iron_hermes_ui/src/main.rs:25-27` — pure, no I/O, so it is
//! unit-testable without a socket.
//!
//! Both adapters call this BEFORE `TcpListener::bind` and return `Err`
//! (never panic — see this crate's `webhook` module for why a refused
//! webhook adapter must not take down the whole gateway process) when it
//! returns `false`. A refused configuration must never open a socket.

use std::net::IpAddr;

/// Returns `true` when `ip` is loopback OR `auth_enabled` is `true`.
///
/// `ip.is_loopback()` covers both IPv4 `127.0.0.0/8` and IPv6 `::1`.
pub fn bind_guard_allows(ip: IpAddr, auth_enabled: bool) -> bool {
    ip.is_loopback() || auth_enabled
}

/// Finding 7 (D-20): a SECOND, independent fail-closed startup guard for the
/// REST API server adapter — a sibling of [`bind_guard_allows`] above, not a
/// widened version of it. Direct precedent: `iron_hermes_ui`'s own second
/// guard, `origin_guard_allows` (`crates/iron_hermes_ui/src/main.rs:44-46`),
/// added alongside its first D-10 credential guard for the identical reason
/// — "is a wildcard cross-origin value configured" is logically independent
/// of "is loopback/auth configured", so it is a sibling predicate rather
/// than a third argument threaded through `bind_guard_allows`.
///
/// **POLARITY INVERSION — read this before touching either guard.**
/// [`bind_guard_allows`] and `iron_hermes_ui`'s `origin_guard_allows` both
/// refuse when a PROTECTION is ABSENT (no auth configured / no origin
/// allowlist configured) on an exposed bind. This guard refuses in the
/// OPPOSITE case: when a PERMISSION — a wildcard cross-origin value — is
/// PRESENT on an exposed bind. A reader scanning three similarly-shaped
/// `fn(IpAddr, bool) -> bool` predicates in this codebase will otherwise
/// assume they all read the same direction, which is exactly how an
/// inverted guard ships broken (a copy-pasted `!` in the wrong place either
/// silently disables it or silently refuses the loopback case that must
/// stay allowed).
///
/// Allowed when the bind is loopback OR no wildcard cross-origin value is
/// configured. Refused ONLY for the non-loopback-with-wildcard combination
/// — today's normal non-loopback-without-wildcard posture is unaffected
/// and must not regress.
///
/// Pure and side-effect-free — no I/O, no logging, no panic — matching this
/// module's stated fail-closed pre-bind contract (see the module doc
/// above): a refused configuration must never open a socket, and the
/// predicate itself never blocks that promise.
pub fn cors_wildcard_guard_allows(ip: IpAddr, wildcard_configured: bool) -> bool {
    ip.is_loopback() || !wildcard_configured
}
