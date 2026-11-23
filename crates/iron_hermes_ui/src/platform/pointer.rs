//! Pointer capture for drag handles (Phase 54.10 Plan 02, RESEARCH Pitfall 7).
//!
//! Dioxus `PointerData` exposes the pointer id but has no capture API, so a drag handle
//! captures through the raw DOM element behind its `onmounted` data. A captured drag cannot
//! lose the item or leave a stuck drag state when the pointer leaves the handle (UI-SPEC A3).
//! The cfg gate sits inside the body so the public signature is identical on every platform
//! (same pattern as `timer::sleep`); native builds have no DOM and do nothing.

#![allow(dead_code)]

use dioxus::prelude::MountedData;

/// `Element.setPointerCapture(pointer_id)` on the mounted element. A refusal (stale pointer
/// id) is ignored: `pointercancel` and window-blur handling still end the drag.
pub fn capture_pointer(el: &MountedData, pointer_id: i32) {
    #[cfg(target_arch = "wasm32")]
    if let Some(e) = el.downcast::<web_sys::Element>() {
        let _ = e.set_pointer_capture(pointer_id);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (el, pointer_id);
}

/// `Element.releasePointerCapture(pointer_id)` on the mounted element; errors ignored as above.
pub fn release_pointer(el: &MountedData, pointer_id: i32) {
    #[cfg(target_arch = "wasm32")]
    if let Some(e) = el.downcast::<web_sys::Element>() {
        let _ = e.release_pointer_capture(pointer_id);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (el, pointer_id);
}
