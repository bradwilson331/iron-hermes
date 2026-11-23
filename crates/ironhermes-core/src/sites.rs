//! Site project registry (Phase 54.8 Plan 08: D-20, D-11, D-12).
//!
//! A `sites:` map in `config.yaml` tells the bot which repositories it may edit
//! and deploy from chat, where the clone lives under `$IRONHERMES_HOME/workspace`
//! and which staging / production targets a deploy uses.
//!
//! The map is stored on [`crate::config::Config`] as raw YAML values (the
//! `mcp_servers` precedent) so a typo in one entry can never make
//! `Config::load` fail and take the gateway down. Entries are parsed and
//! validated on read through `Config::validated_sites`.
//!
//! No secret is ever stored here (D-13): credentials stay in `.env`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How a merged change reaches production (D-11).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeployMode {
    /// The agent runs a production `wrangler deploy` of the merged commit.
    Wrangler,
    /// The merge triggers the Cloudflare build; the agent deploys nothing.
    GitConnected,
}

impl DeployMode {
    /// Stable lowercase name, identical to the YAML spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            DeployMode::Wrangler => "wrangler",
            DeployMode::GitConnected => "git_connected",
        }
    }
}

/// One deploy target (staging or production).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SiteTarget {
    /// Target-selection tokens appended to the deploy command, as a token list
    /// so nothing is ever shell word-split.
    pub deploy_args: Vec<String>,
    /// Public URL of the target. DATA for replies only: it is never
    /// interpolated into a command, so URL parsing here is a sanity check,
    /// not shell escaping.
    pub url: Option<String>,
}

fn default_base_branch() -> String {
    "main".to_string()
}

/// One validated-on-read site project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiteProject {
    /// GitHub `owner/name`.
    pub repo: String,
    /// Clone location relative to `$IRONHERMES_HOME`; must live under `workspace/`.
    pub clone_path: String,
    /// Branch a change is based on and merged into.
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
    pub deploy_mode: DeployMode,
    /// Optional wrangler config path inside the repo (`(auto)` when absent).
    #[serde(default)]
    pub wrangler_config: Option<String>,
    #[serde(default)]
    pub staging: SiteTarget,
    #[serde(default)]
    pub production: SiteTarget,
}

/// A registry entry rule violation. `reason` describes the rule and never
/// echoes the offending value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteConfigError {
    pub site: String,
    pub field: &'static str,
    pub reason: String,
}

impl SiteConfigError {
    fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            site: String::new(),
            field,
            reason: reason.into(),
        }
    }

    /// Attach the site name (entries do not know their own key).
    pub fn with_site(mut self, site: &str) -> Self {
        self.site = site.to_string();
        self
    }
}

impl std::fmt::Display for SiteConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "site '{}': {}: {}", self.site, self.field, self.reason)
    }
}

impl std::error::Error for SiteConfigError {}

/// Components of a clone path below `workspace/`, after lexical validation.
fn clone_components(clone_path: &str) -> Result<Vec<&str>, SiteConfigError> {
    let bad = |reason: &str| SiteConfigError::new("clone_path", reason);
    if clone_path.is_empty() || clone_path.len() > 256 {
        return Err(bad("must be 1-256 characters"));
    }
    if clone_path.starts_with('/') {
        return Err(bad("must be relative to the IronHermes home"));
    }
    let parts: Vec<&str> = clone_path.split('/').collect();
    if parts[0] != "workspace" {
        return Err(bad("must start with the 'workspace' directory"));
    }
    if parts.len() < 2 {
        return Err(bad("must name a directory below 'workspace'"));
    }
    for part in &parts {
        if part.is_empty() || *part == "." || *part == ".." {
            return Err(bad("components must be non-empty and not '.' or '..'"));
        }
        if !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
        {
            return Err(bad(
                "components may only use letters, digits, '_', '.', '-'",
            ));
        }
    }
    Ok(parts[1..].to_vec())
}

/// Identity of a deploy target: the `(env, name)` pair parsed from its
/// `deploy_args`, each `None` when absent. Production may be the default
/// target (both `None`); staging must set at least one and differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetIdentity {
    pub env: Option<String>,
    pub name: Option<String>,
}

