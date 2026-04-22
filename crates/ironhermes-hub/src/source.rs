use async_trait::async_trait;
use ironhermes_core::SkillSource;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillMeta {
    pub name: String,
    pub identifier: String,
    pub source_id: String,
    pub description: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BundleFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct SkillBundle {
    pub name: String,
    pub identifier: String,
    pub source_id: String,
    pub files: Vec<BundleFile>,
    pub skill_md: String,
    pub metadata: serde_json::Value,
    /// Server-returned `skillsComputedHash` for skills-sh source; `None` for other
    /// HubSource impls. Used as the version key for `hermes skills update` diff per D-14.
    /// This is an OPAQUE STRING — never recomputed on the client; stored verbatim and
    /// compared byte-for-byte when diffing local vs remote snapshot.
    pub snapshot_hash: Option<String>,
}

#[async_trait]
pub trait HubSource: Send + Sync {
    fn source_id(&self) -> &str;
    fn trust_level_for(&self, identifier: &str) -> SkillSource;
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillMeta>, crate::HubError>;
    async fn fetch(&self, identifier: &str) -> Result<SkillBundle, crate::HubError>;
}
