//! Phase 27.1.4.2 static-grep regression gates.
//! Locks the correct CMD_LED_MOD#0\n wire command for hexapod led_off (PROV-HEXAPOD).
//! Follows `include_str!` pattern from invariants_22_4.rs. No dev-deps.

const HEXAPOD_TCP_SOURCE: &str = include_str!("../src/hexapod_tcp.rs");

#[test]
fn hexapod_tcp_uses_cmd_led_mod_for_led_off() {
    assert!(
        HEXAPOD_TCP_SOURCE.contains("CMD_LED_MOD#0\\n"),
        "PROV-HEXAPOD: crates/ironhermes-tools/src/hexapod_tcp.rs must contain \
         CMD_LED_MOD#0\\n as the led_off wire command. The Freenove server's \
         CMD_LED_MOD handler (led.py) is the correct off path — CMD_LED is the \
         color channel and silently ignores a single-field argument. \
         See phase 27.1.4.2."
    );
}

#[test]
fn hexapod_tcp_does_not_use_cmd_led_plain_for_led_off() {
    assert!(
        !HEXAPOD_TCP_SOURCE.contains("CMD_LED#0\\n"),
        "PROV-HEXAPOD: crates/ironhermes-tools/src/hexapod_tcp.rs must NOT contain \
         CMD_LED#0\\n — that constant was the wrong off command (color channel, not \
         mode channel). A refactor must not silently revert to it. \
         See phase 27.1.4.2."
    );
}
