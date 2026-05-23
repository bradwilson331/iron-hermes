//! Phase 27.1.4.2 static-grep regression gates.
//! Locks the correct CMD_LED#0#0#0\n wire command for hexapod led_off (PROV-HEXAPOD).
//! CMD_LED_MOD#0 is explicitly prohibited: its 350 ms colorWipe is corrupted by stop_thread.
//! Follows `include_str!` pattern from invariants_22_4.rs. No dev-deps.

const HEXAPOD_TCP_SOURCE: &str = include_str!("../src/hexapod_tcp.rs");

#[test]
fn hexapod_tcp_uses_cmd_led_color_zero_for_led_off() {
    assert!(
        HEXAPOD_TCP_SOURCE.contains("CMD_LED#0#0#0\\n"),
        "PROV-HEXAPOD: crates/ironhermes-tools/src/hexapod_tcp.rs must contain \
         CMD_LED#0#0#0\\n as the led_off wire command. This uses the fast ledIndex \
         path (microseconds) which cannot be interrupted by stop_thread. \
         See phase 27.1.4.2."
    );
}

#[test]
fn hexapod_tcp_does_not_use_cmd_led_mod_for_led_off() {
    assert!(
        !HEXAPOD_TCP_SOURCE.contains("CMD_LED_MOD#0\\n"),
        "PROV-HEXAPOD: crates/ironhermes-tools/src/hexapod_tcp.rs must NOT contain \
         CMD_LED_MOD#0\\n — that command triggers a 350 ms pixel-by-pixel colorWipe \
         which stop_thread async-raises SystemExit into mid-write, leaving LEDs in a \
         corrupted state. A refactor must not reintroduce it. \
         See phase 27.1.4.2."
    );
}
