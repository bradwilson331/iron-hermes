//! Phase 25.1 — Shared lazy-spawned chromiumoxide session for all 11 browser_* tools.
//!
//! Held behind `Arc<tokio::sync::Mutex<Option<BrowserSession>>>` on AgentLoop (plan 09)
//! so all 11 browser_* tools share one chromium process. First browser_* call spawns;
//! browser_close drops back to None; AgentLoop drop kills the handler task.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig as CdpBrowserConfig};
use chromiumoxide::page::Page;
use futures::StreamExt;
use tracing::{debug, warn};

use ironhermes_core::config::BrowserConfig;

/// Phase 25.1 D-03: lazy-spawned CDP session shared across all 11 browser_* tools.
///
/// Lifecycle:
///   * `spawn(config)` — called on first browser_* tool use
///   * Reused across subsequent browser_* calls
///   * `close()` — explicit teardown by browser_close tool
///   * Implicit teardown when the Arc<Mutex<Option<Self>>> is dropped (AgentLoop drop)
pub struct BrowserSession {
    /// chromiumoxide Browser handle.
    pub browser: Browser,
    /// The single active Page (single-page model in v2.1 — D-03 / OUT-OF-SCOPE).
    pub page: Page,
    /// Phase 25.1 D-10: ref table populated by browser_snapshot.
    /// Maps sequential u64 IDs (1, 2, ...) → opaque element selector string
    /// (data-ironhermes-ref attribute injected at snapshot time per RESEARCH OQ-1 fix).
    /// CLEARED at the start of each browser_snapshot call.
    pub ref_table: HashMap<u64, String>,
    /// Phase 25.1 D-08: console log buffer drained by browser_console mode:"log".
    /// Cleared on browser_close + on each browser_navigate.
    pub console_buffer: Vec<serde_json::Value>,
    /// CDP websocket pump handle. Aborted on Drop or close().
    handler_task: tokio::task::JoinHandle<()>,
}

impl BrowserSession {
    /// Phase 25.1 D-03: lazy-spawn a chromium process via chromiumoxide.
    ///
    /// Returns Err if no chromium binary is discoverable (find_chromium_binary returns None).
    /// The returned session has a fresh `about:blank` Page; the caller (typically
    /// browser_navigate) navigates next.
    pub async fn spawn(config: &BrowserConfig) -> anyhow::Result<Self> {
        let binary = find_chromium_binary(config.chromium_path.as_deref())
            .ok_or_else(|| anyhow::anyhow!(
                "Chromium/Chrome binary not found. Set BROWSER_PATH or browser.chromium_path, \
                 or install chromium. Searched: BROWSER_PATH, CHROMIUM_PATH, PATH, /Applications, /usr/bin, %PROGRAMFILES%"
            ))?;

        debug!(binary = %binary.display(), headed = config.headed, no_sandbox = config.no_sandbox,
               "Phase 25.1: spawning chromium");

        let mut builder = CdpBrowserConfig::builder()
            .chrome_executable(binary)
            .launch_timeout(Duration::from_secs(config.timeout_seconds));

        if config.headed {
            // chromiumoxide 0.9 default IS headless; .with_head() opts INTO headed.
            builder = builder.with_head();
        }
        if config.no_sandbox {
            builder = builder.no_sandbox();
        }

        let cdp_cfg = builder
            .arg("--disable-gpu")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .build()
            .map_err(|e| anyhow::anyhow!("BrowserConfig build failed: {e}"))?;

        let (browser, mut handler) = Browser::launch(cdp_cfg).await
            .map_err(|e| anyhow::anyhow!("chromium launch failed: {e}"))?;

        // CDP websocket pump — must run on a separate task so handler can drive events.
        let handler_task = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if let Err(e) = h {
                    warn!(error = %e, "Phase 25.1: CDP handler error; pump exiting");
                    break;
                }
            }
        });

        let page = browser.new_page("about:blank").await
            .map_err(|e| anyhow::anyhow!("chromium new_page failed: {e}"))?;

        Ok(BrowserSession {
            browser,
            page,
            ref_table: HashMap::new(),
            console_buffer: Vec::new(),
            handler_task,
        })
    }

    /// Phase 25.1 D-03 / browser_close (plan 04): explicit teardown.
    /// After this returns, the BrowserSession should be dropped (Option set to None
    /// in the Arc<Mutex<Option<...>>>).
    pub async fn close(mut self) -> anyhow::Result<()> {
        // browser.close() sends CDP Browser.close — chromium exits cleanly.
        let _ = self.browser.close().await;
        self.handler_task.abort();
        Ok(())
    }

    /// Phase 25.1 D-15: validate URL host against the allowlist.
    /// Empty list = allow all (D-15). Non-empty = exact-match required.
    /// Returns Ok(()) when allowed; Err with the allowed list when blocked.
    pub fn validate_navigation_url(allowed_domains: &[String], url: &str) -> anyhow::Result<()> {
        if allowed_domains.is_empty() {
            return Ok(());
        }
        let host = extract_host(url)
            .ok_or_else(|| anyhow::anyhow!("invalid URL '{}': cannot extract host", url))?;
        if allowed_domains.iter().any(|d| d == &host) {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "{}",
                serde_json::json!({
                    "error": "domain_blocked",
                    "url": url,
                    "host": host,
                    "allowed": allowed_domains,
                    "hint": "Add the host to browser.allowed_domains or leave the list empty to allow all"
                }).to_string()
            ))
        }
    }
}

