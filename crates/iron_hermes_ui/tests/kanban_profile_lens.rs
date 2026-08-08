//! Phase 47.4 Plan 09 (D-05) — board profile-lens: proof that the lens
//! stays a pure client-side view transform, never a data-rescoping
//! operation.
//!
//! This file is the plan's own stated deliverable: "the lens is a pure
//! client-side view transform, and this plan proves it stays one." A
//! future change that made the switcher re-resolve the kanban DB per
//! profile, or that turned dimming into DOM removal / count filtering,
//! is designed to fail one of the tests below — the exact regression
//! class this project has already shipped once (a `--profile`-scoped
//! kanban side-DB that was empty in practice, blanking the board).
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot reach `pub(crate)`/private items — these
//! are source-string assertions over the real files, mirroring the
//! established pattern in `tests/profile_health.rs` / `tests/kanban_dnd_wiring.rs`
//! / `tests/assignee_picker.rs`.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

// ---------------------------------------------------------------------------
// D-05 prohibition: no client-side board/DB re-resolution
// ---------------------------------------------------------------------------

#[test]
fn no_kanban_client_screen_calls_open_from_env_or_board() {
    // The board DB resolution (`KanbanStore::open_from_env_or_board`) is a
    // server-only concern. If any client-side screen component ever calls
    // it directly, the lens has stopped being a pure client-side filter
    // and has become a real board-scoping mechanism — exactly what D-05
    // forbids.
    for path in [
        "src/components/hermes_app/screens/kanban.rs",
        "src/components/hermes_app/screens/kanban/board.rs",
        "src/components/hermes_app/screens/kanban/column.rs",
        "src/components/hermes_app/screens/kanban/card.rs",
    ] {
        let src = read(path);
        assert!(
            !src.contains("open_from_env_or_board"),
            "D-05: {path} must never call `open_from_env_or_board` — board \
             DB resolution stays server-only and profile-independent"
        );
    }
}

#[test]
fn fetch_board_signature_carries_no_profile_parameter() {
    // Extract just the `fetch_board` fn (from its `pub async fn` line to
    // the next `#[server]` boundary) so a comment mentioning "profile"
    // elsewhere in this large file (e.g. `current_profile` in an unrelated
    // doc comment) cannot produce a false positive/negative.
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains(
            "pub async fn fetch_board(\n    board: Option<String>,\n    include_archived: bool,\n)"
        ),
        "D-05: fetch_board's signature must remain exactly \
         (board: Option<String>, include_archived: bool) — no profile \
         parameter may be added; a profile-scoped board resolution would \
         open per-profile side-DBs that are empty in practice, blanking \
         the board on nearly every switch (the exact failure class this \
         project has already shipped once)"
    );
    let start = src
        .find("pub async fn fetch_board(")
        .expect("fetch_board must exist in kanban_api.rs");
    let next_server_fn = src[start..]
        .find("#[server]")
        .map(|i| start + i)
        .unwrap_or(src.len());
    let fetch_board_body = &src[start..next_server_fn];
    assert!(
        !fetch_board_body.to_lowercase().contains("profile"),
        "D-05: fetch_board's body must never reference a profile concept — \
         board and profile are separate axes in this codebase"
    );
}

#[test]
fn board_resource_fetch_closure_never_reads_active_profile() {
    // The lens (`active_profile`) must never drive board_resource's fetch
    // closure — that would turn a client-side highlight into a real
    // re-fetch/re-scope trigger, which is precisely what D-05 forbids.
    let src = read("src/components/hermes_app/screens/kanban.rs");
    let start = src
        .find("let mut board_resource = use_resource")
        .expect("board_resource must be declared in kanban.rs");
    let end = src[start..]
        .find("});")
        .map(|i| start + i)
        .expect("board_resource's use_resource closure must close with `});`");
    let resource_span = &src[start..end];
    assert!(
        !resource_span.contains("active_profile"),
        "D-05: board_resource's fetch closure must never reference \
         active_profile — the lens is a pure client-side view transform \
         over already-fetched data, never a trigger for a new/rescoped \
         fetch"
    );
    assert!(
        resource_span.contains("fetch_board(None,"),
        "D-05: board_resource must still call fetch_board(None, ...) — \
         the default board, unconditioned by any profile selection"
    );
}

// ---------------------------------------------------------------------------
// D-05 positive contract: dimming, not removal; true totals, not filtered
// ---------------------------------------------------------------------------

