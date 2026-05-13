//! Shared shell types and Phase 3 demo fixtures.
//!
//! Per CONTEXT D-04: every shell primitive imports from this module via
//! `use crate::state::*;`. Types derive `Clone + PartialEq + Debug` to satisfy
//! Dioxus 0.7 prop bounds (per AGENTS.md / CLAUDE.md). No reactivity here —
//! Phase 3 is pure static rendering (D-06).
//!
//! Fixture functions (`demo_blocks`, `demo_messages`, `demo_palette_items`,
//! `demo_tabs`) return prototype-verbatim copy taken from
//! `warp2ironhermes/project/app/app.jsx` (`seedBlocks` / `seedMessages` /
//! `PALETTE_ITEMS` / `defaultTabs`) and the Phase 3 UI-SPEC tables.

use dioxus::prelude::Signal;

// ---------------------------------------------------------------------------
// Block stream types (D-10..D-13)
// ---------------------------------------------------------------------------

/// One terminal block. Six variants cover the prototype's block kinds:
/// `is-cmd`, `is-out`, `is-ai`, `is-ok`, `is-err`, plus the agent-side
/// `is-tool` block (D-13).
#[derive(Clone, PartialEq, Debug)]
pub enum Block {
    Cmd {
        command: CommandLine,
    },
    Out {
        author: Option<String>,
        time: Option<String>,
        text: String,
    },
    Ai {
        author: Option<String>,
        time: Option<String>,
        markdown: String,
    },
    Ok {
        author: Option<String>,
        time: Option<String>,
        message: String,
    },
    Err {
        author: Option<String>,
        time: Option<String>,
        exit_code: i32,
        message: String,
    },
    Tool {
        call: ToolCall,
    },
}

impl Block {
    /// Returns the CSS class fragment used by `warp-ih.css` to color the
    /// 2px left accent stripe on each block.
    pub fn kind_class(&self) -> &'static str {
        match self {
            Block::Cmd { .. } => "is-cmd",
            Block::Out { .. } => "is-out",
            Block::Ai { .. } => "is-ai",
            Block::Ok { .. } => "is-ok",
            Block::Err { .. } => "is-err",
            Block::Tool { .. } => "is-tool",
        }
    }
}

/// A parsed shell command line — list of tokens plus prompt/cwd metadata.
/// Defaults for `cwd` and `glyph` per `shell.jsx` line 115.
#[derive(Clone, PartialEq, Debug)]
pub struct CommandLine {
    pub tokens: Vec<Token>,
    pub time: Option<String>,
    pub cwd: Option<String>,
    pub glyph: Option<String>,
}

/// One styled span inside a `CommandLine`. Each variant maps to a CSS class
/// (`bin` / `arg` / `flag` / `str`) for distinct coloring.
///
/// `Str` is unused by Phase 3 fixtures (no quoted string args in `demo_blocks()`)
/// but is part of the public `Token` API consumed by Phase 4's data layer.
#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    Bin(String),
    Arg(String),
    Flag(String),
    #[allow(dead_code)]
    Str(String),
}

impl Token {
    pub fn kind_class(&self) -> &'static str {
        match self {
            Token::Bin(_) => "bin",
            Token::Arg(_) => "arg",
            Token::Flag(_) => "flag",
            Token::Str(_) => "str",
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Token::Bin(s) | Token::Arg(s) | Token::Flag(s) | Token::Str(s) => s.as_str(),
        }
    }
}

/// A tool-call block (`is-tool`) — emitted by the agent side panel and
/// occasionally surfaces in the main stream as a `Block::Tool` variant.
#[derive(Clone, PartialEq, Debug)]
pub struct ToolCall {
    pub name: String,
    pub args_summary: String,
    pub status: ToolStatus,
}

/// Four-state tool lifecycle (D-13).
#[derive(Clone, PartialEq, Debug)]
pub enum ToolStatus {
    Pending,
    Running,
    Done,
    Failed,
}

/// Input mode toggle (`⌥+M` cycles).
///
/// Phase 3 hardcodes `Mode::Shell` at the WarpHermes call site (UI-SPEC line 461);
/// `Mode::Agent` is exercised by Phase 4's `⌥+M` keybind plumbing.
#[derive(Clone, PartialEq, Debug)]
pub enum Mode {
    Shell,
    #[allow(dead_code)]
    Agent,
}

