//! Command palette / slash menu (Phase 36.6.3 Plan 01, TUI-INPUT-01, D-03/D-04/D-05).
//!
//! Deliberately NOT an `OverlayKind` variant (RESEARCH Pitfall 2) — the palette
//! is derived LIVE from `app.textarea`'s content every render/key-dispatch via
//! `palette_query`, never from a stored open/closed boolean or an
//! `active_overlay` entry. It floats a bottom-anchored `Rect` directly above
//! the input line (D-03), a deliberate divergence from `overlay.rs`'s
//! `centered_rect` popup recipe — a command palette should feel like
//! lightweight autocomplete, not a modal that covers the transcript.
//!
//! Rendered unconditionally as a 3rd call in `event_loop.rs`'s draw closure,
//! strictly AFTER `overlay::render` (so an active overlay always wins — the
//! palette self-gates via `palette_query` returning `None` whenever
//! `app.active_overlay.is_some()`).
//!
//! Filtering goes through a plain `Vec` filter over the already-public
//! `app.command_router.commands` — NOT a reuse/extension of
//! `CommandRouter::resolve` (a single-best-match resolver built for dispatch,
//! wrong shape for "list every candidate").

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::tui_rata::app::App;

/// Max visible dropdown rows before the overflow hint replaces the last
/// command row (UI-SPEC "Layout / Command palette").
pub(crate) const MAX_VISIBLE_ROWS: usize = 6;

/// Derives the live palette query from the textarea. `Some(prefix)` (the
/// token after the leading `/`, without the slash) when the input is exactly
/// `/{token}` with no space or newline typed yet; `None` otherwise (no
/// leading slash, multi-word input, or an active overlay). Pure — no `App`
/// mutation, matches `dispatch_or_submit`'s existing
/// `self.textarea.lines().join("\n")` accessor (app.rs).
///
/// This is the ENTIRE state model for "is the palette open" (D-03) — there is
/// no companion boolean field anywhere on `App` to keep in sync.
pub(crate) fn palette_query(app: &App) -> Option<String> {
    if app.active_overlay.is_some() {
        return None; // modal always wins
    }
    let text = app.textarea.lines().join("\n");
    if !text.starts_with('/') || text.contains(' ') || text.contains('\n') {
        return None;
    }
    Some(text[1..].to_string())
}

/// Prefix + `Platform::Local` filtered view over the command registry,
/// preserving registry (source) order — never sorted, never deduped beyond
/// what the registry itself guarantees. Excludes any command whose
/// `platform_filter` does not cover `Platform::Local` (RESEARCH Pitfall 4 —
/// `app.command_router.commands` carries gateway/ACP-only entries that
/// `CommandRouter::resolve` gates at dispatch time; a naive prefix filter
/// here would leak them into the LOCAL palette's list).
pub(crate) fn filtered_commands<'a>(
    app: &'a App,
    prefix: &str,
) -> Vec<&'a ironhermes_core::commands::CommandDef> {
    app.command_router
        .commands
        .iter()
        .filter(|c| {
            c.name.starts_with(prefix) && c.is_available_on(&ironhermes_core::types::Platform::Local)
        })
        .collect()
}

/// One row in the unified palette list (D-05): either a slash `CommandDef` or a
/// `SkillRegistry` entry. Skills carry only the two fields a row needs (name +
/// description), borrowed from `app.skill_registry` for the render's lifetime.
pub(crate) enum PaletteEntry<'a> {
    Command(&'a ironhermes_core::commands::CommandDef),
    Skill { name: &'a str, description: &'a str },
}

impl PaletteEntry<'_> {
    /// The command/skill name (no leading `/`) — the token Tab/Enter complete
    /// against and dispatch through the shared slash path.
    pub(crate) fn name(&self) -> &str {
        match self {
            PaletteEntry::Command(c) => c.name,
            PaletteEntry::Skill { name, .. } => name,
        }
    }
}

/// The unified prefix-filtered palette list (D-05): `filtered_commands` UNIONed
/// with `app.skill_registry` entries, one flat list. Commands come first (their
/// registry order, Platform::Local-gated as before), then matching skills in
/// registry order — a skill row on Enter fires the same one-shot activate+run
/// path a real command does (routed through `dispatch_or_submit` →
/// `dispatch_slash`'s SKILL-13 fallback → `apply_slash_outcome`, Plan 02 D-01).
pub(crate) fn filtered_entries<'a>(app: &'a App, prefix: &str) -> Vec<PaletteEntry<'a>> {
    let mut entries: Vec<PaletteEntry<'a>> = filtered_commands(app, prefix)
        .into_iter()
        .map(PaletteEntry::Command)
        .collect();
    if let Some(registry) = &app.skill_registry {
        for rec in registry.list() {
            if rec.name.starts_with(prefix) {
                entries.push(PaletteEntry::Skill {
                    name: &rec.name,
                    description: &rec.description,
                });
            }
        }
    }
    entries
}

