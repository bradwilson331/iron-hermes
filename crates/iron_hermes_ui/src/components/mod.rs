pub mod shell;
pub mod warp_hermes;

pub use warp_hermes::WarpHermes;
// shell submodule re-exports its primitives via shell/mod.rs;
// consumers do `use crate::components::shell::TitleBar` etc.