/// One row in the command palette overlay — slash command or workflow.
#[derive(Clone, PartialEq, Debug)]
pub struct PaletteItem {
    pub section: String,
    pub cmd: String,
    pub label: String,
    pub kbd: Vec<String>,
}

/// Title-bar tab.
#[derive(Clone, PartialEq, Debug)]
pub struct Tab {
    pub label: String,
    pub live: bool,
    /// Session key used by `WarpHermes::on_tab_click` to switch the active session.
    /// Populated from the server-returned session ID (Phase 26.2 D-09).
    pub session_id: String,
}

/// One side-panel message (`who: "user" | "hermes"`). When `tool` is `Some`,
/// the message renders as a tool-call card and `body` is empty.
#[derive(Clone, PartialEq, Debug)]
pub struct Message {
    pub who: String,
    pub time: String,
    pub body: String,
    pub tool: Option<ToolCall>,
}

/// Status-bar token-budget pill content.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TokenBudget {
    pub used: u32,
    pub max: u32,
}

/// Compact per-session summary rendered as a memory card in the side panel.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SessionMemory {
    pub session_id: String,
    pub label: String,
    pub is_live: bool,
    pub first_time: String,
    pub last_time: String,
    /// Number of user turns in this session.
    pub exchange_count: u32,
    /// Last known token usage.
    pub token_count: u32,
    /// Personality slug active in this session.
    pub personality: String,
    /// Last user message, truncated to ≤160 chars.
    pub last_input: String,
}

// ---------------------------------------------------------------------------
// Phase 4 type vocabulary (D-03, D-07, D-20, D-02, D-34)
// ---------------------------------------------------------------------------

/// Six personality presets per CONTEXT D-03 + MOCK-01.
///
/// Maps 1:1 to `fakeAgentReply` switch arms in
/// `warp2ironhermes/project/app/app.jsx` lines 339-349. The `label()`
/// returns the lowercase slug used in palette substate rows and the
/// status-bar personality pill (D-22).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Personality {
    Concise,
    Technical,
    Noir,
    Hype,
    Catgirl,
    #[default]
    Default,
}

impl Personality {
    /// Lowercase slug used in palette substate rows and status-bar pill (D-22).
    pub fn label(&self) -> &'static str {
        match self {
            Personality::Concise => "concise",
            Personality::Technical => "technical",
            Personality::Noir => "noir",
            Personality::Hype => "hype",
            Personality::Catgirl => "catgirl",
            Personality::Default => "default",
        }
    }

    /// All six variants in declaration order — used by the palette
    /// `PersonalityPick` substate to enumerate selectable personalities
    /// (D-20). Per CONTEXT Claude's Discretion: small const helper, no `strum`.
    pub const ALL: [Personality; 6] = [
        Personality::Concise,
        Personality::Technical,
        Personality::Noir,
        Personality::Hype,
        Personality::Catgirl,
        Personality::Default,
    ];
}

/// Identified wrapper around `Block` for stable RSX keys across `/clear`
/// and append cycles per CONTEXT D-07. The `Block` enum stays a pure
/// data shape; identity is wrapper concern. RSX iterates with
/// `key: "{entry.id}"` (D-08 starts ids at 1000 for fresh entries).
#[derive(Clone, PartialEq, Debug)]
pub struct BlockEntry {
    pub id: u64,
    pub block: Block,
}

/// Two-state palette substate per CONTEXT D-20.
///
/// `Browse` is the default — full PALETTE_ITEMS list visible.
/// `PersonalityPick` shows the six personalities as palette rows.
/// Selecting `/personality` while in `Browse` transitions to
/// `PersonalityPick`; selecting a personality writes
/// `ShellSettings.personality` and transitions back to `Browse` (then
/// closes palette). Esc from `PersonalityPick` returns to `Browse`
/// without closing.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum PaletteState {
    #[default]
    Browse,
    PersonalityPick,
}

