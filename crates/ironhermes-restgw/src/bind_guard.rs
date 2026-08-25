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