fn is_slug_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// Value of a target-selection flag: `[A-Za-z0-9][A-Za-z0-9_.-]{0,63}`.
fn valid_target_value(v: &str) -> bool {
    v.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && v.len() <= 64
        && v.chars().all(is_slug_char)
}

/// Parse a `deploy_args` token list. Only `--env <v>`, `--env=<v>`, `-e <v>`,
/// `--name <v>` and `--name=<v>` are accepted, each target kind at most once
/// (so at most two pairs); any other token or flag is an error. Nothing is
/// ever shell-split or interpreted.
pub fn parse_deploy_args(
    args: &[String],
    field: &'static str,
) -> Result<TargetIdentity, SiteConfigError> {
    let bad = |reason: &str| SiteConfigError::new(field, reason);
    let mut ident = TargetIdentity::default();
    let mut i = 0;
    while i < args.len() {
        let tok = args[i].as_str();
        let (kind_env, value) = if let Some(v) = tok.strip_prefix("--env=") {
            (true, v)
        } else if let Some(v) = tok.strip_prefix("--name=") {
            (false, v)
        } else if tok == "--env" || tok == "-e" || tok == "--name" {
            let Some(v) = args.get(i + 1) else {
                return Err(bad("a target-selection flag is missing its value"));
            };
            i += 1;
            (tok != "--name", v.as_str())
        } else {
            return Err(bad(
                "only the target-selection flags --env/-e/--name with a value are allowed",
            ));
        };
        i += 1;
        if !valid_target_value(value) {
            return Err(bad(
                "target value must be 1-64 characters of letters, digits, '_', '.', '-', \
                 starting with a letter or digit",
            ));
        }
        let slot = if kind_env {
            &mut ident.env
        } else {
            &mut ident.name
        };
        if slot.is_some() {
            return Err(bad("each target-selection flag may appear at most once"));
        }
        *slot = Some(value.to_string());
    }
    Ok(ident)
}

/// Validate a target URL: https, no userinfo, <= 2048 chars. Returns the
/// parsed URL so callers can compare targets structurally.
fn parse_target_url(raw: &str, field: &'static str) -> Result<url::Url, SiteConfigError> {
    let bad = |reason: &str| SiteConfigError::new(field, reason);
    if raw.len() > 2048 {
        return Err(bad("must be at most 2048 characters"));
    }
    let parsed = url::Url::parse(raw).map_err(|_| bad("must be a valid https URL"))?;
    if parsed.scheme() != "https" {
        return Err(bad("must use https"));
    }
    if parsed.host_str().is_none() {
        return Err(bad("must include a host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(bad("must not contain credentials"));
    }
    Ok(parsed)
}

fn validate_repo(repo: &str) -> Result<(), SiteConfigError> {
    let bad = |reason: &str| SiteConfigError::new("repo", reason);
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(bad("must be exactly 'owner/name'"));
    }
    for part in parts {
        if part.is_empty() || part.len() > 100 || part == "." || part == ".." {
            return Err(bad(
                "owner and name must be 1-100 characters and not '.' or '..'",
            ));
        }
        if !part.chars().all(is_slug_char) {
            return Err(bad(
                "owner and name may only use letters, digits, '_', '.', '-'",
            ));
        }
    }
    Ok(())
}

fn validate_base_branch(branch: &str) -> Result<(), SiteConfigError> {
    let bad = |reason: &str| SiteConfigError::new("base_branch", reason);
    if branch.is_empty() || branch.len() > 100 {
        return Err(bad("must be 1-100 characters"));
    }
    if !branch
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return Err(bad("may only use letters, digits, '.', '_', '/', '-'"));
    }
    if branch.starts_with('-') || branch.starts_with('/') {
        return Err(bad("must not start with '-' or '/'"));
    }
    if branch.contains("..") || branch.ends_with('/') || branch.ends_with(".lock") {
        return Err(bad("must not contain '..' or end with '/' or '.lock'"));
    }
    Ok(())
}