/// Cross-cutting settings bundle provided via `use_context_provider` per
/// CONTEXT D-02 + RESEARCH Pattern 5.
///
/// Forward-compatible: Phase 5 will add `theme: Signal<Theme>`,
/// `density: Signal<Density>`, `block: Signal<BlockStyle>`,
/// `agent: Signal<AgentLayout>` fields without refactoring existing
/// consumers. Putting individual signals in the struct (rather than
/// `Signal<ShellSettings>`) means writes to one field don't invalidate
/// consumers of the others — canonical Dioxus 0.7 "bag of related
/// signals" pattern.
///
/// `Signal<T>` is `Copy`, so `ShellSettings` derives `Copy` — required
/// for cheap `use_context::<ShellSettings>()` clones in consumers.
#[derive(Clone, Copy)]
pub struct ShellSettings {
    pub personality: Signal<Personality>,
    // Phase 5: theme, density, block, agent — additive only (D-02).
}

/// Cross-platform timestamp helper per CONTEXT D-34.
///
/// On wasm32 returns `HH:MM:SS` from `js_sys::Date::new_0()` (local
/// timezone — see RESEARCH Assumptions Log A2). On native returns
/// hardcoded `"00:00:00"` per D-34 trade-off (skip chrono dep; native
/// builds are compile-gated only, not visually exercised). Used by
/// `runShell`/`runAgent` outputs and palette `/status` and `/help`
/// blocks.
pub fn now_time() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new_0();
        format!(
            "{:02}:{:02}:{:02}",
            d.get_hours() as u32,
            d.get_minutes() as u32,
            d.get_seconds() as u32,
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "00:00:00".to_string()
    }
}

// ---------------------------------------------------------------------------
// Phase 3 demo fixtures (D-17, D-18; UI-SPEC tables; app.jsx source-of-truth)
// ---------------------------------------------------------------------------

/// 10-block prototype seed wrapped in `BlockEntry` for stable RSX keys
/// (D-07, D-09). Ids 1..=10 mirror the existing `b1..b10` prototype-source
/// comments. `next_id` (D-08) starts at 1000 for runtime-appended entries
/// so seed ids and runtime ids never collide.
///
/// Source-of-truth: `warp2ironhermes/project/app/app.jsx` `seedBlocks`
/// lines 50-108 plus the four `is-tool` extension entries from CONTEXT D-18.
#[cfg(any(test, feature = "demo"))]
pub fn demo_block_entries() -> Vec<BlockEntry> {
    vec![
        // b1 — `ironhermes doctor`
        BlockEntry { id: 1, block: Block::Cmd {
            command: CommandLine {
                tokens: vec![
                    Token::Bin("ironhermes".into()),
                    Token::Arg("doctor".into()),
                ],
                time: Some("0.4s".into()),
                cwd: Some("~/projects/ironhermes".into()),
                glyph: Some("❯".into()),
            },
        }},
        // b2 — doctor output
        BlockEntry { id: 2, block: Block::Out {
            author: Some("doctor".into()),
            time: Some("00:14:02".into()),
            text: "IronHermes Doctor\n\
                   ----------------------------------------\n\
                   Rust toolchain 1.81.0 stable [OK]\n\
                   dx CLI 0.7.1                  [OK]\n\
                   WASM target installed         [OK]\n\
                   Tailwind v4 input present     [OK]\n\
                   Cargo.lock present            [OK]\n\
                   .gitignore covers .DS_Store   [OK]\n\
                   Anthropic key set             [OK]\n\
                   OpenAI key not set            [MISSING]"
                .into(),
        }},
        // b3 — `git diff --stat`
        BlockEntry { id: 3, block: Block::Cmd {
            command: CommandLine {
                tokens: vec![
                    Token::Bin("git".into()),
                    Token::Arg("diff".into()),
                    Token::Flag("--stat".into()),
                ],
                time: Some("0.1s".into()),
                cwd: Some("~/projects/ironhermes".into()),
                glyph: Some("❯".into()),
            },
        }},
        // b4 — git diff success output
        BlockEntry { id: 4, block: Block::Ok {
            author: Some("git".into()),
            time: Some("00:14:31".into()),
            message: "src/agent/personality.rs | 36 ++++++++++++++++++++--\n \
                      src/status/pills.rs      |  4 ++--\n \
                      tests/personality.rs     | 12 ++++++++++++\n \
                      3 files changed, 48 insertions(+), 4 deletions(-)"
                .into(),
        }},
        // b5 — Hermes reply
        BlockEntry { id: 5, block: Block::Ai {
            author: Some("Hermes".into()),
            time: Some("00:14:48".into()),
            markdown: "The diff looks clean — the new `concise` personality slot is wired through `personality.rs` and the status line picks it up via the existing pill rotation. Want me to add a test that snapshots the rendered status line for each preset?".into(),
        }},
        // b6 — cargo error
        BlockEntry { id: 6, block: Block::Err {
            author: Some("cargo".into()),
            time: Some("00:14:55".into()),
            exit_code: 1,
            message: "error[E0282]: type annotations needed\n  --> src/main.rs:42:9".into(),
        }},
        // b7 — pending tool call
        BlockEntry { id: 7, block: Block::Tool {
            call: ToolCall {
                name: "read_file".into(),
                args_summary: "{\"path\":\"src/lib.rs\"}".into(),
                status: ToolStatus::Pending,
            },
        }},
        // b8 — running tool call
        BlockEntry { id: 8, block: Block::Tool {
            call: ToolCall {
                name: "edit_file".into(),
                args_summary: "{\"path\":\"src/main.rs\",\"line\":42}".into(),
                status: ToolStatus::Running,
            },
        }},
        // b9 — done tool call
        BlockEntry { id: 9, block: Block::Tool {
            call: ToolCall {
                name: "search".into(),
                args_summary: "{\"q\":\"PROMPT_CACHE\"}".into(),
                status: ToolStatus::Done,
            },
        }},
        // b10 — failed tool call
        BlockEntry { id: 10, block: Block::Tool {
            call: ToolCall {
                name: "compile".into(),
                args_summary: "{\"target\":\"wasm32\"}".into(),
                status: ToolStatus::Failed,
            },
        }},
    ]
}