#[test]
fn card_renders_lens_dimmed_attribute_exactly_once() {
    let src = read("src/components/hermes_app/screens/kanban/card.rs");
    assert_eq!(
        src.matches("data-lens-dimmed").count(),
        1,
        "D-05: card.rs must declare the data-lens-dimmed attribute exactly \
         once — a duplicate declaration or a second divergent \
         implementation would drift from the single source of truth"
    );
    assert!(
        src.contains("lens_dimmed: bool"),
        "D-05: KanbanCard must accept a lens_dimmed: bool prop"
    );
}

#[test]
fn column_never_filters_or_retains_the_card_iteration() {
    // The column's `for task in filtered.into_iter()` loop and its
    // `.kn-column-count` derivation must stay untouched by the lens — the
    // column may only compute a per-card dimmed flag and pass it through.
    let src = read("src/components/hermes_app/screens/kanban/column.rs");
    assert!(
        !src.contains(".filter(|t| t.assignee"),
        "D-05: column.rs must never filter the card iteration by \
         assignee/lens — dimming happens via a per-card boolean prop, \
         never by removing cards from the render list"
    );
    // The column-count span must still derive from the unfiltered
    // `filtered.len()` value, not from any lens-aware count.
    let count_idx = src
        .find("span { class: \"kn-column-count\"")
        .expect("kn-column-count span must exist");
    let count_line = &src[count_idx..count_idx + 80];
    assert!(
        count_line.contains("{count}"),
        "D-05: .kn-column-count must keep rendering the unfiltered `count` \
         value — filtering it would contradict D-05's board/profile \
         separate-axes framing"
    );
}

#[test]
fn lens_dimmed_derivation_is_a_pure_comparison_no_io() {
    let src = read("src/components/hermes_app/screens/kanban/column.rs");
    assert!(
        src.contains("task.assignee != name"),
        "D-05: lens_dimmed must be derived via a plain string comparison \
         over already-fetched task data — no server call, no store lookup"
    );
    assert!(
        !src.contains("spawn(") || !src.contains("lens_dimmed"),
        "D-05: the lens_dimmed derivation must not be wrapped in an async \
         spawn — it is a synchronous, render-time comparison only"
    );
}

// ---------------------------------------------------------------------------
// D-05 + D-12: lens-seeded, still-editable default assignee
// ---------------------------------------------------------------------------

#[test]
fn create_task_modal_seeds_but_never_constrains_the_assignee() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("initial_assignee: Option<String>"),
        "D-05/D-12: CreateTaskModal must accept an initial_assignee: \
         Option<String> prop to seed the lens-selected profile"
    );
    assert!(
        src.contains("let mut assignee: Signal<String> ="),
        "D-05/D-12: the assignee field must remain a plain Signal<String> \
         — seeding an initial value must never coerce it into a select \
         or other constrained control"
    );
    assert!(
        src.contains("oninput: move |evt| assignee.set(evt.value())"),
        "D-05/D-12: the assignee <input>'s oninput handler must remain \
         unchanged — the seeded value stays freely editable"
    );
}

#[test]
fn kanban_screen_seeds_create_task_modal_from_the_active_lens() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    assert!(
        src.contains("initial_assignee: active_profile.read().clone()"),
        "D-05/D-12: kanban.rs must pass the active lens profile as \
         CreateTaskModal's initial_assignee — a card created while a \
         lens is active must be pre-filled with that profile name"
    );
}

// ---------------------------------------------------------------------------
// Toolbar lens indicator (E7/zero-one-many, E7/empty)
// ---------------------------------------------------------------------------

#[test]
fn lens_indicator_template_is_declared_once_and_absent_when_no_lens() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    assert_eq!(
        src.matches("kn-lens-indicator").count(),
        1,
        "D-05: kanban.rs must render the kn-lens-indicator class exactly \
         once"
    );
    assert_eq!(
        src.matches("Showing {matching} of {total} for {name}")
            .count(),
        1,
        "E7/zero-one-many: the indicator template must cover 0, 1, and \
         many matches with one template — no special-casing"
    );
    assert!(
        src.contains("active_profile.read().clone().map(|name| {"),
        "D-05: the indicator must be None (not rendered) when the lens is \
         ALL PROFILES — computed via .map() over the Option, not a \
         separate branch that could diverge"
    );
}