/// Number of actual command rows shown/selectable at a given match count —
/// excludes the DIM overflow hint row, which is never a selectable target.
/// Single source of truth for BOTH `render`'s row loop and `App`'s
/// Up/Down/Tab/Enter selection clamping (mirrors `skills_hub_filtered`'s
/// SSOT discipline in `overlay.rs`) — past 6 matches the affordance is
/// "keep typing" to narrow the filter, never "keep pressing Down" into rows
/// that were never drawn.
pub(crate) fn visible_command_count(total_matches: usize) -> usize {
    if total_matches > MAX_VISIBLE_ROWS {
        MAX_VISIBLE_ROWS - 1
    } else {
        total_matches
    }
}

/// DIM-styled one-glyph marker per `CommandCategory` (D-05 discretion,
/// UI-SPEC Typography table) — flat list, no header-grouped sections for v1.
fn category_glyph(category: &ironhermes_core::commands::CommandCategory) -> &'static str {
    use ironhermes_core::commands::CommandCategory;
    match category {
        CommandCategory::Session => "»",
        CommandCategory::Configuration => "⚙",
        CommandCategory::ToolsAndSkills => "◆",
        CommandCategory::Info => "ℹ",
        CommandCategory::Exit => "⏻",
    }
}

/// DIM one-glyph marker for skill rows (D-05). Deliberately distinct from the
/// five `CommandCategory` glyphs (`»⚙◆ℹ⏻`) — in particular NOT `◆`, which the
/// `ToolsAndSkills` category already owns — so a skill row is visually
/// separable from any command row at a glance (UI-SPEC §D, must_haves T-5).
const SKILL_GLYPH: &str = "✦";

/// One palette row: DIM glyph, BOLD `/{name}`, plain description (UI-SPEC
/// Typography). `selected` ORs `Modifier::REVERSED` onto every span so the
/// whole row appears highlighted, not just one segment. Shared by command and
/// skill rows so both surfaces stay pixel-identical except for the glyph.
fn palette_row(glyph: &str, name: &str, description: &str, selected: bool) -> Line<'static> {
    let styled = |text: String, base: Style| -> Span<'static> {
        let style = if selected {
            base.add_modifier(Modifier::REVERSED)
        } else {
            base
        };
        Span::styled(text, style)
    };
    Line::from(vec![
        styled(
            format!("{glyph} "),
            Style::default().add_modifier(Modifier::DIM),
        ),
        styled(
            format!("/{name}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        styled(format!("  —  {description}"), Style::default()),
    ])
}

/// Build a row for a unified `PaletteEntry` (D-05): commands use their
/// `CommandCategory` glyph; skills use `SKILL_GLYPH` (`✦`).
fn entry_row(entry: &PaletteEntry<'_>, selected: bool) -> Line<'static> {
    match entry {
        PaletteEntry::Command(cmd) => palette_row(
            category_glyph(&cmd.category),
            cmd.name,
            cmd.description,
            selected,
        ),
        PaletteEntry::Skill { name, description } => {
            palette_row(SKILL_GLYPH, name, description, selected)
        }
    }
}

/// A single DIM-styled informational row (empty state / overflow hint) —
/// shares styling with `overlay.rs::append_queue_hint`'s convention.
fn dim_row(text: String) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().add_modifier(Modifier::DIM)))
}