/// 5-message side-panel seed (UI-SPEC lines 237-247; app.jsx `seedMessages`
/// lines 109-118).
#[cfg(any(test, feature = "demo"))]
pub fn demo_messages() -> Vec<Message> {
    vec![
        Message {
            who: "user".into(),
            time: "00:14:42".into(),
            body: "Pull request feedback on the personality refactor — did I miss anything?"
                .into(),
            tool: None,
        },
        Message {
            who: "hermes".into(),
            time: "00:14:43".into(),
            body: String::new(),
            tool: Some(ToolCall {
                name: "read_file".into(),
                args_summary: "{\"path\":\"crates/ironhermes-agent/src/personality.rs\"}".into(),
                status: ToolStatus::Done,
            }),
        },
        Message {
            who: "hermes".into(),
            time: "00:14:46".into(),
            body: "I'll read the file first…\n\nThe new preset registry is clean. One nit: `personality.rs:84` builds the system-prompt prefix with `format!`, but the old code interned it via `PROMPT_CACHE`. Worth restoring to avoid an alloc per turn.".into(),
            tool: None,
        },
        Message {
            who: "user".into(),
            time: "00:14:50".into(),
            body: "Good catch. Patch it.".into(),
            tool: None,
        },
        Message {
            who: "hermes".into(),
            time: "00:14:51".into(),
            body: String::new(),
            tool: Some(ToolCall {
                name: "edit_file".into(),
                args_summary: "{\"path\":\"crates/ironhermes-agent/src/personality.rs\",\"find\":\"format!\",\"replace\":\"PROMPT_CACHE.intern(format!\"}".into(),
                status: ToolStatus::Running,
            }),
        },
    ]
}

/// 10 palette items (6 slash + 4 workflow). Order mirrors UI-SPEC lines
/// 282-298 and app.jsx `PALETTE_ITEMS` lines 12-48.
///
/// Gated behind `cfg(test)` or `feature = "demo"` — production code fetches
/// real commands from the server via `list_slash_commands()` (Plan 03).
#[cfg(any(test, feature = "demo"))]
pub fn demo_palette_items() -> Vec<PaletteItem> {
    vec![
        // Slash commands
        PaletteItem {
            section: "slash".into(),
            cmd: "/help".into(),
            label: "Show available commands".into(),
            kbd: vec!["?".into()],
        },
        PaletteItem {
            section: "slash".into(),
            cmd: "/status".into(),
            label: "IronHermes status".into(),
            kbd: vec!["⌘".into(), "I".into()],
        },
        PaletteItem {
            section: "slash".into(),
            cmd: "/doctor".into(),
            label: "Run doctor checks".into(),
            kbd: vec![],
        },
        PaletteItem {
            section: "slash".into(),
            cmd: "/personality".into(),
            label: "Change personality preset".into(),
            kbd: vec![],
        },
        PaletteItem {
            section: "slash".into(),
            cmd: "/clear".into(),
            label: "Clear scrollback".into(),
            kbd: vec!["⌘".into(), "K".into()],
        },
        PaletteItem {
            section: "slash".into(),
            cmd: "/quit".into(),
            label: "Exit chat".into(),
            kbd: vec!["⌘".into(), "Q".into()],
        },
        // Workflows
        PaletteItem {
            section: "workflow".into(),
            cmd: "git status".into(),
            label: "Git: working tree status".into(),
            kbd: vec![],
        },
        PaletteItem {
            section: "workflow".into(),
            cmd: "cargo build".into(),
            label: "Cargo: build workspace".into(),
            kbd: vec![],
        },
        PaletteItem {
            section: "workflow".into(),
            cmd: "ironhermes chat".into(),
            label: "Start chat session".into(),
            kbd: vec![],
        },
        PaletteItem {
            section: "workflow".into(),
            cmd: "ironhermes doctor".into(),
            label: "Run config doctor".into(),
            kbd: vec![],
        },
    ]
}

