//! `ironhermes sites list|show` (Phase 54.8 Plan 08: D-20, D-11, D-12).
//!
//! Read-only view of the `sites:` registry in config.yaml. The site-deploy
//! skill calls `sites show <name> --json` right before it clones or deploys,
//! which re-runs the on-disk containment check every time. Nothing here prints
//! a secret: the registry schema has none (D-13).

use std::path::Path;

use clap::Subcommand;
use ironhermes_core::Config;
use ironhermes_core::get_hermes_home;
use ironhermes_core::sites::{SiteProject, workspace_dir};

#[derive(Subcommand, Debug)]
pub enum SitesAction {
    /// List configured site projects (invalid entries are reported, not hidden)
    List,
    /// Show one validated site project
    Show {
        /// Site name (key under `sites:` in config.yaml)
        name: String,
        /// Print one JSON object instead of `key: value` lines
        #[arg(long)]
        json: bool,
    },
}

fn join_args(args: &[String]) -> String {
    if args.is_empty() {
        "(none)".to_string()
    } else {
        args.join(" ")
    }
}

fn or_none(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "(none)".to_string())
}

/// Absolute path of the pending-deploy record the site-deploy skill reads and writes.
fn deploy_record_path(name: &str, home: &Path) -> std::path::PathBuf {
    workspace_dir(home)
        .join(".site-deploys")
        .join(format!("{name}.json"))
}

/// The `key: value` lines of `sites show`, in a fixed order.
pub fn render_site_show(
    name: &str,
    site: &SiteProject,
    clone_dir: &Path,
    home: &Path,
) -> Vec<String> {
    vec![
        format!("name: {name}"),
        format!("repo: {}", site.repo),
        format!("clone_dir: {}", clone_dir.display()),
        format!(
            "clone_present: {}",
            if clone_dir.join(".git").exists() {
                "yes"
            } else {
                "no"
            }
        ),
        format!(
            "deploy_record: {}",
            deploy_record_path(name, home).display()
        ),
        format!("base_branch: {}", site.base_branch),
        format!("deploy_mode: {}", site.deploy_mode.as_str()),
        format!(
            "wrangler_config: {}",
            site.wrangler_config.as_deref().unwrap_or("(auto)")
        ),
        format!(
            "staging_deploy_args: {}",
            join_args(&site.staging.deploy_args)
        ),
        format!("staging_url: {}", or_none(&site.staging.url)),
        format!(
            "production_deploy_args: {}",
            join_args(&site.production.deploy_args)
        ),
        format!("production_url: {}", or_none(&site.production.url)),
    ]
}

/// The same fields as one JSON object (deploy_args as arrays, missing values as null).
pub fn render_site_json(
    name: &str,
    site: &SiteProject,
    clone_dir: &Path,
    home: &Path,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "repo": site.repo,
        "clone_dir": clone_dir.display().to_string(),
        "clone_present": clone_dir.join(".git").exists(),
        "deploy_record": deploy_record_path(name, home).display().to_string(),
        "base_branch": site.base_branch,
        "deploy_mode": site.deploy_mode.as_str(),
        "wrangler_config": site.wrangler_config,
        "staging_deploy_args": site.staging.deploy_args,
        "staging_url": site.staging.url,
        "production_deploy_args": site.production.deploy_args,
        "production_url": site.production.url,
    })
}

/// One line per site: `<name>  <repo>  <deploy_mode>  <clone_dir>` or
/// `<name>  INVALID  <field>: <reason>` (bad entries are reported by field,
/// including an on-disk containment failure). Empty registry -> a hint line.
pub fn render_site_list(config: &Config, home: &Path) -> Vec<String> {
    let sites = config.validated_sites();
    if sites.is_empty() {
        return vec![
            "No sites configured — add a sites: entry to config.yaml (see docs/SITE-DEPLOY-FROM-CHAT.md)"
                .to_string(),
        ];
    }
    sites
        .into_iter()
        .map(|(name, res)| {
            let checked = res.and_then(|site| {
                site.resolve_clone_dir(home)
                    .map(|dir| (site, dir))
                    .map_err(|e| e.with_site(&name))
            });
            match checked {
                Ok((site, dir)) => format!(
                    "{name}  {}  {}  {}",
                    site.repo,
                    site.deploy_mode.as_str(),
                    dir.display()
                ),
                Err(e) => format!("{name}  INVALID  {}: {}", e.field, e.reason),
            }
        })
        .collect()
}

/// Run a `sites` subcommand; returns the process exit code.
pub fn run_sites(action: SitesAction) -> anyhow::Result<i32> {
    let config = Config::load()?;
    let home = get_hermes_home();
    match action {
        SitesAction::List => {
            for line in render_site_list(&config, &home) {
                println!("{line}");
            }
            Ok(0)
        }
        SitesAction::Show { name, json } => {
            let Some((_, res)) = config
                .validated_sites()
                .into_iter()
                .find(|(n, _)| *n == name)
            else {
                eprintln!("site '{name}' not found in config");
                return Ok(1);
            };
            let site = match res {
                Ok(site) => site,
                Err(e) => {
                    eprintln!("{e}");
                    return Ok(2);
                }
            };
            let clone_dir = match site.resolve_clone_dir(&home) {
                Ok(dir) => dir,
                Err(e) => {
                    eprintln!("{}", e.with_site(&name));
                    return Ok(2);
                }
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&render_site_json(&name, &site, &clone_dir, &home))?
                );
            } else {
                for line in render_site_show(&name, &site, &clone_dir, &home) {
                    println!("{line}");
                }
            }
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_empty_registry_prints_hint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let lines = render_site_list(&Config::default(), tmp.path());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("No sites configured"));
        assert!(lines[0].contains("docs/SITE-DEPLOY-FROM-CHAT.md"));
    }

    #[test]
    fn list_valid_and_invalid_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let cfg: Config = serde_yaml::from_str(
            "sites:\n  demo-site:\n    repo: example-org/demo-site\n    clone_path: workspace/demo-site\n    deploy_mode: wrangler\n    staging:\n      deploy_args: [\"--env\", \"staging\"]\n      url: \"https://demo-site-staging.example.workers.dev\"\n  broken:\n    repo: x\n    deploy_mode: bogus\n",
        )
        .unwrap();
        let lines = render_site_list(&cfg, &home);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("broken  INVALID  entry: "));
        assert_eq!(
            lines[1],
            format!(
                "demo-site  example-org/demo-site  wrangler  {}",
                home.join("workspace/demo-site").display()
            )
        );
    }
}
