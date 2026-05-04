pub mod agent_panel;
pub mod block;
pub mod block_stream;
pub mod command_line;
pub mod command_palette;
pub mod input_box;
pub mod markdown;
pub mod scanner;
pub mod sigil;
pub mod status_bar;
pub mod title_bar;
pub mod tool_call;

// Public API re-exports. Phase 3's only consumer (`warp_hermes.rs`) imports
// `TitleBar`, `BlockStream`, `InputBox`, `AgentPanel`, `StatusBar`, and
// `CommandPalette` directly; the remaining five (`Sigil`, `Block`,
// `CommandLine`, `ToolCall`, `Scanner`) are composed internally via
// `super::*` paths in other primitive files. The crate-root re-exports stay
// public for Phase 4 / 5 / 6 callers (TweaksPanel, MobileShell, data layer)
// but appear unused at Phase 3's compile gate — silence the lint here.
#[allow(unused_imports)]
pub use agent_panel::AgentPanel;
#[allow(unused_imports)]
pub use block::Block;
#[allow(unused_imports)]
pub use block_stream::BlockStream;
#[allow(unused_imports)]
pub use command_line::CommandLine;
#[allow(unused_imports)]
pub use command_palette::CommandPalette;
#[allow(unused_imports)]
pub use input_box::InputBox;
#[allow(unused_imports)]
pub use markdown::render_inline_code;
#[allow(unused_imports)]
pub use scanner::Scanner;
#[allow(unused_imports)]
pub use sigil::Sigil;
#[allow(unused_imports)]
pub use status_bar::StatusBar;
#[allow(unused_imports)]
pub use title_bar::TitleBar;
#[allow(unused_imports)]
pub use tool_call::ToolCall;