/// 3 title-bar tabs (UI-SPEC lines 200-204).
///
/// Gated behind `cfg(test)` or `feature = "demo"` — production code fetches
/// real sessions from the server via `list_sessions()` (Plan 03).
#[cfg(any(test, feature = "demo"))]
pub fn demo_tabs() -> Vec<Tab> {
    vec![
        Tab {
            label: "ironhermes chat".into(),
            live: true,
            session_id: "demo-0".to_string(),
        },
        Tab {
            label: "cargo watch".into(),
            live: true,
            session_id: "demo-1".to_string(),
        },
        Tab {
            label: "agent · scratch".into(),
            live: false,
            session_id: "demo-2".to_string(),
        },
    ]
}

// ===========================================================================
// Phase 26.2.1 — Wheel-menu UI primitives (Plan 02)
// ===========================================================================
//
// Canonical 10-wedge order from wheel-v2.js DEFAULT_SECTIONS lines 11-22
// (CONTEXT D-10): chat, agents, models, tools, skills, memory, sessions,
// providers, gateway, settings. Soul / Schedules / Office are Screen
// variants but NOT wheel wedges — they are reachable via Settings sub-nav
// (Plan 07's SettingsScreenLink).
//
// All types except the newtypes derive Serialize / Deserialize so they
// round-trip through `serde_json` into localStorage (RESEARCH Pattern 5).
// Per D-26 these types are shared across both shells — no
// `#[cfg(feature = "legacy-shell")]` gating.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_default_is_chat() {
        assert_eq!(Screen::default(), Screen::Chat);
    }

    #[test]
    fn wheel_wedge_default_is_chat() {
        assert_eq!(WheelWedge::default(), WheelWedge::Chat);
        assert_eq!(WheelWedge::Chat.index(), 0);
    }

    #[test]
    fn wheel_wedge_from_index_round_trips_all_indices() {
        for i in 0..10 {
            assert_eq!(WheelWedge::from_index(i).index(), i, "wedge {} mismatch", i);
        }
    }

    #[test]
    fn wheel_wedge_from_index_wraps_modulo_ten() {
        assert_eq!(WheelWedge::from_index(10), WheelWedge::Chat);
        assert_eq!(WheelWedge::from_index(11), WheelWedge::Agents);
        assert_eq!(WheelWedge::from_index(19), WheelWedge::Settings);
        assert_eq!(WheelWedge::from_index(20), WheelWedge::Chat);
    }

    #[test]
    fn wheel_wedge_from_index_handles_extreme_value() {
        // Should not panic; result is a valid variant.
        let w = WheelWedge::from_index(usize::MAX);
        // Round-trip via index() must be ≤ 9.
        assert!(w.index() < 10);
    }

    #[test]
    fn wheel_wedge_labels_match_wheel_v2_js() {
        assert_eq!(WheelWedge::Chat.label(), "CHAT");
        assert_eq!(WheelWedge::Agents.label(), "AGENTS");
        assert_eq!(WheelWedge::Models.label(), "MODELS");
        assert_eq!(WheelWedge::Tools.label(), "TOOLS");
        assert_eq!(WheelWedge::Skills.label(), "SKILLS");
        assert_eq!(WheelWedge::Memory.label(), "MEMORY");
        assert_eq!(WheelWedge::Sessions.label(), "SESSIONS");
        assert_eq!(WheelWedge::Providers.label(), "PROVIDER");
        assert_eq!(WheelWedge::Gateway.label(), "GATEWAY");
        assert_eq!(WheelWedge::Settings.label(), "SYSTEM");
    }

    #[test]
    fn wheel_wedge_subs_match_wheel_v2_js() {
        assert_eq!(WheelWedge::Chat.sub(), "INTELLIGENCE CONSOLE");
        assert_eq!(WheelWedge::Agents.sub(), "AUTONOMOUS WORKERS");
        assert_eq!(WheelWedge::Models.sub(), "LANGUAGE CORES");
        assert_eq!(WheelWedge::Tools.sub(), "INSTRUMENT BAY");
        assert_eq!(WheelWedge::Skills.sub(), "CAPABILITY LATTICE");
        assert_eq!(WheelWedge::Memory.sub(), "PERSISTENT CONTEXT");
        assert_eq!(WheelWedge::Sessions.sub(), "ACTIVE TRANSCRIPTS");
        assert_eq!(WheelWedge::Providers.sub(), "INFERENCE GATEWAYS");
        assert_eq!(WheelWedge::Gateway.sub(), "NETWORK BRIDGE");
        assert_eq!(WheelWedge::Settings.sub(), "CONFIGURATION");
    }

    #[test]
    fn wheel_wedge_glyphs_match_wheel_v2_js() {
        assert_eq!(WheelWedge::Chat.glyph(), "▓");
        assert_eq!(WheelWedge::Agents.glyph(), "◆");
        assert_eq!(WheelWedge::Models.glyph(), "◇");
        assert_eq!(WheelWedge::Tools.glyph(), "◈");
        assert_eq!(WheelWedge::Skills.glyph(), "✦");
        assert_eq!(WheelWedge::Memory.glyph(), "⬢");
        assert_eq!(WheelWedge::Sessions.glyph(), "▣");
        assert_eq!(WheelWedge::Providers.glyph(), "◉");
        assert_eq!(WheelWedge::Gateway.glyph(), "⌬");
        assert_eq!(WheelWedge::Settings.glyph(), "⚙");
    }

    #[test]
    fn wheel_wedge_to_screen_one_to_one() {
        assert_eq!(WheelWedge::Chat.to_screen(), Screen::Chat);
        assert_eq!(WheelWedge::Agents.to_screen(), Screen::Agents);
        assert_eq!(WheelWedge::Models.to_screen(), Screen::Models);
        assert_eq!(WheelWedge::Tools.to_screen(), Screen::Tools);
        assert_eq!(WheelWedge::Skills.to_screen(), Screen::Skills);
        assert_eq!(WheelWedge::Memory.to_screen(), Screen::Memory);
        assert_eq!(WheelWedge::Sessions.to_screen(), Screen::Sessions);
        assert_eq!(WheelWedge::Providers.to_screen(), Screen::Providers);
        assert_eq!(WheelWedge::Gateway.to_screen(), Screen::Gateway);
        assert_eq!(WheelWedge::Settings.to_screen(), Screen::Settings);
    }

    #[test]
    fn wheel_state_default_uses_research_pitfall4_geometry() {
        let w = WheelState::default();
        assert_eq!(w.position, (24.0, 24.0));
        assert_eq!(w.size, 240.0);
        assert_eq!(w.active_wedge, WheelWedge::Chat);
    }

    #[test]
    fn wheel_state_round_trips_through_json() {
        let original = WheelState::default();
        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: WheelState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, original);
    }

    #[test]
    fn wheel_wedge_round_trips_through_json() {
        for i in 0..10 {
            let w = WheelWedge::from_index(i);
            let json = serde_json::to_string(&w).expect("serialize");
            let parsed: WheelWedge = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, w);
        }
    }

    #[test]
    fn screen_round_trips_through_json() {
        for s in [
            Screen::Chat,
            Screen::Sessions,
            Screen::Agents,
            Screen::Skills,
            Screen::Models,
            Screen::Memory,
            Screen::Soul,
            Screen::Tools,
            Screen::Schedules,
            Screen::Gateway,
            Screen::Office,
            Screen::Settings,
            Screen::Providers,
        ] {
            let json = serde_json::to_string(&s).expect("serialize");
            let parsed: Screen = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, s);
        }
    }
}
