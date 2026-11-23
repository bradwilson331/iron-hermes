//! Production [`Tier2LlmHandle`] (D-16): Tier 2 consolidation runs on the model configured under
//! `model.roles.memory`, resolved through the same cascade as every other role, and falls back to
//! the main model when the role is unset.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use ironhermes_core::config::Config;
use ironhermes_core::types::{ContentPart, MessageContent};
use ironhermes_core::{ChatMessage, ProviderResolver};
use ironhermes_tier2::llm::Tier2LlmHandle;

use crate::any_client::{build_main_client, build_role_client};

enum ConfigSource {
    /// `<home>/config.yaml`, re-read on every call so profile edits apply without a restart.
    Home(PathBuf),
    Fixed(Box<Config>),
}

pub struct ConfigBackedTier2Llm {
    source: ConfigSource,
}

impl ConfigBackedTier2Llm {
    /// Bound to one profile home; never reads `IRONHERMES_HOME`.
    pub fn for_home(home: &Path) -> Self {
        Self {
            source: ConfigSource::Home(home.join("config.yaml")),
        }
    }

    pub fn with_config(cfg: Config) -> Self {
        Self {
            source: ConfigSource::Fixed(Box::new(cfg)),
        }
    }

    fn config(&self) -> anyhow::Result<Config> {
        match &self.source {
            ConfigSource::Home(path) => Config::load_from(path)
                .with_context(|| format!("loading {} for the memory model", path.display())),
            ConfigSource::Fixed(cfg) => Ok((**cfg).clone()),
        }
    }
}

#[async_trait::async_trait]
impl Tier2LlmHandle for ConfigBackedTier2Llm {
    async fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        let cfg = self.config()?;
        let resolver = ProviderResolver::build(&cfg)?;
        let client = match build_role_client(&resolver, "memory")? {
            Some(c) => c,
            None => build_main_client(&resolver)?,
        };
        let messages = [ChatMessage::system(&system), ChatMessage::user(&user)];
        let response = client
            .chat_completion_fitting(&messages, None, None, Some(max_tokens as usize), None, None)
            .await?;
        let text = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .and_then(|content| match content {
                MessageContent::Text(s) => Some(s),
                MessageContent::Parts(parts) => parts.into_iter().find_map(|p| match p {
                    ContentPart::Text { text } => Some(text),
                    _ => None,
                }),
            })
            .filter(|s| !s.trim().is_empty());
        text.ok_or_else(|| anyhow!("memory model returned an empty reply"))
    }
}
