//! Phase 49.4 Plan 07 (D-05..D-09): the Skills IMPORT wizard, the NEW SKILL
//! form wizard, and the SKILL.md editor with fork-on-save for bundled
//! skills — mounted from `screens/skills.rs`.
//!
//! Every wizard here calls exactly one of plan 05's four gated `#[server]`
//! fns in `crate::server::skills_import_api` and nothing else: no client
//! fetch, no client-side SKILL.md parsing, and no client filesystem access.

pub mod import_wizard;
pub mod new_skill_wizard;
pub mod skill_editor;

pub use import_wizard::SkillImportWizard;
pub use new_skill_wizard::NewSkillWizard;
pub use skill_editor::{EditorTarget, SkillMdEditor};