fn validate_wrangler_config(path: &str) -> Result<(), SiteConfigError> {
    let bad = |reason: &str| SiteConfigError::new("wrangler_config", reason);
    if path.is_empty() || path.len() > 256 {
        return Err(bad("must be 1-256 characters"));
    }
    if path.starts_with('/') {
        return Err(bad("must be relative to the repository"));
    }
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(bad("components must be non-empty and not '.' or '..'"));
        }
        if !part.chars().all(is_slug_char) {
            return Err(bad(
                "components may only use letters, digits, '_', '.', '-'",
            ));
        }
    }
    Ok(())
}

impl SiteProject {
    /// Validate the entry. Errors carry an empty `site`; callers attach the
    /// name with [`SiteConfigError::with_site`].
    ///
    /// This is lexical validation only; on-disk containment of `clone_path`
    /// is checked by [`SiteProject::resolve_clone_dir`].
    pub fn validate(&self) -> Result<(), SiteConfigError> {
        validate_repo(&self.repo)?;
        clone_components(&self.clone_path)?;
        validate_base_branch(&self.base_branch)?;
        if let Some(cfg) = &self.wrangler_config {
            validate_wrangler_config(cfg)?;
        }

        let staging = parse_deploy_args(&self.staging.deploy_args, "staging.deploy_args")?;
        let production = parse_deploy_args(&self.production.deploy_args, "production.deploy_args")?;
        if self.deploy_mode == DeployMode::GitConnected && !self.production.deploy_args.is_empty() {
            return Err(SiteConfigError::new(
                "production.deploy_args",
                "must be empty in git_connected mode (the merge triggers the build)",
            ));
        }
        if staging == TargetIdentity::default() {
            return Err(SiteConfigError::new(
                "staging.deploy_args",
                "must select a staging target with --env, -e or --name",
            ));
        }
        if staging == production {
            return Err(SiteConfigError::new(
                "staging.deploy_args",
                "must select a different target than production",
            ));
        }

        let Some(staging_raw) = &self.staging.url else {
            return Err(SiteConfigError::new("staging.url", "is required"));
        };
        let staging_url = parse_target_url(staging_raw, "staging.url")?;
        if let Some(prod_raw) = &self.production.url {
            let prod_url = parse_target_url(prod_raw, "production.url")?;
            if staging_url == prod_url {
                return Err(SiteConfigError::new(
                    "staging.url",
                    "must differ from the production url",
                ));
            }
        }
        Ok(())
    }

    /// Resolve the clone directory as an absolute path under the canonical
    /// `home/workspace`, refusing symlinks and non-directories on disk.
    ///
    /// Every existing component below the workspace is inspected with
    /// `symlink_metadata`, so a pre-planted symlink cannot move a clone or a
    /// deploy outside the workspace. Checking stops at the first component
    /// that does not exist (the clone will create it). Callers re-run this on
    /// every use; it is not a one-time check.
    pub fn resolve_clone_dir(&self, home: &Path) -> Result<PathBuf, SiteConfigError> {
        let components = clone_components(&self.clone_path)?;
        let bad = |reason: &str| SiteConfigError::new("clone_path", reason);
        let mut dir = workspace_dir(home);
        let mut missing = false;
        for c in components {
            dir.push(c);
            if missing {
                continue;
            }
            match std::fs::symlink_metadata(&dir) {
                Ok(md) if md.file_type().is_symlink() => {
                    return Err(bad(
                        "a component on disk is a symlink; the clone must stay inside the workspace",
                    ));
                }
                Ok(md) if !md.is_dir() => {
                    return Err(bad("a component on disk exists but is not a directory"));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing = true,
                Err(_) => return Err(bad("a component on disk could not be inspected")),
            }
        }
        Ok(dir)
    }
}

/// Site name rule: `^[a-z0-9][a-z0-9-]{0,63}$`. The name is a map key and a
/// file stem (`.site-deploys/<name>.json`), so it must stay path-safe.
pub fn validate_site_name(name: &str) -> Result<(), SiteConfigError> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if first_ok && rest_ok && name.len() <= 64 {
        Ok(())
    } else {
        Err(SiteConfigError::new(
            "name",
            "must be 1-64 characters of lowercase letters, digits and '-', not starting with '-'",
        )
        .with_site(name))
    }
}

