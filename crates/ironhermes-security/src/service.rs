//! `SecurityService`: findings from a [`ScreenBackend`] become a level-dependent
//! verdict (Phase 57, D-06 / D-12).

//!
//! [`boot`] spawns exactly one supervised sidecar per process, writes the
//! ironhermes-owned tirith policy root, registers the process-global service and
//! installs it as core's [`ContentScreen`] (D-02).

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use async_trait::async_trait;
use ironhermes_core::config::DefconLevel;
use ironhermes_core::content_screen::{
    ContentKind, ContentScreen, ContentScreenOutcome, ContentScreenRequest, install_content_screen,
};
use ironhermes_security_proto::{SubjectKind, WireFinding};

use crate::client::{SidecarClient, SidecarHealth, SidecarOptions, Unavailable, UnavailableReason};
use crate::policy::{Severity, Verdict, table_verdict, unavailable_verdict};

/// What to screen.
#[derive(Clone, Debug)]
pub struct ScreenSubject {
    pub kind: SubjectKind,
    pub text: String,
    pub name: Option<String>,
}

/// Where a verdict came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictSource {
    Tirith,
    Floor,
    IronhermesRule,
    Unavailable,
}

/// The decision for one subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityVerdict {
    pub verdict: Verdict,
    pub severity: Option<Severity>,
    /// Rule ids only, never titles or evidence.
    pub rules: Vec<String>,
    /// Short human-readable reasons.
    pub reasons: Vec<String>,
    pub source: VerdictSource,
    /// Set whenever the sidecar could not answer; never `None` in that case.
    pub unavailable: Option<UnavailableReason>,
}

/// A source of findings. Implemented by the real client and by test fakes.
#[async_trait]
pub trait ScreenBackend: Send + Sync {
    async fn check(&self, subject: &ScreenSubject) -> Result<Vec<WireFinding>, Unavailable>;
    fn check_blocking(&self, subject: &ScreenSubject) -> Result<Vec<WireFinding>, Unavailable>;
    fn health(&self) -> SidecarHealth;
}

#[async_trait]
impl ScreenBackend for SidecarClient {
    async fn check(&self, subject: &ScreenSubject) -> Result<Vec<WireFinding>, Unavailable> {
        SidecarClient::check(self, subject.kind, &subject.text, subject.name.as_deref()).await
    }
    fn check_blocking(&self, subject: &ScreenSubject) -> Result<Vec<WireFinding>, Unavailable> {
        SidecarClient::check_blocking(self, subject.kind, &subject.text, subject.name.as_deref())
    }
    fn health(&self) -> SidecarHealth {
        SidecarClient::health(self)
    }
}

/// Turns backend findings into verdicts.
pub struct SecurityService {
    backend: Arc<dyn ScreenBackend>,
}

impl SecurityService {
    pub fn new(backend: Arc<dyn ScreenBackend>) -> Self {
        Self { backend }
    }

    /// Screen `subject` at `level`. `warn_class` marks a Warn-class surface (D-12).
    pub async fn screen(
        &self,
        subject: &ScreenSubject,
        level: DefconLevel,
        warn_class: bool,
    ) -> SecurityVerdict {
        verdict_from(
            self.backend.check(subject).await,
            level,
            warn_class,
            subject.kind,
        )
    }

    /// Blocking twin of [`SecurityService::screen`]; never touches a tokio runtime.
    pub fn screen_blocking(
        &self,
        subject: &ScreenSubject,
        level: DefconLevel,
        warn_class: bool,
    ) -> SecurityVerdict {
        verdict_from(
            self.backend.check_blocking(subject),
            level,
            warn_class,
            subject.kind,
        )
    }
}

/// findings -> max severity -> `table_verdict`; `Err` -> the D-12 unavailable rule.
fn verdict_from(
    result: Result<Vec<WireFinding>, Unavailable>,
    level: DefconLevel,
    warn_class: bool,
    kind: SubjectKind,
) -> SecurityVerdict {
    match result {
        Ok(findings) => {
            let severity = findings.iter().map(|f| Severity::from(f.sev)).max();
            let verdict = severity.map_or(Verdict::Allow, |sev| table_verdict(level, sev));
            let mut rules: Vec<String> = Vec::new();
            for f in &findings {
                if !rules.contains(&f.rule) {
                    rules.push(f.rule.clone());
                }
            }
            SecurityVerdict {
                verdict,
                severity,
                rules,
                reasons: findings.iter().map(|f| f.title.clone()).collect(),
                source: VerdictSource::Tirith,
                unavailable: None,
            }
        }
        Err(u) => SecurityVerdict {
            verdict: unavailable_verdict(level, warn_class, kind, u.reason),
            severity: None,
            rules: Vec::new(),
            reasons: vec![format!("security sidecar unavailable: {u}")],
            source: VerdictSource::Unavailable,
            unavailable: Some(u.reason),
        },
    }
}

/// Core's seam: callers in `ironhermes-core`, `-cron` and `-mcp` reach the service
/// through this impl. Synchronous, so it uses the blocking client path and never
/// touches a tokio runtime.
impl ContentScreen for SecurityService {
    fn screen(&self, req: &ContentScreenRequest<'_>) -> ContentScreenOutcome {
        let kind = match req.kind {
            ContentKind::Exec => SubjectKind::Exec,
            ContentKind::Paste => SubjectKind::Paste,
            ContentKind::File => SubjectKind::File,
        };
        let subject = ScreenSubject {
            kind,
            text: req.text.to_string(),
            name: req.name.map(str::to_string),
        };
        let v = self.screen_blocking(&subject, req.level, req.warn_class);
        ContentScreenOutcome {
            verdict: v.verdict,
            rules: v.rules,
            max_severity: v.severity.map(|s| s.label().to_string()),
            unavailable: v.unavailable.map(|r| r.to_string()),
        }
    }
}

