//! Behavioral tests for Phase 22.4.2 Plan 03 toggle handlers.
//!
//! Tests App-side Arc<AtomicBool> and Arc<RwLock<String>> mutations directly —
//! simulating what handle_toggle() does to App fields. Requires `test-support`
//! feature for App::new_test_empty().
//!
//! PATTERNS.md Cat-5D: App toggle test pattern using App::new_test_empty().

#![cfg(feature = "test-support")]

use ironhermes_cli::tui_rata::app::App;
use std::sync::atomic::Ordering;

/// INV companion: yolo_enabled is Arc<AtomicBool> (D-09 upgrade from bool).
/// fetch_xor toggles the bit; calling it twice restores original state.
#[test]
fn yolo_toggle_flips_atomic_bool() {
    let app = App::new_test_empty();
    let initial = app.yolo_enabled.load(Ordering::SeqCst);
    // Simulate handle_toggle(app, "yolo", "") — fetch_xor
    let old = app.yolo_enabled.fetch_xor(true, Ordering::SeqCst);
    assert_eq!(old, initial, "fetch_xor returns the previous value");
    assert_ne!(
        app.yolo_enabled.load(Ordering::SeqCst),
        initial,
        "bit flipped"
    );
    // Second toggle restores original
    app.yolo_enabled.fetch_xor(true, Ordering::SeqCst);
    assert_eq!(
        app.yolo_enabled.load(Ordering::SeqCst),
        initial,
        "second flip restores"
    );
}

/// verbose_enabled starts false; toggling sets to true.
#[test]
fn verbose_toggle_starts_false_and_flips_to_true() {
    let app = App::new_test_empty();
    assert!(
        !app.verbose_enabled.load(Ordering::SeqCst),
        "verbose starts false"
    );
    app.verbose_enabled.fetch_xor(true, Ordering::SeqCst);
    assert!(
        app.verbose_enabled.load(Ordering::SeqCst),
        "verbose now true"
    );
}

/// statusbar_enabled starts true (D-09: initial value true).
#[test]
fn statusbar_default_is_true() {
    let app = App::new_test_empty();
    assert!(
        app.statusbar_enabled.load(Ordering::SeqCst),
        "statusbar_enabled must be true by default (D-09)"
    );
}

/// debug_enabled starts false; toggling flips it.
#[test]
fn debug_toggle_flips() {
    let app = App::new_test_empty();
    let initial = app.debug_enabled.load(Ordering::SeqCst);
    app.debug_enabled.fetch_xor(true, Ordering::SeqCst);
    assert_ne!(
        app.debug_enabled.load(Ordering::SeqCst),
        initial,
        "debug_enabled must flip"
    );
}

/// skin field is Arc<RwLock<String>>; write then read round-trips correctly.
/// T-22.4.2-03-07: poison recovery via unwrap_or_else(|p| p.into_inner()).
#[test]
fn skin_write_then_read_round_trip() {
    let app = App::new_test_empty();
    {
        let mut w = app.skin.write().unwrap_or_else(|p| p.into_inner());
        *w = "minimal".to_string();
    }
    let r = app.skin.read().unwrap_or_else(|p| p.into_inner());
    assert_eq!(*r, "minimal", "skin value must round-trip through RwLock");
}

/// fast_enabled starts false; toggling twice restores original state.
#[test]
fn fast_toggle_round_trips() {
    let app = App::new_test_empty();
    assert!(
        !app.fast_enabled.load(Ordering::SeqCst),
        "fast starts false"
    );
    app.fast_enabled.fetch_xor(true, Ordering::SeqCst);
    assert!(app.fast_enabled.load(Ordering::SeqCst), "fast now true");
    app.fast_enabled.fetch_xor(true, Ordering::SeqCst);
    assert!(
        !app.fast_enabled.load(Ordering::SeqCst),
        "fast restored to false"
    );
}