/// Hand-rolled host extractor — avoids adding the `url` crate dep (OQ-4 resolution).
/// Handles `scheme://host/path`, `scheme://host:port/path`, etc.
fn extract_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let host_with_port = after_scheme.split('/').next()?;
    Some(host_with_port.split(':').next()?.to_string())
}

/// Phase 25.1 D-05: walk env vars → config path → inline PATH search → platform paths.
/// Returns Some(path) when a valid chromium binary file is found, else None.
///
/// `config_path` is the operator-set `browser.chromium_path` from config.yaml (highest precedence
/// after the env vars per D-05 ordering: env vars > config > PATH > platform paths).
pub fn find_chromium_binary(config_path: Option<&str>) -> Option<PathBuf> {
    // 1. BROWSER_PATH env var
    if let Ok(p) = std::env::var("BROWSER_PATH") {
        let path = PathBuf::from(&p);
        if path.is_file() { return Some(path); }
    }
    // 2. CHROMIUM_PATH env var
    if let Ok(p) = std::env::var("CHROMIUM_PATH") {
        let path = PathBuf::from(&p);
        if path.is_file() { return Some(path); }
    }
    // 3. config.browser.chromium_path
    if let Some(p) = config_path {
        let path = PathBuf::from(p);
        if path.is_file() { return Some(path); }
    }
    // 4. Inline PATH search (no `which` crate per OQ-4 — zero new workspace deps).
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for name in &["chromium-browser", "chromium", "google-chrome", "chrome"] {
            for dir in path_var.split(separator) {
                let candidate = PathBuf::from(dir).join(name);
                if candidate.is_file() { return Some(candidate); }
                #[cfg(windows)]
                {
                    let candidate_exe = PathBuf::from(dir).join(format!("{name}.exe"));
                    if candidate_exe.is_file() { return Some(candidate_exe); }
                }
            }
        }
    }
    // 5. macOS platform paths
    #[cfg(target_os = "macos")]
    for p in &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ] {
        let path = PathBuf::from(p);
        if path.is_file() { return Some(path); }
    }
    // 6. Linux platform paths
    #[cfg(target_os = "linux")]
    for p in &[
        "/usr/bin/chromium", "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome", "/snap/bin/chromium",
    ] {
        let path = PathBuf::from(p);
        if path.is_file() { return Some(path); }
    }
    // 7. Windows platform paths
    #[cfg(target_os = "windows")]
    for p in &[
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    ] {
        let path = PathBuf::from(p);
        if path.is_file() { return Some(path); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn find_chromium_binary_returns_none_when_browser_path_points_to_absent_file() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: env_lock + --test-threads=1 ensure single mutator (Phase 21.6 Rust 2024 pattern).
        unsafe {
            std::env::set_var("BROWSER_PATH", "/dev/null/definitely-absent-chromium");
            std::env::remove_var("CHROMIUM_PATH");
            std::env::set_var("PATH", "/dev/null/empty-path");
        }
        // Note: this test relies on no chromium being installed at platform paths.
        // On dev machines with Chrome installed at /Applications/..., the test will
        // still find it — that's expected and acceptable. The contract here is "BROWSER_PATH
        // pointing to absent file is NOT used as a positive match" — the function falls through
        // to PATH/platform paths.
        let result = find_chromium_binary(None);
        // We can't assert None universally (dev machines may have Chrome). We assert the
        // returned path is NOT the absent BROWSER_PATH.
        if let Some(p) = result {
            assert_ne!(p, PathBuf::from("/dev/null/definitely-absent-chromium"),
                "absent BROWSER_PATH must NOT be returned");
        }
        unsafe {
            std::env::remove_var("BROWSER_PATH");
            std::env::remove_var("PATH");
        }
    }

    #[test]
    fn find_chromium_binary_uses_browser_path_when_set_to_real_file() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // Use /bin/sh (always exists on macOS+Linux) as a stand-in for a chromium binary —
        // we're testing the path-resolution contract, not whether the binary IS chromium.
        #[cfg(unix)]
        {
            let real_file = "/bin/sh";
            unsafe { std::env::set_var("BROWSER_PATH", real_file); }
            let result = find_chromium_binary(None);
            unsafe { std::env::remove_var("BROWSER_PATH"); }
            assert_eq!(result, Some(PathBuf::from(real_file)),
                "BROWSER_PATH pointing at a real file MUST be returned");
        }
        #[cfg(not(unix))]
        {
            // Skip on non-unix — no guaranteed real file path
        }
    }

    #[test]
    fn validate_navigation_url_empty_allowlist_allows_all() {
        assert!(BrowserSession::validate_navigation_url(&[], "https://example.com").is_ok());
        assert!(BrowserSession::validate_navigation_url(&[], "http://internal.local").is_ok());
    }

    #[test]
    fn validate_navigation_url_allowlist_blocks_unlisted_host() {
        let allow = vec!["example.com".to_string()];
        let result = BrowserSession::validate_navigation_url(&allow, "https://evil.com");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("domain_blocked"));
        assert!(msg.contains("evil.com"));
        assert!(msg.contains("example.com"));
    }

    #[test]
    fn validate_navigation_url_allowlist_allows_listed_host() {
        let allow = vec!["example.com".to_string()];
        assert!(BrowserSession::validate_navigation_url(&allow, "https://example.com/page").is_ok());
    }
}