/// Options for [`boot`].
#[derive(Clone, Debug)]
pub struct BootOptions {
    pub hermes_home: PathBuf,
    /// Sidecar binary; `None` lets the client fall back to its own lookup and, if
    /// that finds nothing, run unavailable (Missing).
    pub binary: Option<PathBuf>,
    /// Extra argv for the child. Empty in production; tests pass `--mode ..`.
    pub extra_args: Vec<String>,
}

impl BootOptions {
    /// Production options at `hermes_home`, locating the sidecar binary.
    pub fn from_home(hermes_home: PathBuf) -> Self {
        Self {
            hermes_home,
            binary: crate::locate::locate_sidecar_binary(),
            extra_args: Vec::new(),
        }
    }
}

static INSTALLED: LazyLock<RwLock<Option<Arc<SecurityService>>>> =
    LazyLock::new(|| RwLock::new(None));

/// The process-global service, if [`boot`] (or a test install) has run.
pub fn installed() -> Option<Arc<SecurityService>> {
    INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Replace the process-global service. Used by the test helpers.
pub(crate) fn store_installed(svc: Option<Arc<SecurityService>>) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = svc;
}

/// The ironhermes-owned tirith policy: no environment bypass, ever (the client
/// already scrubs the child's environment; this is the second lock).
const BASELINE_POLICY: &str = "\
# Written by IronHermes at boot (Phase 57). IronHermes owns this file; edits are overwritten.
allow_bypass_env: false
allow_bypass_env_noninteractive: false
";

/// Write `<policy_root>/.tirith/policy.yaml` when it is missing or different.
fn write_baseline_policy(policy_root: &Path) {
    let dir = policy_root.join(".tirith");
    let file = dir.join("policy.yaml");
    if std::fs::read_to_string(&file).is_ok_and(|existing| existing == BASELINE_POLICY) {
        return;
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let result = builder
        .create(&dir)
        .and_then(|()| std::fs::write(&file, BASELINE_POLICY));
    if let Err(e) = result {
        tracing::warn!(
            file = %file.display(),
            error = %e,
            "could not write the tirith baseline policy"
        );
    }
}

/// Spawn exactly one supervised sidecar for this process, write the policy root,
/// register the process-global service and install it as core's [`ContentScreen`].
/// A second call returns the same service and spawns nothing (D-02).
///
/// The client queues its own warm-up ping at spawn, so no separate ping is needed
/// here; a failed start is recorded in the client's health, never fatal.
pub fn boot(opts: &BootOptions) -> Arc<SecurityService> {
    let mut slot = INSTALLED.write().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = slot.as_ref() {
        return existing.clone();
    }
    let mut sidecar = SidecarOptions::for_home(&opts.hermes_home);
    sidecar.binary = opts.binary.clone();
    sidecar.extra_args = opts.extra_args.clone();
    write_baseline_policy(&sidecar.policy_root);

    let client = SidecarClient::spawn(sidecar);
    let health = client.health();
    tracing::info!(
        target: "ironhermes::security::sidecar",
        binary = ?health.binary,
        state = ?health.state,
        unavailable = ?health.last_unavailable,
        "security sidecar boot"
    );
    let service = Arc::new(SecurityService::new(Arc::new(client)));
    *slot = Some(service.clone());
    drop(slot);
    install_content_screen(service.clone());
    service
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeBackend;

    fn subject(text: &str) -> ScreenSubject {
        ScreenSubject {
            kind: SubjectKind::Exec,
            text: text.to_string(),
            name: None,
        }
    }

    #[test]
    fn max_severity_wins_and_rule_ids_are_deduplicated() {
        let svc = SecurityService::new(Arc::new(
            FakeBackend::new()
                .finding_if_contains("x", "RuleA", Severity::Low)
                .finding_if_contains("x", "RuleA", Severity::High)
                .finding_if_contains("x", "RuleB", Severity::Medium),
        ));
        let v = svc.screen_blocking(&subject("x"), DefconLevel::THREE, false);
        assert_eq!(v.severity, Some(Severity::High));
        assert_eq!(v.verdict, Verdict::Block);
        assert_eq!(v.rules, vec!["RuleA".to_string(), "RuleB".to_string()]);
        assert_eq!(v.unavailable, None);
        assert_eq!(v.source, VerdictSource::Tirith);
    }

    #[test]
    fn clean_answer_is_allow_with_no_unavailable_reason() {
        let svc = SecurityService::new(Arc::new(FakeBackend::new()));
        let v = svc.screen_blocking(&subject("nothing"), DefconLevel::ONE, true);
        assert_eq!(v.verdict, Verdict::Allow);
        assert_eq!(v.unavailable, None);
    }

    #[test]
    fn unavailable_carries_reason_and_is_not_tirith_sourced() {
        let svc = SecurityService::new(Arc::new(FakeBackend::unavailable(
            UnavailableReason::Timeout,
        )));
        let v = svc.screen_blocking(&subject("x"), DefconLevel::ONE, true);
        assert_eq!(v.verdict, Verdict::Block);
        assert_eq!(v.unavailable, Some(UnavailableReason::Timeout));
        assert_eq!(v.source, VerdictSource::Unavailable);
        assert!(v.rules.is_empty());
    }
}
