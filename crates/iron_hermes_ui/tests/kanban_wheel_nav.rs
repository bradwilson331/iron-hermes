//! Wheel navigation vs Kanban — source-text integration tests.
//!
//! `iron_hermes_ui` is a bin-only crate (no `[lib]` target), so integration
//! tests cannot `use iron_hermes_ui::*;` directly. This test follows the same
//! `include_str!` + source-text grep pattern used by
//! `tests/websocket_lifecycle_parity.rs` and `tests/web_queue_protocol.rs`.
//!
//! Phase 54.4: the former 11th `Kanban` wheel wedge was removed (the
//! persistent header's Kanban shortcut button covers it). `Screen::Kanban`
//! is unchanged. These tests lock the new truth.
//!
//! Coverage map:
//!   - `WheelWedge` has NO `Kanban` variant / arms; the wheel has 10 wedges.
//!   - `from_index` uses `rem_euclid(10)` (a stale index 10 wraps to Chat).
//!   - `Screen::Kanban` variant present and routed by the screen router.
//!   - header.rs still ships the Kanban shortcut button routing to Screen::Kanban.
//!   - wheel.rs has `pub const N: usize = 10;`.
//!   - agents.rs has `KANBAN BOARD →` and `Screen::Kanban` onclick.
//!   - The 10 remaining variants still map to indices 0..=9 (no shuffle).

const STATE_SOURCE: &str = include_str!("../src/state.rs");
const WHEEL_SOURCE: &str = include_str!("../src/components/hermes_app/wheel.rs");
const AGENTS_SOURCE: &str = include_str!("../src/components/hermes_app/screens/agents.rs");
const HEADER_SOURCE: &str = include_str!("../src/components/hermes_app/header.rs");
const ROUTER_SOURCE: &str = include_str!("../src/components/hermes_app/screen_router.rs");

// ─────────────────────────────────────────────────────────────────────────────
// state.rs — WheelWedge has no Kanban wedge (Phase 54.4)
// ─────────────────────────────────────────────────────────────────────────────

/// The `WheelWedge` enum body plus its impl (everything up to `WheelState`).
fn wheel_wedge_source() -> &'static str {
    let start = STATE_SOURCE
        .find("pub enum WheelWedge")
        .expect("WheelWedge enum declaration must exist");
    let end = STATE_SOURCE
        .find("pub struct WheelState")
        .expect("WheelState declaration must exist");
    &STATE_SOURCE[start..end]
}

#[test]
fn state_rs_wheelwedge_has_no_kanban_variant() {
    assert!(
        !wheel_wedge_source().contains("WheelWedge::Kanban"),
        "WheelWedge must not reference a Kanban wedge (removed in Phase 54.4)"
    );
    let enum_start = STATE_SOURCE.find("pub enum WheelWedge").unwrap();
    let enum_body_end = STATE_SOURCE[enum_start..].find('}').unwrap();
    assert!(
        !STATE_SOURCE[enum_start..enum_start + enum_body_end].contains("Kanban"),
        "WheelWedge enum body must not declare a Kanban variant"
    );
}

#[test]
fn state_rs_from_index_uses_modulo_ten() {
    // 10 wedges: a stale index 10 must wrap safely to Chat, never panic.
    assert!(
        STATE_SOURCE.contains("rem_euclid(10)"),
        "state.rs WheelWedge::from_index() must use rem_euclid(10)"
    );
    assert!(
        !STATE_SOURCE.contains("rem_euclid(11)"),
        "state.rs must NOT retain rem_euclid(11) (the wheel has 10 wedges)"
    );
    assert!(
        !wheel_wedge_source().contains("10 => WheelWedge"),
        "from_index() must have no arm for index 10 (it wraps to Chat)"
    );
}

