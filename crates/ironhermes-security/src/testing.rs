//! In-process fake backend for surface tests (Phase 57).
//!
//! Hidden from docs and never installed by production code: only tests call
//! [`install_fake_for_tests`]. Plan 16's sweep asserts that no production caller
//! exists. Always compiled so every crate's integration tests can use it without a
//! feature flag; it adds no dependencies.
#![doc(hidden)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::content_screen::{clear_content_screen_for_tests, install_content_screen};
use ironhermes_security_proto::WireFinding;

use crate::client::{SidecarHealth, SidecarState, Unavailable, UnavailableReason};
use crate::policy::Severity;
use crate::service::{ScreenBackend, ScreenSubject, SecurityService, store_installed};

struct Rule {
    needle: String,
    rule: String,
    sev: Severity,
}

/// A scripted [`ScreenBackend`]: findings for subjects containing a needle, or a
/// constant unavailability. Records every subject text it was asked to screen.
#[derive(Default)]
pub struct FakeBackend {
    rules: Vec<Rule>,
    down: Option<UnavailableReason>,
    seen: Mutex<Vec<String>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend that always fails with `reason`.
    pub fn unavailable(reason: UnavailableReason) -> Self {
        Self {
            down: Some(reason),
            ..Self::default()
        }
    }

    /// Report a finding (rule id `FakeBlock`) at `sev` when the text contains `needle`.
    pub fn block_if_contains(self, needle: &str, sev: Severity) -> Self {
        self.finding_if_contains(needle, "FakeBlock", sev)
    }

    /// Report a finding (rule id `FakeWarn`) at `sev` when the text contains `needle`.
    pub fn warn_if_contains(self, needle: &str, sev: Severity) -> Self {
        self.finding_if_contains(needle, "FakeWarn", sev)
    }

    /// Report a finding with rule id `rule` at `sev` when the text contains `needle`.
    pub fn finding_if_contains(mut self, needle: &str, rule: &str, sev: Severity) -> Self {
        self.rules.push(Rule {
            needle: needle.to_string(),
            rule: rule.to_string(),
            sev,
        });
        self
    }

    /// Every subject text screened so far, in order.
    pub fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn run(&self, subject: &ScreenSubject) -> Result<Vec<WireFinding>, Unavailable> {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(subject.text.clone());
        if let Some(reason) = self.down {
            return Err(Unavailable {
                reason,
                detail: "fake backend is down".to_string(),
            });
        }
        Ok(self
            .rules
            .iter()
            .filter(|r| subject.text.contains(&r.needle))
            .map(|r| WireFinding {
                rule: r.rule.clone(),
                sev: r.sev.into(),
                title: format!("fake finding {}", r.rule),
            })
            .collect())
    }
}

#[async_trait]
impl ScreenBackend for FakeBackend {
    async fn check(&self, subject: &ScreenSubject) -> Result<Vec<WireFinding>, Unavailable> {
        self.run(subject)
    }

    fn check_blocking(&self, subject: &ScreenSubject) -> Result<Vec<WireFinding>, Unavailable> {
        self.run(subject)
    }

    fn health(&self) -> SidecarHealth {
        SidecarHealth {
            state: if self.down.is_some() {
                SidecarState::Missing
            } else {
                SidecarState::Healthy
            },
            respawns: 0,
            consecutive_timeouts: 0,
            late_replies: 0,
            last_unavailable: self.down,
            binary: None,
        }
    }
}

/// Replace the process-global service and core's content screen with one over
/// `fake`. Tests only.
pub fn install_fake_for_tests(fake: FakeBackend) -> Arc<SecurityService> {
    let service = Arc::new(SecurityService::new(Arc::new(fake)));
    store_installed(Some(service.clone()));
    install_content_screen(service.clone());
    service
}

/// Remove the process-global service and core's content screen. Tests only.
pub fn clear_installed_for_tests() {
    store_installed(None);
    clear_content_screen_for_tests();
}