/// Parse and validate one raw `sites:` value. Every failure is attributed to
/// the site by name; a parse failure reports field `entry`.
pub fn parse_site_entry(
    name: &str,
    value: &serde_yaml::Value,
) -> Result<SiteProject, SiteConfigError> {
    validate_site_name(name)?;
    let site: SiteProject = serde_yaml::from_value(value.clone()).map_err(|_| {
        SiteConfigError::new(
            "entry",
            "not a valid site entry (check required keys repo, clone_path, deploy_mode and their types)",
        )
        .with_site(name)
    })?;
    site.validate().map_err(|e| e.with_site(name))?;
    Ok(site)
}

/// Canonical workspace directory (falls back to the plain join when absent).
pub fn workspace_dir(home: &Path) -> PathBuf {
    let ws = home.join("workspace");
    std::fs::canonicalize(&ws).unwrap_or(ws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const DEMO: &str = "sites:\n  demo-site:\n    repo: example-org/demo-site\n    clone_path: workspace/demo-site\n    deploy_mode: wrangler\n    staging:\n      deploy_args: [\"--env\", \"staging\"]\n      url: \"https://demo-site-staging.example.workers.dev\"\n    production:\n      url: \"https://demo-site.example.workers.dev\"\n";

    #[test]
    fn valid_site_parses_and_resolves_clone_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let cfg_path = home.join("config.yaml");
        std::fs::write(&cfg_path, DEMO).unwrap();
        let config = Config::load_from(&cfg_path).expect("config loads");
        let sites = config.validated_sites();
        assert_eq!(sites.len(), 1);
        let (name, res) = &sites[0];
        assert_eq!(name, "demo-site");
        let site = res.as_ref().expect("demo-site validates");
        assert_eq!(site.base_branch, "main");
        assert_eq!(site.deploy_mode, DeployMode::Wrangler);
        assert_eq!(site.staging.deploy_args, vec!["--env", "staging"]);
        assert!(site.production.deploy_args.is_empty());
        assert_eq!(
            site.resolve_clone_dir(&home).unwrap(),
            home.join("workspace").join("demo-site")
        );
    }

    const STAGING_URL: &str = "https://demo-site-staging.example.workers.dev";
    const PROD_URL: &str = "https://demo-site.example.workers.dev";

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn base() -> SiteProject {
        SiteProject {
            repo: "example-org/demo-site".to_string(),
            clone_path: "workspace/demo-site".to_string(),
            base_branch: "main".to_string(),
            deploy_mode: DeployMode::Wrangler,
            wrangler_config: None,
            staging: SiteTarget {
                deploy_args: args(&["--env", "staging"]),
                url: Some(STAGING_URL.to_string()),
            },
            production: SiteTarget {
                deploy_args: vec![],
                url: Some(PROD_URL.to_string()),
            },
        }
    }

    fn field_of(site: &SiteProject) -> Option<&'static str> {
        site.validate().err().map(|e| e.field)
    }

    #[test]
    fn base_site_is_valid() {
        assert_eq!(base().validate(), Ok(()));
    }

    #[test]
    fn rejects_bad_repo() {
        for bad in [
            "example-org/demo/extra",
            "../demo",
            "example org/demo",
            "example-org/demo;id",
            "/demo",
            "example-org/..",
            "example-org",
            "",
        ] {
            let mut s = base();
            s.repo = bad.to_string();
            assert_eq!(field_of(&s), Some("repo"), "repo {bad:?} must be rejected");
        }
    }

    #[test]
    fn rejects_clone_path_escape() {
        for bad in [
            "../demo",
            "/abs/demo",
            "workspace/../demo",
            "other/demo",
            "workspace/demo site",
            "workspace",
            "workspace/./demo",
            "workspace//demo",
        ] {
            let mut s = base();
            s.clone_path = bad.to_string();
            assert_eq!(
                field_of(&s),
                Some("clone_path"),
                "clone_path {bad:?} must be rejected"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_clone_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.join("workspace/demo-site")).unwrap();
        let err = base().resolve_clone_dir(&home).unwrap_err();
        assert_eq!(err.field, "clone_path");
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_ancestor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.join("workspace/group")).unwrap();
        let mut s = base();
        s.clone_path = "workspace/group/demo-site".to_string();
        let err = s.resolve_clone_dir(&home).unwrap_err();
        assert_eq!(err.field, "clone_path");
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn rejects_non_directory_component() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        std::fs::write(home.join("workspace/demo-site"), b"not a dir").unwrap();
        let err = base().resolve_clone_dir(&home).unwrap_err();
        assert_eq!(err.field, "clone_path");
        // A regular file as a non-final component is refused too.
        let mut s = base();
        s.clone_path = "workspace/demo-site/inner".to_string();
        assert_eq!(s.resolve_clone_dir(&home).unwrap_err().field, "clone_path");
    }

    #[test]
    fn accepts_absent_clone_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        assert_eq!(
            base().resolve_clone_dir(&home).unwrap(),
            home.join("workspace/demo-site")
        );
        // An existing real directory is fine too.
        std::fs::create_dir_all(home.join("workspace/demo-site")).unwrap();
        assert!(base().resolve_clone_dir(&home).is_ok());
    }

    #[test]
    fn rejects_unsafe_deploy_args() {
        let long = "a".repeat(65);
        let cases: Vec<Vec<String>> = vec![
            args(&["--env staging"]),
            args(&["$(id)"]),
            args(&["`id`"]),
            args(&[";rm"]),
            args(&[">out"]),
            args(&["a|b"]),
            args(&["\"quoted\""]),
            args(&["--env", &long]),
            args(&["--dry-run"]),
            args(&["--config", "x"]),
            args(&["--compatibility-date=2024-01-01"]),
            args(&["--var", "A:B"]),
            args(&["--env"]),
            args(&["staging"]),
            args(&["--env", "a", "--name", "b", "-e", "c"]),
            args(&["--env", "--dry-run"]),
            args(&["--env=staging", "-e", "other"]),
        ];
        for bad in cases {
            let mut s = base();
            s.staging.deploy_args = bad.clone();
            assert_eq!(
                field_of(&s),
                Some("staging.deploy_args"),
                "staging args {bad:?} must be rejected"
            );
            let mut p = base();
            p.production.deploy_args = bad.clone();
            assert_eq!(
                field_of(&p),
                Some("production.deploy_args"),
                "production args {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_target_selection_args() {
        for good in [
            args(&["--env", "staging"]),
            args(&["--env=staging"]),
            args(&["-e", "staging"]),
            args(&["--name", "demo-site_2"]),
            args(&["--name=demo-site_2", "--env", "staging"]),
        ] {
            let mut s = base();
            s.staging.deploy_args = good.clone();
            assert_eq!(
                s.validate(),
                Ok(()),
                "staging args {good:?} must be accepted"
            );
        }
    }

    #[test]
    fn rejects_staging_without_target() {
        let mut s = base();
        s.staging.deploy_args = vec![];
        assert_eq!(field_of(&s), Some("staging.deploy_args"));
    }

    #[test]
    fn rejects_staging_equal_to_production() {
        let mut s = base();
        s.production.deploy_args = args(&["--env", "staging"]);
        assert_eq!(field_of(&s), Some("staging.deploy_args"));
        // Different spellings of the same identity are still equal.
        s.staging.deploy_args = args(&["--env=staging"]);
        s.production.deploy_args = args(&["-e", "staging"]);
        assert_eq!(field_of(&s), Some("staging.deploy_args"));
        // A genuinely different environment passes.
        s.production.deploy_args = args(&["--env", "production"]);
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn rejects_missing_or_equal_staging_url() {
        let mut s = base();
        s.staging.url = None;
        assert_eq!(field_of(&s), Some("staging.url"));
        let mut s = base();
        s.staging.url = s.production.url.clone();
        assert_eq!(field_of(&s), Some("staging.url"));
    }

    #[test]
    fn rejects_non_https_or_userinfo_url() {
        let long = format!("https://x.example/{}", "a".repeat(2040));
        assert!(long.len() > 2048);
        for bad in [
            "http://x.example".to_string(),
            "https://user:pw@x.example".to_string(),
            "ftp://x.example".to_string(),
            "not a url".to_string(),
            long,
        ] {
            let mut s = base();
            s.staging.url = Some(bad.clone());
            assert_eq!(
                field_of(&s),
                Some("staging.url"),
                "staging url {bad:?} must be rejected"
            );
        }
        let mut p = base();
        p.production.url = Some("http://x.example".to_string());
        assert_eq!(field_of(&p), Some("production.url"));
    }

    #[test]
    fn rejects_bad_base_branch_and_wrangler_config() {
        for bad in [
            "-x",
            "a..b",
            "main.lock",
            "/main",
            "feature/",
            "has space",
            "",
        ] {
            let mut s = base();
            s.base_branch = bad.to_string();
            assert_eq!(
                field_of(&s),
                Some("base_branch"),
                "base_branch {bad:?} must be rejected"
            );
        }
        let mut ok = base();
        ok.base_branch = "release/1.2".to_string();
        assert_eq!(ok.validate(), Ok(()));
        for bad in ["../wrangler.jsonc", "/etc/x", "a/../b", "a b", ""] {
            let mut s = base();
            s.wrangler_config = Some(bad.to_string());
            assert_eq!(
                field_of(&s),
                Some("wrangler_config"),
                "wrangler_config {bad:?} must be rejected"
            );
        }
        let mut ok = base();
        ok.wrangler_config = Some("apps/web/wrangler.jsonc".to_string());
        assert_eq!(ok.validate(), Ok(()));
    }

    #[test]
    fn git_connected_rejects_production_args() {
        let mut s = base();
        s.deploy_mode = DeployMode::GitConnected;
        assert_eq!(s.validate(), Ok(()));
        s.production.deploy_args = args(&["--env", "prod"]);
        assert_eq!(field_of(&s), Some("production.deploy_args"));
    }

    #[test]
    fn rejects_bad_site_name() {
        for bad in [
            "",
            "-x",
            "Demo",
            "demo_site",
            "demo site",
            "a/b",
            &"a".repeat(65),
        ] {
            assert_eq!(
                validate_site_name(bad).unwrap_err().field,
                "name",
                "site name {bad:?} must be rejected"
            );
        }
        assert!(validate_site_name("demo-site").is_ok());
        assert!(validate_site_name("a").is_ok());
    }

    #[test]
    fn invalid_entry_does_not_break_config_load() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.yaml");
        let yaml = format!("{DEMO}  broken:\n    repo: x\n    deploy_mode: bogus\n");
        std::fs::write(&cfg_path, yaml).unwrap();
        let config = Config::load_from(&cfg_path).expect("a bad site entry must not fail load");
        let sites = config.validated_sites();
        assert_eq!(sites.len(), 2);
        let ok = sites.iter().filter(|(_, r)| r.is_ok()).count();
        assert_eq!(ok, 1);
        let (name, res) = sites.iter().find(|(_, r)| r.is_err()).unwrap();
        assert_eq!(name, "broken");
        let err = res.as_ref().unwrap_err();
        assert_eq!(err.field, "entry");
        assert_eq!(err.site, "broken");
    }

    #[test]
    fn sites_absent_is_not_serialized() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.yaml");
        Config::default().save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.lines().any(|l| l.starts_with("sites:")),
            "a default config must not emit a sites: key"
        );
    }

    #[test]
    fn site_project_has_no_secret_fields() {
        let src = include_str!("sites.rs");
        let banned = ["tok", "en"].concat();
        let banned_key = ["ke", "y"].concat();
        let banned_secret = ["sec", "ret"].concat();
        let banned_pw = ["pass", "word"].concat();
        for def in [
            ["pub struct ", "SiteProject {"].concat(),
            ["pub struct ", "SiteTarget {"].concat(),
        ] {
            let start = src.find(&def).expect("struct definition present");
            let body = &src[start..];
            let end = body.find("\n}\n").expect("struct end");
            for line in body[..end].lines() {
                let line = line.trim();
                if line.starts_with("pub ") && !line.starts_with("pub struct") {
                    let name = line
                        .trim_start_matches("pub ")
                        .split(':')
                        .next()
                        .unwrap()
                        .to_lowercase();
                    for b in [&banned, &banned_key, &banned_secret, &banned_pw] {
                        assert!(!name.contains(b.as_str()), "secret-like field {name}");
                    }
                }
            }
        }
    }
}
