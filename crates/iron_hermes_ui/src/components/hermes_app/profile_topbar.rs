//! The two root-provided profile contexts shared by the header and the
//! screens.
//!
//! History: Phase 49.4 Plan 11 (D-19) put the active-profile indicator and
//! chat-scope quick switch (`TopbarProfileSwitch`) here. Phase 54.4 Plan 05
//! retired that component: the header's Agent dropdown
//! (`header/agent_dropdown.rs`) is now the single profile switcher (49.4 D-19:
//! never two profile switchers), and this file only defines the two contexts
//! below.
//!
//! # Refresh signal — provided at the root, never here
//!
//! A successful activation bumps `TopbarProfileRefreshCtx`, a `Signal<u32>`
//! provided at the `HermesApp` root (`hermes_app/mod.rs`) — never inside this
//! module. The header's Agent dropdown and the Soul page's own activation
//! surface are siblings under that root, not ancestor/descendant, so a
//! child-level provider would panic whichever consumer didn't provide it
//! (Dioxus context-panic rule, `crates/iron_hermes_ui/CLAUDE.md`). Both
//! surfaces call the identical `activate_profile` server fn, which writes the
//! one persisted `Config.active_profile` record every reader resolves
//! through, so they can never disagree about which profile is active.

use crate::protocol::ProfileRow;
use dioxus::prelude::*;

/// Phase 49.4 Plan 11 (D-19): shared refresh signal so the header's agent
/// dropdown and any other consumer (the Soul page's activation surface)
/// re-derive from the same persisted record after either surface activates a
/// profile. MUST be provided at the `HermesApp` root — see this module's doc
/// comment for the context-panic rationale.
#[allow(dead_code)] // field read via `.0` at the one provide site (mod.rs) and the consume sites — dead_code fires on isolated compilation units, same class as AuthedContext in mod.rs
#[derive(Clone, Copy)]
pub struct TopbarProfileRefreshCtx(pub Signal<u32>);

/// Phase 49.4 hotfix: the ONE root-provided `list_profiles` resource every
/// boot-path consumer reads (the header's agent dropdown, @mention roster,
/// gateway scope selector) instead of each mounting its own. Each
/// `list_profiles` call does a full `Config::load_from` per profile, so
/// mounting several at once on boot saturated the WASM client and froze the
/// UI. Provided at the `HermesApp` root and refreshed by the same
/// [`TopbarProfileRefreshCtx`] tick, so a profile activation still re-derives
/// the list for every reader.
#[allow(dead_code)] // read via `.0` at the provide site (mod.rs) and consume sites — dead_code fires on isolated compilation units
#[derive(Clone, Copy)]
pub struct SharedProfilesCtx(pub Resource<Result<Vec<ProfileRow>, ServerFnError>>);

#[cfg(test)]
mod tests {
    const PROFILE_TOPBAR_RS: &str = include_str!("profile_topbar.rs");
    const SITE_CSS: &str = include_str!("../../../assets/site.css");

    #[test]
    fn topbar_switch_is_retired() {
        let code = &PROFILE_TOPBAR_RS[..PROFILE_TOPBAR_RS
            .find("#[cfg(test)]")
            .unwrap_or(PROFILE_TOPBAR_RS.len())];
        assert!(code.contains("pub struct SharedProfilesCtx"));
        assert!(code.contains("pub struct TopbarProfileRefreshCtx"));
        let component = ["pub fn ", "TopbarProfile", "Switch"].concat();
        assert!(
            !code.contains(&component),
            "the topbar profile switcher must be gone (49.4 D-19: one profile switcher)"
        );
        let css = ["topbar", "-profile"].concat();
        assert!(
            !SITE_CSS.contains(&css),
            "the retired switcher's `.{css}*` rules must be gone from site.css"
        );
    }
}
