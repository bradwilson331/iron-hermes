pub mod approval;
pub mod browser_back; // Phase 25.1 — implemented by plan 04
pub mod browser_click; // Phase 25.1 — implemented by plan 06
pub mod browser_close; // Phase 25.1 — implemented by plan 04
pub mod browser_console; // Phase 25.1 — implemented by plan 07
pub mod browser_get_images; // Phase 25.1 — implemented by plan 05
pub mod browser_navigate; // Phase 25.1 — implemented by plan 04
pub mod browser_press; // Phase 25.1 — implemented by plan 03
pub mod browser_scroll; // Phase 25.1 — implemented by plan 03
pub mod browser_session; // Phase 25.1 — implemented by plan 02
pub mod browser_snapshot; // Phase 25.1 — implemented by plan 05
pub mod browser_type; // Phase 25.1 — implemented by plan 06
pub mod browser_vision; // Phase 25.1 — implemented by plan 08
pub mod cronjob_tool;
pub mod delegate_task;
pub mod execute_code;
pub mod file_tools;
pub mod hexapod_tcp; // Phase 27.1.1 — registration in Plan 04 register_defaults
pub mod hexapod_video; // Phase 27.1.4 — stateless single-frame JPEG capture via port 8002
pub mod memory_manager_handle;
pub mod memory_tool;
pub mod registry;
pub mod skills_tool;
pub mod terminal;
pub mod toolset_session; // Phase 25.2 Plan 15 — production ToolsetSessionHandle impl (UAT Issue 2)
pub mod web_extract; // Phase 25.2
pub mod web_local; // Phase 25.2 — shared HTML→Markdown helpers (extract_content_local target)
pub mod web_read;
pub mod web_search;

pub use memory_manager_handle::MemoryManagerHandle;
pub use registry::{
    InterceptHandler, Prerequisite, Tool, ToolRegistry, todo_read_schema, todo_write_schema,
};
pub use toolset_session::RegistryToolsetSession;
pub use web_extract::WebExtractTool;
