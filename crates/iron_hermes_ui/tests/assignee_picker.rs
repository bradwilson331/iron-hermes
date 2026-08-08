//! Phase 47.4 Plan 04 Task 3 — `AssigneePicker` shape locks.
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot reach `pub(crate)` items in
//! `modals.rs`/`drawer.rs` (see `tests/profile_health.rs`'s identical,
//! already-established note from Plan 01). The real behavioral tests —
//! the assignee-unmatched predicate and the hint-line renderer, including
//! the required mutation check — live as `#[cfg(test)]` unit tests inside
//! `src/components/hermes_app/screens/kanban/modals.rs` itself (`mod
//! assignee_picker_tests`). Run them with:
//!
//!   cargo nextest run -p iron_hermes_ui --features server assignee_picker --test-threads=1
//!
//! This file locks the module's *shape* and the D-12 negative guarantee
//! (nothing filters the typed assignee value against the known-profile
//! set) via source-string assertions, mirroring `tests/profile_health.rs`
//! / `tests/provider_config_api.rs`.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn modals_declares_the_datalist_id_const() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("pub(crate) const KN_ASSIGNEE_DATALIST_ID"),
        "modals.rs must declare `KN_ASSIGNEE_DATALIST_ID`"
    );
}

#[test]
fn modals_declares_the_pure_hint_and_unmatched_fns() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    for name in ["fn assignee_hint_text(", "fn is_assignee_unmatched("] {
        assert!(
            src.contains(name),
            "modals.rs must declare `{name}` as a pure, unit-testable fn"
        );
    }
}

#[test]
fn modals_never_calls_restart_on_the_profiles_resource() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert_eq!(
        src.matches("restart()").count(),
        0,
        "modals.rs must never call .restart() on the profiles resource — \
         this breaks hook ordering per providers.rs's documented hazard"
    );
}

#[test]
fn assignee_input_still_a_plain_string_signal_no_select_filter() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    // Phase 47.4 Plan 09 (D-05): the initializer now seeds from
    // `initial_assignee` (the board PROFILE lens) instead of a bare
    // `String::new()` literal — the type and free-text semantics this
    // assertion protects are unchanged; only the exact literal moved.
    assert!(
        src.contains("let mut assignee: Signal<String> ="),
        "D-12: the assignee field's type must remain Signal<String>, unchanged"
    );
    assert!(
        src.contains("initial_assignee.clone().unwrap_or_default()"),
        "D-05 (Plan 09): the assignee signal must seed from initial_assignee, \
         still producing an empty string when no lens is active"
    );
    // D-12 negative case: nothing may filter the typed value against the
    // known-profile set on the submit path.
    let payload_start = src
        .find("let payload = CreateTaskPayload")
        .expect("CreateTaskModal must build a CreateTaskPayload on submit");
    let submit_end = src[payload_start..]
        .find("spawn(async move")
        .map(|i| payload_start + i)
        .expect("payload construction must be followed by the submit spawn");
    let payload_span = &src[payload_start..submit_end];
    assert!(
        payload_span.contains("assignee.read().clone()"),
        "D-12: the submit payload must carry the typed assignee value verbatim"
    );
    for forbidden in ["profile_names_sig.read().contains", "filter(", ".retain("] {
        assert!(
            !payload_span.contains(forbidden),
            "D-12: the submit path must not filter the typed assignee value \
             against the known-profile set (found `{forbidden}`)"
        );
    }
}

#[test]
fn modals_datalist_id_referenced_at_least_three_times() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.matches("KN_ASSIGNEE_DATALIST_ID").count() >= 3,
        "KN_ASSIGNEE_DATALIST_ID must appear at least 3 times: const decl, \
         `list:` attribute, and the datalist `id`"
    );
}

#[test]
fn modals_registers_datalist_element() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("datalist {"),
        "CreateTaskModal must render a sibling `datalist` element"
    );
}

#[test]
fn drawer_declares_the_unmatched_suffix_and_marker_class_exactly_once() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    assert_eq!(
        src.matches("unmatched profile").count(),
        1,
        "drawer.rs must render the literal `unmatched profile` suffix exactly once"
    );
    assert_eq!(
        src.matches("kn-assignee-unmatched").count(),
        1,
        "drawer.rs must reference `kn-assignee-unmatched` exactly once"
    );
}

#[test]
fn drawer_never_writes_back_to_the_kanban_store_for_the_marker() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    for forbidden in ["update_task(", "set_assignee(", "KanbanStore"] {
        assert!(
            !src.contains(forbidden),
            "D-12: drawer.rs must never write back to the kanban store \
             (found `{forbidden}`) — the unmatched marker is render-time only"
        );
    }
}
