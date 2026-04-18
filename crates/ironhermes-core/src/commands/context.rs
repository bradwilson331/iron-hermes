use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::skills::SkillRegistry;
use crate::types::Platform;

/// Context passed to every command handler.
///
/// Keeps ironhermes-core as a leaf crate by only including deps
/// that live in core itself. CLI and gateway extend context at
/// their integration layer before calling dispatch().
pub struct CommandContext {
    // Required — always available
    pub platform: Platform,
    pub session_id: String,
    pub agent_running: Arc<AtomicBool>,

    // Optional — platform-dependent or not always wired
    pub skill_registry: Option<Arc<SkillRegistry>>,
}

impl CommandContext {
    /// Create a minimal context with all optional fields set to None.
    pub fn new(
        platform: Platform,
        session_id: String,
        agent_running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            platform,
            session_id,
            agent_running,
            skill_registry: None,
        }
    }

    /// Builder: attach a skill registry.
    pub fn with_skill_registry(mut self, registry: Arc<SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }
}