/// Bottom-anchored dropdown float, directly above the input line (D-03,
/// RESEARCH Pattern 1). Self-gates via `palette_query` — always called
/// unconditionally from the draw closure, paints nothing when the query is
/// `None`.
pub fn render(frame: &mut Frame, app: &App) {
    let Some(prefix) = palette_query(app) else {
        return;
    };
    let matches = filtered_entries(app, &prefix);

    let frame_area = frame.area();
    let visible_rows = matches.len().clamp(1, MAX_VISIBLE_ROWS);
    let dropdown_height = visible_rows as u16 + 2; // +2 = top/bottom Plain border rows
    let input_top_y = frame_area.y + frame_area.height.saturating_sub(3);
    let rect = Rect {
        x: frame_area.x,
        y: input_top_y.saturating_sub(dropdown_height),
        width: frame_area.width,
        height: dropdown_height,
    };

    frame.render_widget(Clear, rect);
    // NOT Double/Yellow (the modal accent, reserved for the `/model` picker
    // per D-03/UI-SPEC Color) — Plain, unstyled border only.
    let block = Block::bordered().border_type(BorderType::Plain);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    if matches.is_empty() {
        // D-05: skills now share this list, so the empty copy covers both.
        lines.push(dim_row("No matching commands or skills.".to_string()));
    } else {
        let visible_count = visible_command_count(matches.len());
        for (i, entry) in matches.iter().take(visible_count).enumerate() {
            lines.push(entry_row(entry, i == app.palette_selected));
        }
        // >6 matches: never a 7th command row — the last visible row becomes
        // a DIM overflow hint instead (mirrors `overlay.rs::append_queue_hint`).
        if matches.len() > MAX_VISIBLE_ROWS {
            let hidden = matches.len() - visible_count;
            lines.push(dim_row(format!("(+{hidden} more — keep typing)")));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use crate::tui_rata::app::App;

    /// Renders `f` into a `width`x`height` TestBackend buffer and flattens
    /// every cell symbol into a single string (row-major, newline-separated)
    /// for substring assertions. Duplicated verbatim from `overlay.rs`'s
    /// private `render_to_text` (RESEARCH: "moved to a shared test-util
    /// location or duplicated identically... do not write a second,
    /// slightly-different flattening fn") — `overlay.rs`'s copy is private to
    /// its own test module, unreachable from here.
    fn render_to_text(width: u16, height: u16, draw: impl FnOnce(&mut ratatui::Frame)) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(draw).unwrap();
        let buf = terminal.backend().buffer();
        let mut s = String::new();
        for y in 0..height {
            for x in 0..width {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s.push('\n');
        }
        s
    }

    // ── palette_query: the whole "is it open" state model (D-03) ───────────

    #[test]
    fn palette_query_derives_live_from_textarea() {
        let empty = App::new_test_empty();
        assert_eq!(
            super::palette_query(&empty),
            None,
            "empty textarea: no query"
        );

        let mut non_slash = App::new_test_empty();
        non_slash.textarea.insert_str("hello");
        assert_eq!(
            super::palette_query(&non_slash),
            None,
            "non-slash text: no query"
        );

        let mut bare = App::new_test_empty();
        bare.textarea.insert_str("/");
        assert_eq!(
            super::palette_query(&bare),
            Some(String::new()),
            "bare slash: Some(empty prefix)"
        );

        let mut with_space = App::new_test_empty();
        with_space.textarea.insert_str("/help now");
        assert_eq!(
            super::palette_query(&with_space),
            None,
            "space present: query closes (D-03) — never a picker-arg exception here"
        );

        let mut token = App::new_test_empty();
        token.textarea.insert_str("/mod");
        assert_eq!(super::palette_query(&token), Some("mod".to_string()));
    }

    // ── RED (Task 1): the tracer's defining behavior — not yet implemented ──

    #[test]
    fn palette_renders_prefix_filtered_rows() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/mod");
        let out = render_to_text(80, 24, |f| crate::tui_rata::palette::render(f, &app));
        // Asserts on the RENDERED buffer, not on palette_query's own math —
        // repo lesson: no self-verifying tests.
        assert!(out.contains("/model"), "buffer:\n{out}");
    }

    /// Build an isolated `SkillRegistry` with one `gsd-config` skill, loaded via
    /// `load_with_paths` (no `IRONHERMES_HOME` env mutation, so no `env_lock`
    /// needed). The returned `TempDir` is kept alive by the caller.
    fn app_with_one_skill() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("gsd-config");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: gsd-config\ndescription: Configure GSD\n---\nbody\n",
        )
        .unwrap();
        let registry = ironhermes_core::SkillRegistry::load_with_paths(&[skills_dir]);
        let mut app = App::new_test_empty();
        app.skill_registry = Some(std::sync::Arc::new(registry));
        (app, dir)
    }

    /// D-05: `SkillRegistry` entries appear in the unified `/` palette as `✦`
    /// rows alongside commands, and filter by prefix. Asserts on the RENDERED
    /// buffer (no self-verifying math).
    #[test]
    fn palette_lists_skill_rows() {
        let (mut app, _dir) = app_with_one_skill();

        // A prefix that matches the skill: it renders as a ✦ row.
        app.textarea.insert_str("/gsd-con");
        let out = render_to_text(80, 24, |f| crate::tui_rata::palette::render(f, &app));
        assert!(
            out.contains(super::SKILL_GLYPH),
            "a matching skill must render with the ✦ glyph; buffer:\n{out}"
        );
        assert!(
            out.contains("/gsd-config"),
            "the skill name must render in the palette; buffer:\n{out}"
        );

        // The ✦ glyph must NOT collide with any CommandCategory glyph.
        assert_ne!(super::SKILL_GLYPH, "»");
        assert_ne!(super::SKILL_GLYPH, "⚙");
        assert_ne!(super::SKILL_GLYPH, "◆");
        assert_ne!(super::SKILL_GLYPH, "ℹ");
        assert_ne!(super::SKILL_GLYPH, "⏻");

        // Prefix filtering: a non-matching prefix drops the skill.
        let (mut app2, _dir2) = app_with_one_skill();
        app2.textarea.insert_str("/zzznot");
        let out2 = render_to_text(80, 24, |f| crate::tui_rata::palette::render(f, &app2));
        assert!(
            !out2.contains("gsd-config"),
            "a non-matching prefix must exclude the skill row; buffer:\n{out2}"
        );
    }

    fn press(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code,
            modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    /// Tab inserts the highlighted (index-0, registry-order) command with a
    /// TRAILING SPACE (artifacts_produced) — ready to type args, and this
    /// closes the palette itself (the trailing space fails `palette_query`'s
    /// no-space predicate), unlike Enter's completion insert (Task 2, no
    /// trailing space).
    #[test]
    fn palette_tab_inserts() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/mod"); // matches ["model", "models"]; index 0 = "model"
        app.handle_key(press(crossterm::event::KeyCode::Tab));
        assert_eq!(
            app.textarea.lines().join(""),
            "/model ",
            "Tab must insert the highlighted command with a trailing space"
        );
        assert_eq!(
            app.palette_selected, 0,
            "selection resets to 0 after insert"
        );
    }

    // ── geometry (correct from the start — RESEARCH Pattern 1's fixed formula) ──

    #[test]
    fn palette_dropdown_sits_directly_above_input_no_gap() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/help"); // unique single match on Platform::Local
        let text = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            crate::tui_rata::palette::render(f, &app);
        });
        let rows: Vec<&str> = text.lines().collect();
        // frame height=24 -> input_top_y = 24-3 = 21; 1 match -> visible_rows=1,
        // dropdown_height=3 -> rect.y = 21-3 = 18. Dropdown rows 18..21: 18=top
        // border, 19=content, 20=bottom border. Input's own top border/title
        // ("Prompt") starts at row 21 — no gap, no overlap.
        assert!(
            rows[21].contains("Prompt"),
            "row 21 must be the input's top border/title; got: {:?}",
            rows[21]
        );
        assert!(
            rows[20].chars().any(|c| "─└┘│┌┐".contains(c)),
            "row 20 must be the dropdown's own bottom border (no gap before the input); got: {:?}",
            rows[20]
        );
    }

    // ── Task 2: empty / overflow / platform / suppression / Enter ──────────

    #[test]
    fn palette_empty_state() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/zzznotreal");
        let text = render_to_text(80, 24, |f| crate::tui_rata::palette::render(f, &app));
        assert!(
            text.contains("No matching commands or skills."),
            "buffer:\n{text}"
        );
    }

    #[test]
    fn palette_overflow_caps_at_six() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/");
        let total = super::filtered_commands(&app, "").len();
        assert!(
            total > super::MAX_VISIBLE_ROWS,
            "test setup: expected the full Platform::Local command set to exceed {} (got {total})",
            super::MAX_VISIBLE_ROWS
        );

        let text = render_to_text(80, 24, |f| crate::tui_rata::palette::render(f, &app));
        let rows: Vec<&str> = text.lines().collect();
        // 24-row frame, >6 matches -> visible_rows=6, dropdown_height=8,
        // input_top_y=21 -> rect.y=13. Content rows (inner, border-excluded)
        // are 14..=19 (6 rows): 5 command rows + 1 hint row — never a 7th
        // command row (the cap).
        let content_rows = &rows[14..20];
        let command_rows = content_rows.iter().filter(|r| r.contains('/')).count();
        let hint_rows = content_rows
            .iter()
            .filter(|r| r.contains("more — keep typing"))
            .count();
        assert_eq!(
            command_rows, 5,
            "must render exactly 5 command rows, never a 7th (6-row cap); rows:\n{content_rows:?}"
        );
        assert_eq!(
            hint_rows, 1,
            "must render exactly one DIM overflow hint row; rows:\n{content_rows:?}"
        );
    }

    /// RESEARCH Pitfall 4: `/com` matches BOTH "commands" (GatewayOnly) and
    /// "compress" (Universal) — a real overlap in the registry, not an
    /// isolated single-match case. Only "compress" may appear.
    #[test]
    fn palette_excludes_gateway_only_commands() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/com");
        let text = render_to_text(80, 24, |f| crate::tui_rata::palette::render(f, &app));
        assert!(text.contains("compress"), "buffer:\n{text}");
        assert!(
            !text.contains("commands"),
            "GatewayOnly 'commands' must not appear in a Platform::Local render:\n{text}"
        );
    }

    #[test]
    fn palette_suppressed_during_overlay() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/help");
        app.active_overlay = Some(crate::tui_rata::overlay::OverlayKind::SkillsHub);

        let text_without = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            crate::tui_rata::overlay::render(f, &app);
        });
        let text_with_palette_call = render_to_text(80, 24, |f| {
            crate::tui_rata::ui::ui(f, &app);
            crate::tui_rata::overlay::render(f, &app);
            crate::tui_rata::palette::render(f, &app);
        });
        assert_eq!(
            text_without, text_with_palette_call,
            "palette must render nothing while an overlay is active — a modal always wins"
        );
    }

    #[test]
    fn palette_up_down_clamp() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/mod"); // matches ["model", "models"] — 2 rows
        assert_eq!(app.palette_selected, 0);

        app.handle_key(press(crossterm::event::KeyCode::Up));
        assert_eq!(app.palette_selected, 0, "Up at index 0 must not underflow");

        app.handle_key(press(crossterm::event::KeyCode::Down));
        assert_eq!(app.palette_selected, 1);

        app.handle_key(press(crossterm::event::KeyCode::Down));
        assert_eq!(
            app.palette_selected, 1,
            "Down past the last match must not overflow"
        );
    }

    /// D-05 discretion / must_haves "exact + prefix": "model" and "models"
    /// BOTH match the prefix "model" — the palette enumerates all
    /// candidates rather than resolving to a single best match (unlike
    /// `CommandRouter::resolve`) — and registry (source) order is preserved.
    #[test]
    fn palette_lists_exact_and_prefix_matches_both() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/model");
        let matches = super::filtered_commands(&app, "model");
        let names: Vec<&str> = matches.iter().map(|c| c.name).collect();
        assert!(
            names.contains(&"model"),
            "exact match must be listed; got {names:?}"
        );
        assert!(
            names.contains(&"models"),
            "longer prefix match must ALSO be listed; got {names:?}"
        );
        let model_idx = names.iter().position(|n| *n == "model").unwrap();
        let models_idx = names.iter().position(|n| *n == "models").unwrap();
        assert!(
            model_idx < models_idx,
            "registry order must be preserved ('model' declared before 'models'); got {names:?}"
        );
    }

    /// D-04: Enter on an already-complete arg-less command (the highlighted
    /// match's name equals the typed token — nothing to complete) falls
    /// through to `dispatch_or_submit`. Proven the same way
    /// `slash_dispatch_or_submit_short_circuits_submit` (app.rs) proves it:
    /// outside a tokio runtime, the test-path fallback clears the textarea
    /// and records a "slash (test): ..." hint — never re-inserts the text,
    /// which is what an insert-branch bug would leave behind.
    #[test]
    fn palette_enter_argless_runs() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/help"); // unique exact match, no args_hint
        app.handle_key(press(crossterm::event::KeyCode::Enter));
        assert_eq!(
            app.textarea.lines().join(""),
            "",
            "dispatch_or_submit always clears the textarea"
        );
        assert!(
            app.status.hint.contains("slash") || app.status.hint.contains("/help"),
            "status.hint must reflect the slash dispatch ran; got: {:?}",
            app.status.hint
        );
    }

    /// D-04: Enter on a DIFFERING token (highlighted command's name != the
    /// typed token) completes it with NO trailing space — the dropdown
    /// stays open on the now-exact match — so a SECOND Enter submits.
    #[test]
    fn palette_enter_completes_differing_token() {
        let mut app = App::new_test_empty();
        app.textarea.insert_str("/mod"); // matches ["model","models"]; index 0 = "model"
        app.handle_key(press(crossterm::event::KeyCode::Enter));
        assert_eq!(
            app.textarea.lines().join(""),
            "/model",
            "Enter must complete a DIFFERING token with no trailing space (no premature submit)"
        );

        app.handle_key(press(crossterm::event::KeyCode::Enter));
        assert_eq!(
            app.textarea.lines().join(""),
            "",
            "second Enter (now an exact match) must fall through to dispatch_or_submit"
        );
    }
}
