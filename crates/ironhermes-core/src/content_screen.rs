//! Process-wide content-screening seam (Phase 57, D-02 / D-03).
//!
//! `ironhermes-core`, `-cron` and `-mcp` need to screen text (context files, cron
//! prompts, MCP descriptors) with the tirith sidecar, but they must not depend on
//! the crate that owns the sidecar client (`ironhermes-security` depends on core,
//! so the reverse edge would be a cycle). This module is the dependency-inversion
//! point: core defines the verdict type and the [`ContentScreen`] trait, the
//! security crate implements the trait and installs it once per process at boot,
//! and callers ask [`content_screen`] for whatever is installed.
//!
//! [`ScreenVerdict`] is the ONE verdict type for the phase. Its `Ord` is the
//! strictest-wins merge: `Allow < Warn < Block`.
//!
//! The global is an `RwLock<Option<Arc<..>>>` rather than a `OnceLock` so tests can
//! swap it. Implementations of [`ContentScreen::screen`] are synchronous and must
//! not use `block_in_place` (the web server's per-connection `LocalSet` forbids it).

use std::sync::{Arc, LazyLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::config::DefconLevel;

/// What a screening decided. `Ord` gives strictest-wins: `Allow < Warn < Block`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScreenVerdict {
    Allow,
    Warn,
    Block,
}

/// Which kind of text is being screened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    /// A terminal command string.
    Exec,
    /// Inbound or prompt text.
    Paste,
    /// The text of a named context file.
    File,
}

/// One screening request.
pub struct ContentScreenRequest<'a> {
    pub kind: ContentKind,
    pub text: &'a str,
    /// Bare basename for [`ContentKind::File`].
    pub name: Option<&'a str>,
    pub level: DefconLevel,
    /// True when the caller is a Warn-class surface (D-12): an unavailable
    /// sidecar then degrades to Warn / Block by level instead of Allow.
    pub warn_class: bool,
}

/// The result of a screening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentScreenOutcome {
    pub verdict: ScreenVerdict,
    /// Rule ids only: never titles or evidence text.
    pub rules: Vec<String>,
    /// Highest raw severity seen, when any finding existed.
    pub max_severity: Option<String>,
    /// Why the sidecar could not answer. Never `None` for an unavailable
    /// sidecar: an unavailable screen is never reported as clean.
    pub unavailable: Option<String>,
}

/// Prefix of the rule id a surface reports for a subject that could not be screened
/// (`unscreened:too_large`).
///
/// On the tool path this is a cause marker carried in `SecurityEvent::rule_ids` when no
/// named rule decided a Block. It is not a tirith rule id, and the tool path never writes
/// it into an audit row.
pub const UNSCREENED_RULE_PREFIX: &str = "unscreened:";

/// Prefix of the event rule id for a Block caused by a sidecar outage
/// (`sidecar_outage:crash`). A cause marker in `SecurityEvent::rule_ids`, never a tirith
/// rule id and never written into an audit row.
pub const SIDECAR_OUTAGE_RULE_PREFIX: &str = "sidecar_outage:";

/// Event rule id for a Block the regex floor decided. The floor has no rule ids of its
/// own, so this cause marker stands in for them in `SecurityEvent::rule_ids`. It is never
/// written into an audit row.
pub const REGEX_FLOOR_RULE_ID: &str = "regex_floor";

impl ContentScreenOutcome {
    /// Why this subject went unscreened, when it did.
    ///
    /// A caller that receives a Block or Warn with `unavailable` set got that verdict
    /// because the subject could not be screened (for a non-Warn-class subject an
    /// outage is Allow). It names the cause with this helper and audits a
    /// `sidecar_unavailable` row instead of reporting a tirith finding.
    pub fn unscreened_reason(&self) -> Option<String> {
        self.unavailable
            .as_deref()
            .map(|reason| format!("could not be screened ({reason})"))
    }

    /// The rule id for an unscreened subject: `unscreened:<reason>`.
    pub fn unscreened_rule_id(&self) -> Option<String> {
        self.unavailable
            .as_deref()
            .map(|reason| format!("{UNSCREENED_RULE_PREFIX}{reason}"))
    }
}

/// A screening backend. Synchronous; must not use `block_in_place`.
pub trait ContentScreen: Send + Sync {
    fn screen(&self, req: &ContentScreenRequest<'_>) -> ContentScreenOutcome;
}

static CONTENT_SCREEN: LazyLock<RwLock<Option<Arc<dyn ContentScreen>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Install the process-wide screen, replacing any previous one.
pub fn install_content_screen(screen: Arc<dyn ContentScreen>) {
    *CONTENT_SCREEN.write().unwrap_or_else(|e| e.into_inner()) = Some(screen);
}

/// The installed screen, if any.
pub fn content_screen() -> Option<Arc<dyn ContentScreen>> {
    CONTENT_SCREEN
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Remove the installed screen. Tests only.
#[doc(hidden)]
pub fn clear_content_screen_for_tests() {
    *CONTENT_SCREEN.write().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Local;
    impl ContentScreen for Local {
        fn screen(&self, req: &ContentScreenRequest<'_>) -> ContentScreenOutcome {
            ContentScreenOutcome {
                verdict: if req.text.contains("BAD") {
                    ScreenVerdict::Block
                } else {
                    ScreenVerdict::Allow
                },
                rules: vec!["LocalRule".into()],
                max_severity: None,
                unavailable: None,
            }
        }
    }

    #[test]
    fn content_screen_seam_round_trip() {
        clear_content_screen_for_tests();
        assert!(content_screen().is_none(), "nothing installed yet");

        install_content_screen(Arc::new(Local));
        let installed = content_screen().expect("installed screen is returned");
        let out = installed.screen(&ContentScreenRequest {
            kind: ContentKind::Paste,
            text: "BAD text",
            name: None,
            level: DefconLevel::THREE,
            warn_class: false,
        });
        assert_eq!(out.verdict, ScreenVerdict::Block);
        assert_eq!(out.rules, vec!["LocalRule".to_string()]);

        clear_content_screen_for_tests();
        assert!(content_screen().is_none(), "cleared");
    }

    #[test]
    fn unscreened_reason_names_the_cause() {
        let unscreened = ContentScreenOutcome {
            verdict: ScreenVerdict::Block,
            rules: Vec::new(),
            max_severity: None,
            unavailable: Some("too_large".to_string()),
        };
        assert_eq!(
            unscreened.unscreened_reason().as_deref(),
            Some("could not be screened (too_large)")
        );
        assert_eq!(
            unscreened.unscreened_rule_id().as_deref(),
            Some("unscreened:too_large")
        );

        let screened = ContentScreenOutcome {
            unavailable: None,
            ..unscreened
        };
        assert_eq!(screened.unscreened_reason(), None);
        assert_eq!(screened.unscreened_rule_id(), None);
    }

    #[test]
    fn verdict_order_is_strictest_wins() {
        assert!(ScreenVerdict::Allow < ScreenVerdict::Warn);
        assert!(ScreenVerdict::Warn < ScreenVerdict::Block);
        assert_eq!(
            [
                ScreenVerdict::Warn,
                ScreenVerdict::Block,
                ScreenVerdict::Allow
            ]
            .into_iter()
            .max(),
            Some(ScreenVerdict::Block)
        );
    }
}
