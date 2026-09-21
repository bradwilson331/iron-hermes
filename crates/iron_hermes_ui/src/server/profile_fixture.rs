//! Phase 51 Plan 14 (T13): the shared, genuinely-dispatchable profile fixture
//! consumed by `cli_handoff`, `group_chat_api`, and `mention_handoff_api`'s
//! bin unit tests.
//!
//! `799fd62b8` (51-10 Task 3) routed every UI bot dispatch through
//! `decide_spawn_credential` -> `evaluate_profile_dispatch_at`
//! (`ironhermes_core::dispatch_gate::evaluate_profile_dispatch_core`), which
//! refuses a profile with no `config.yaml` (`dispatch_gate.rs:247`). Before
//! that commit, a bare `workspace/` directory was a sufficient fixture for a
//! bot profile in these tests; afterward it is not — the gate is right to
//! refuse an unconfigured profile, and the fixture was what was wrong. This
//! helper scaffolds a profile the gate actually reaches `Allow` for: a
//! `config.yaml` naming a built-in provider plus a `.env` carrying that
//! provider's key, written in the same strong-quoted form the project's own
//! writer (`profile_api::render_profile_env_with_stamp`) produces. Mirrors
//! the `write_profile`/`openrouter_config` fixture idiom already established
//! in `crates/ironhermes-core/tests/dispatch_gate_vault_backed.rs`.
#![cfg(all(test, feature = "server"))]

use std::path::PathBuf;

use ironhermes_core::config::{Config, ProviderConfig};

/// An obviously-fake value — no provider would accept it — written through
/// [`ironhermes_core::dotenv_write::quote_env_value`] so the `.env` line is
/// in the exact form the project's own writer emits (T-51-83: realistic in
/// FORM, not in VALUE; a grep for a real key prefix over the test tree stays
/// clean).
const FIXTURE_API_KEY: &str = "sk-fixture-not-a-real-key-0000000000000000000000000";

/// Scaffolds `profiles/{name}/` with a `config.yaml` + `.env` the dispatch
/// gate will `Allow`, plus the `workspace/` subdirectory every existing call
/// site depends on. Returns the `workspace/` path, preserving
/// `scaffold_profile_workspace`'s old return value so call sites that use it
/// keep working unchanged.
pub(crate) fn scaffold_dispatchable_profile(name: &str) -> PathBuf {
    let dir = crate::server::profile_api::profile_dir_for(name);
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).expect("mkdir workspace");

    let mut config = Config::default();
    config.model.provider = "openrouter".to_string();
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config
        .save_to(&dir.join("config.yaml"))
        .expect("save_to config.yaml");

    let env_contents = format!(
        "OPENROUTER_API_KEY={}\n",
        ironhermes_core::dotenv_write::quote_env_value(FIXTURE_API_KEY)
    );
    std::fs::write(dir.join(".env"), env_contents).expect("write fixture .env");

    workspace
}
