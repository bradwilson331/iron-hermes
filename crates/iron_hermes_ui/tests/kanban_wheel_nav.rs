//! 36.3.7.11-04 — Wheel navigation Kanban wedge integration tests.
//!
//! `iron_hermes_ui` is a bin-only crate (no `[lib]` target), so integration
//! tests cannot `use iron_hermes_ui::*;` directly. This test follows the same
//! `include_str!` + source-text grep pattern used by
//! `tests/websocket_lifecycle_parity.rs` and `tests/web_queue_protocol.rs`.
//!
//! It locks the Risk 1 atomic 6-method update on `WheelWedge` plus the D-03
//! Agents-page `KANBAN BOARD →` button plus the wheel.rs 11-wedge wiring.
//!
//! Coverage map:
//!   - `WheelWedge::Kanban` variant present in state.rs.
//!   - `WheelWedge::Kanban => 10` (index match arm).
//!   - `WheelWedge::Kanban => "KANBAN"` (label).
//!   - `WheelWedge::Kanban => "TASK BOARD"` (sub).
//!   - `WheelWedge::Kanban => "▦"` or another non-existing-set glyph (glyph).
//!   - `WheelWedge::Kanban => Screen::Kanban` (to_screen).
//!   - `10 => WheelWedge::Kanban` (from_index match arm).
//!   - `rem_euclid(11)` present, `rem_euclid(10)` absent (Risk 1 atomic fix).
//!   - `Screen::Kanban` variant present.
//!   - wheel.rs has `pub const N: usize = 11;` (geometry update).
//!   - agents.rs has `KANBAN BOARD →` and `Screen::Kanban` onclick.
//!   - All 10 pre-existing variants still map to indices 0..=9 (no shuffle).

const STATE_SOURCE: &str = include_str!("../src/state.rs");
const WHEEL_SOURCE: &str = include_str!("../src/components/hermes_app/wheel.rs");
const AGENTS_SOURCE: &str = include_str!("../src/components/hermes_app/screens/agents.rs");
const ROUTER_SOURCE: &str = include_str!("../src/components/hermes_app/screen_router.rs");

// ─────────────────────────────────────────────────────────────────────────────
// state.rs — WheelWedge::Kanban + 6-method atomic update
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn state_rs_declares_wheelwedge_kanban_variant() {
    // Risk 1: the 6-method update must add the variant to the enum.
    // The enum literal `Kanban,` after `Settings,` in the WheelWedge enum.
    assert!(
        STATE_SOURCE.contains("Kanban"),
        "state.rs must declare WheelWedge::Kanban variant"
    );
}

#[test]
fn state_rs_index_method_maps_kanban_to_ten() {
    assert!(
        STATE_SOURCE.contains("WheelWedge::Kanban => 10"),
        "state.rs WheelWedge::index() must have arm `WheelWedge::Kanban => 10`"
    );
}

#[test]
fn state_rs_from_index_uses_modulo_eleven() {
    // Risk 1 atomic fix: rem_euclid(10) → rem_euclid(11).
    assert!(
        STATE_SOURCE.contains("rem_euclid(11)"),
        "state.rs WheelWedge::from_index() must use rem_euclid(11) (Risk 1 atomic fix)"
    );
    assert!(
        !STATE_SOURCE.contains("rem_euclid(10)"),
        "state.rs must NOT retain rem_euclid(10) — would silently route index 10 to Chat (Risk 1)"
    );
}

#[test]
fn state_rs_from_index_has_ten_to_kanban_arm() {
    assert!(
        STATE_SOURCE.contains("10 => WheelWedge::Kanban"),
        "state.rs WheelWedge::from_index() must have arm `10 => WheelWedge::Kanban`"
    );
}

#[test]
fn state_rs_label_kanban_is_kanban_uppercase() {
    assert!(
        STATE_SOURCE.contains("WheelWedge::Kanban => \"KANBAN\""),
        "state.rs WheelWedge::label() must return \"KANBAN\" for Kanban (UI-SPEC §3.3)"
    );
}

#[test]
fn state_rs_sub_kanban_is_task_board() {
    assert!(
        STATE_SOURCE.contains("WheelWedge::Kanban => \"TASK BOARD\""),
        "state.rs WheelWedge::sub() must return \"TASK BOARD\" for Kanban (UI-SPEC §3.3)"
    );
}

#[test]
fn state_rs_glyph_kanban_not_in_existing_set() {
    // UI-SPEC §3.3 suggests "▦" or "⬛". The glyph must NOT collide with the
    // existing 10-wedge glyph set ▓◆◇◈✦⬢▣◉⌬⚙.
    // We can't run the function, so we grep for the canonical "▦" glyph that
    // the discovery file recommended.
    assert!(
        STATE_SOURCE.contains("WheelWedge::Kanban => \"▦\"")
            || STATE_SOURCE.contains("WheelWedge::Kanban => \"⬛\""),
        "state.rs WheelWedge::glyph() must return \"▦\" or \"⬛\" for Kanban (UI-SPEC §3.3)"
    );
}

#[test]
fn state_rs_to_screen_kanban_to_screen_kanban() {
    assert!(
        STATE_SOURCE.contains("WheelWedge::Kanban => Screen::Kanban"),
        "state.rs WheelWedge::to_screen() must map Kanban → Screen::Kanban"
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
fn wheel_rs_constant_n_is_eleven() {
    // D-02: bumping pub const N: usize from 10 to 11. DYNAMIC geometry.
    assert!(
        WHEEL_SOURCE.contains("pub const N: usize = 11"),
        "wheel.rs must declare `pub const N: usize = 11` (D-02 11-wedge geometry)"
    );
    assert!(
        !WHEEL_SOURCE.contains("pub const N: usize = 10"),
        "wheel.rs must NOT retain `pub const N: usize = 10` — would render only 10 wedges"
    );
}

#[test]
fn wheel_rs_references_kanban() {
    // D-02 atomicity: the 11-wedge geometry update must at least reference
    // the new wedge somewhere (in a comment, documentation block, or test).
    assert!(
        WHEEL_SOURCE.contains("Kanban") || WHEEL_SOURCE.contains("kanban"),
        "wheel.rs must reference 'Kanban' or 'kanban' (proves the 11-wedge update reached this file)"
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