#[test]
fn state_rs_stale_wheel_index_fallback_is_unit_tested() {
    // The behavioural check lives in state.rs's unit tests; lock its presence.
    assert!(
        STATE_SOURCE.contains("assert_eq!(WheelWedge::from_index(10), WheelWedge::Chat)"),
        "state.rs must unit-test that stale index 10 falls back to Chat"
    );
    assert!(
        STATE_SOURCE.contains("wheel_state_stale_kanban_active_wedge_falls_back_to_chat"),
        "state.rs must unit-test that a stale persisted Kanban active_wedge parses as Chat"
    );
}

#[test]
fn state_rs_screen_enum_has_kanban_variant() {
    // Plan 04 ships first; Screen::Kanban must be added.
    // Grep for the Screen enum body and then the Kanban arm in screen_label etc.
    // Quick check: enum body contains "Kanban," somewhere after `pub enum Screen`.
    let needle = "pub enum Screen";
    let body_start = STATE_SOURCE
        .find(needle)
        .expect("Screen enum declaration must exist");
    let after = &STATE_SOURCE[body_start..];
    let body_end = after.find('}').expect("Screen enum body must close");
    let body = &after[..body_end];
    assert!(
        body.contains("Kanban"),
        "Screen enum body must contain Kanban variant — got body: {body}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Pre-existing wedges unchanged — Risk 1 no-shuffle guard
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn state_rs_pre_existing_index_arms_unchanged() {
    // No wedge other than Kanban changed its index in the 6-method update.
    let needles = [
        ("WheelWedge::Chat => 0", "Chat"),
        ("WheelWedge::Agents => 1", "Agents"),
        ("WheelWedge::Models => 2", "Models"),
        ("WheelWedge::Tools => 3", "Tools"),
        ("WheelWedge::Skills => 4", "Skills"),
        ("WheelWedge::Memory => 5", "Memory"),
        ("WheelWedge::Sessions => 6", "Sessions"),
        ("WheelWedge::Providers => 7", "Providers"),
        ("WheelWedge::Gateway => 8", "Gateway"),
        ("WheelWedge::Settings => 9", "Settings"),
    ];
    for (needle, name) in needles {
        assert!(
            STATE_SOURCE.contains(needle),
            "state.rs WheelWedge::index() arm for {name} must still map to its original index — needle '{needle}' missing"
        );
    }
}

#[test]
fn state_rs_pre_existing_from_index_arms_unchanged() {
    let needles = [
        "0 => WheelWedge::Chat",
        "1 => WheelWedge::Agents",
        "2 => WheelWedge::Models",
        "3 => WheelWedge::Tools",
        "4 => WheelWedge::Skills",
        "5 => WheelWedge::Memory",
        "6 => WheelWedge::Sessions",
        "7 => WheelWedge::Providers",
        "8 => WheelWedge::Gateway",
        "9 => WheelWedge::Settings",
    ];
    for needle in needles {
        assert!(
            STATE_SOURCE.contains(needle),
            "state.rs from_index() arm '{needle}' must remain unchanged"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// wheel.rs — geometry update (N = 11)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn wheel_rs_constant_n_is_ten() {
    assert!(
        WHEEL_SOURCE.contains("pub const N: usize = 10;"),
        "wheel.rs must declare `pub const N: usize = 10` (10-wedge geometry)"
    );
    assert!(
        !WHEEL_SOURCE.contains("pub const N: usize = 11"),
        "wheel.rs must NOT retain `pub const N: usize = 11` — the Kanban wedge was removed"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// header.rs — Kanban stays reachable via the header shortcut button
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn header_rs_kanban_shortcut_routes_to_kanban_screen() {
    assert!(
        HEADER_SOURCE.contains("(Screen::Kanban, \"Kanban\")"),
        "header.rs HEADER_SHORTCUTS must keep the Kanban shortcut now that the wedge is gone"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// agents.rs — D-03 KANBAN BOARD button
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn agents_rs_has_kanban_board_button_label() {
    // D-03: button labeled "KANBAN BOARD →" inside the Agents toolbar.
    assert!(
        AGENTS_SOURCE.contains("KANBAN BOARD"),
        "agents.rs must contain the literal \"KANBAN BOARD\" button label (D-03)"
    );
    // Arrow → is part of UI-SPEC §7.1 — exact label.
    assert!(
        AGENTS_SOURCE.contains("KANBAN BOARD →"),
        "agents.rs button label must match UI-SPEC §7.1 exactly: \"KANBAN BOARD →\""
    );
}

#[test]
fn agents_rs_kanban_button_routes_to_kanban_screen() {
    // D-03: onclick must call `active_screen.set(Screen::Kanban)`.
    assert!(
        AGENTS_SOURCE.contains("Screen::Kanban"),
        "agents.rs must reference Screen::Kanban in the KANBAN BOARD button's onclick handler"
    );
}

#[test]
fn agents_rs_kanban_button_appears_after_prune_ended() {
    // PATTERNS.md / UI-SPEC §3.2: insertion point is immediately after
    // PRUNE ENDED inside `div { class: "screen-actions" }`.
    let prune_idx = AGENTS_SOURCE
        .find("PRUNE ENDED")
        .expect("agents.rs must contain PRUNE ENDED");
    let kanban_idx = AGENTS_SOURCE
        .find("KANBAN BOARD")
        .expect("agents.rs must contain KANBAN BOARD");
    assert!(
        kanban_idx > prune_idx,
        "KANBAN BOARD button must appear AFTER PRUNE ENDED in agents.rs source order (UI-SPEC §3.2)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// screen_router.rs — placeholder ScreenKanban mount
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn screen_router_mounts_kanban_placeholder() {
    // Plan 04 placeholder: a minimal `screens::kanban::ScreenKanban`
    // module exists and is mounted by the router. Plan 01 replaces it with
    // the live board.
    assert!(
        ROUTER_SOURCE.contains("Screen::Kanban"),
        "screen_router.rs must reference Screen::Kanban in its render fan-out"
    );
    assert!(
        ROUTER_SOURCE.contains("screens::kanban::ScreenKanban"),
        "screen_router.rs must mount screens::kanban::ScreenKanban (Plan 04 placeholder)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// wheel.css — wedge focus style (no UA/outline square; themed focus-visible)
// ─────────────────────────────────────────────────────────────────────────────

const WHEEL_CSS: &str = include_str!("../assets/wheel.css");

/// Body of the first CSS rule whose selector list contains `selector`.
fn css_rule_body(selector: &str) -> &'static str {
    let at = WHEEL_CSS
        .find(selector)
        .unwrap_or_else(|| panic!("wheel.css must contain `{selector}`"));
    let open = at + WHEEL_CSS[at..].find('{').expect("rule must open a block");
    let close = open + WHEEL_CSS[open..].find('}').expect("rule must close");
    &WHEEL_CSS[open + 1..close]
}

#[test]
fn wheel_wedge_focus_has_no_default_outline_square() {
    // An outline on an SVG <path> paints its bounding rectangle (the white
    // square). Plain :focus (mouse click) must have no outline.
    let focus = css_rule_body(".wheel-shell--docked .wheel-wedge:focus {");
    assert!(
        focus.contains("outline: none"),
        "wedge :focus must set outline: none, got: {focus}"
    );
}

#[test]
fn wheel_wedge_focus_visible_is_themed_not_an_outline() {
    let fv = css_rule_body(".wheel-shell--docked .wheel-wedge:focus-visible");
    assert!(
        fv.contains("outline: none"),
        "focus-visible must not draw a box outline: {fv}"
    );
    assert!(
        fv.contains("var(--wheel-accent)") && fv.contains("stroke"),
        "focus-visible must show the accent stroke (--wheel-accent, derived from --teal) on the wedge itself: {fv}"
    );
    assert!(fv.contains("fill:"), "focus-visible must highlight the wedge fill: {fv}");
}
