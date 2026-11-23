//! Phase 54.4 Plan 05 (D-01) — the header's environment chip.
//!
//! A NON-interactive, read-only display of the focused agent's
//! `terminal.backend` (`ENV local`). It must not look or behave switchable: no
//! caret, no hover state, not focusable, no event handler and no write path.
//! Switching environments is Phase 54.5.
//!
//! 54.5 hand-off: the chip is fed a `Vec<EnvOption { id, label }>` and renders
//! the first item. Today that vector always holds the single backend of the
//! focused agent; 54.5's named environment list drops into the same slot.
//!
//! Declares no context provider and reads no signal.

use crate::server::profile_api::AgentHeaderState;
use dioxus::prelude::*;

/// One selectable environment (a single read-only item until 54.5).
#[derive(Clone, PartialEq, Debug)]
pub struct EnvOption {
    pub id: String,
    pub label: String,
}

/// The chip's items: the focused agent's terminal backend, or a single
/// em-dash while it is unresolved, failed or unset.
pub(crate) fn env_options(state: Option<&AgentHeaderState>) -> Vec<EnvOption> {
    let label = state
        .and_then(|s| s.terminal_backend.clone())
        .unwrap_or_else(|| "\u{2014}".to_string());
    vec![EnvOption {
        id: label.clone(),
        label,
    }]
}

/// `ENV {backend}` for `agent`. Fixed width with an ellipsized value; the full
/// text is in `title` and `aria-label`.
#[component]
pub fn EnvChip(agent: String, options: Vec<EnvOption>) -> Element {
    let label = options
        .first()
        .map(|o| o.label.clone())
        .unwrap_or_else(|| "\u{2014}".to_string());
    let tooltip = format!("Terminal backend for {agent} (read-only)");
    let aria = format!("Terminal backend for {agent}: {label} (read-only)");
    rsx! {
        div {
            class: "ih-env-chip",
            role: "group",
            title: "{tooltip}",
            "aria-label": "{aria}",
            span { class: "ih-seg-label", "ENV" }
            span { class: "v", title: "{label}", "{label}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV_CHIP_RS: &str = include_str!("env_chip.rs");

    #[test]
    fn env_dropdown_is_read_only() {
        let code = &ENV_CHIP_RS[..ENV_CHIP_RS.find("#[cfg(test)]").unwrap_or(ENV_CHIP_RS.len())];
        assert!(code.contains("EnvOption"), "the chip is fed by Vec<EnvOption>");
        for frag in [
            ["on", "change"].concat(),
            ["on", "click"].concat(),
            ["on", "keydown"].concat(),
            ["tab", "index"].concat(),
            ["update", "_"].concat(),
        ] {
            assert!(
                !code.contains(&frag),
                "the read-only env chip must not contain `{frag}` (D-01)"
            );
        }
    }

    #[test]
    fn env_options_one_item_from_the_backend() {
        let st = AgentHeaderState {
            provider: None,
            model: None,
            terminal_backend: Some("docker".to_string()),
        };
        let opts = env_options(Some(&st));
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].label, "docker");
        assert_eq!(opts[0].id, "docker");
    }

    #[test]
    fn env_options_without_a_backend_is_a_dash() {
        assert_eq!(env_options(None)[0].label, "\u{2014}");
        let st = AgentHeaderState::default();
        assert_eq!(env_options(Some(&st))[0].label, "\u{2014}");
    }
}
