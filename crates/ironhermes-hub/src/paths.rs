use std::path::PathBuf;

/// Root directory for installed skills: $HERMES_HOME/skills or ~/.ironhermes/skills.
pub fn skills_root() -> anyhow::Result<PathBuf> {
    if let Ok(home) = std::env::var("HERMES_HOME") {
        return Ok(PathBuf::from(home).join("skills"));
    }
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?;
    Ok(home.join(".ironhermes").join("skills"))
}

pub fn hub_dir() -> anyhow::Result<PathBuf> {
    Ok(skills_root()?.join(".hub"))
}

/// Single manifest dict file — matches reference skills_hub.py exactly.
pub fn manifest_path() -> anyhow::Result<PathBuf> {
    Ok(hub_dir()?.join("lock.json"))
}

/// Quarantine lives under .hub/ so cross-FS atomic rename to final dest stays
/// within $HOME (Pitfall 2).
pub fn quarantine_dir() -> anyhow::Result<PathBuf> {
    Ok(hub_dir()?.join("quarantine"))
}
