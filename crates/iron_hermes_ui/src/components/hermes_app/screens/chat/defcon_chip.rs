//! Phase 57 UI-SPEC R9c / D-09 / D-21 — the DEFCON level chip in the chat header.
//!
//! The 54.3 yolo chip is gone, so a session left at level 4-5 would show nothing until a
//! turn turned amber. This chip is the always-visible level signal: `DEFCON {N}` for the
//! session on screen, neutral at levels 1-3 and amber with `DEFCON {N} · bypass` when the
//! server says the bypass is active (D-21: the `bypass` flag comes from the server, never
//! from the number). The level is read through `session_defcon_level` at mount, on every
//! session switch and after every reconnect, and updated by every `DefconLevel` push.
//!
//! Severity colour is on the non-text border only (UI-SPEC Color rule: amber text fails AA
//! at this size in the light theme). `DefconChipCtx` is created and provided only at the
//! `HermesApp` root; this module consumes it, never provides it.

use crate::protocol::{ChatStreamEvent, SessionDefconInfo};
use dioxus::prelude::*;

/// The latest `DefconLevel` push the socket delivered.
#[derive(Clone, Debug, PartialEq)]
pub struct DefconPush {
    pub session_id: String,
    pub level: u8,
    pub bypass: bool,
}

/// Context newtype for the latest push, provided at the `HermesApp` root (a provider
/// declared in a child panics its consumers at runtime).
#[derive(Clone, Copy)]
pub struct DefconChipCtx(pub Signal<Option<DefconPush>>);

/// Store a `DefconLevel` push. Returns whether the store changed; any other event is ignored.
pub fn apply_defcon_event(store: &mut Option<DefconPush>, ev: &ChatStreamEvent) -> bool {
    let ChatStreamEvent::DefconLevel {
        session_id,
        level,
        bypass,
    } = ev
    else {
        return false;
    };
    let next = DefconPush {
        session_id: session_id.clone(),
        level: *level,
        bypass: *bypass,
    };
    if store.as_ref() == Some(&next) {
        return false;
    }
    *store = Some(next);
    true
}

/// Forget the stored push, so the value fetched after a reconnect wins (a server restart
/// resets the level). Returns whether anything was cleared.
pub fn clear_defcon_pushes(store: &mut Option<DefconPush>) -> bool {
    store.take().is_some()
}

/// The level and bypass flag to show for the session on screen: a push for that session
/// wins over the fetched value, a push for another session is ignored, and `None` means the
/// level is not known yet (the chip is not rendered).
pub fn chip_level(
    on_screen: &str,
    pushed: Option<&DefconPush>,
    fetched: Option<&SessionDefconInfo>,
) -> Option<(u8, bool)> {
    if let Some(p) = pushed {
        if p.session_id == on_screen {
            return Some((p.level, p.bypass));
        }
    }
    fetched.map(|f| (f.level, f.bypass))
}

/// The chip's colour treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipTone {
    Neutral,
    Amber,
}

/// The chip text and tone for a level. The tone follows the server's `bypass` flag.
pub fn defcon_chip_view(level: u8, bypass: bool) -> (String, ChipTone) {
    if bypass {
        (format!("DEFCON {level} \u{00b7} bypass"), ChipTone::Amber)
    } else {
        (format!("DEFCON {level}"), ChipTone::Neutral)
    }
}

/// The chat-header chip for the on-screen session's DEFCON level: one span whose text is a
/// text node (no title, no extra copy).
#[component]
pub fn DefconChip(level: u8, bypass: bool) -> Element {
    let (text, tone) = defcon_chip_view(level, bypass);
    let class = match tone {
        ChipTone::Neutral => "screen-meta ih-defcon-chip is-neutral",
        ChipTone::Amber => "screen-meta ih-defcon-chip is-amber",
    };
    rsx! {
        span { class: "{class}", "{text}" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(session: &str, level: u8, bypass: bool) -> ChatStreamEvent {
        ChatStreamEvent::DefconLevel {
            session_id: session.into(),
            level,
            bypass,
        }
    }

    thread_local! {
        static CHIP_PROPS: std::cell::Cell<(u8, bool)> = const { std::cell::Cell::new((3, false)) };
    }

    fn render_chip(level: u8, bypass: bool) -> String {
        CHIP_PROPS.with(|p| p.set((level, bypass)));
        fn root() -> Element {
            let (level, bypass) = CHIP_PROPS.with(|p| p.get());
            rsx! { DefconChip { level, bypass } }
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn defcon_chip_text_and_tone() {
        assert_eq!(
            defcon_chip_view(3, false),
            ("DEFCON 3".to_string(), ChipTone::Neutral)
        );
        assert_eq!(
            defcon_chip_view(1, false),
            ("DEFCON 1".to_string(), ChipTone::Neutral)
        );
        assert_eq!(
            defcon_chip_view(4, true),
            ("DEFCON 4 \u{00b7} bypass".to_string(), ChipTone::Amber)
        );
        assert_eq!(
            defcon_chip_view(5, true),
            ("DEFCON 5 \u{00b7} bypass".to_string(), ChipTone::Amber)
        );
        assert_eq!(
            defcon_chip_view(5, false).1,
            ChipTone::Neutral,
            "the tone follows the server's bypass flag, not the number"
        );
        assert_eq!(defcon_chip_view(2, true).1, ChipTone::Amber);

        let amber = render_chip(5, true);
        for needle in ["ih-defcon-chip", "is-amber", "DEFCON 5 \u{00b7} bypass"] {
            assert!(amber.contains(needle), "missing `{needle}` in {amber}");
        }
        let neutral = render_chip(3, false);
        assert!(neutral.contains("ih-defcon-chip"), "{neutral}");
        assert!(neutral.contains("is-neutral"), "{neutral}");
        assert!(neutral.contains("DEFCON 3"), "{neutral}");
        assert!(!neutral.contains("bypass"), "{neutral}");
    }

    #[test]
    fn defcon_chip_follows_level_events() {
        let mut store = None;
        assert!(apply_defcon_event(&mut store, &push("s1", 1, false)));
        assert_eq!(
            store,
            Some(DefconPush {
                session_id: "s1".into(),
                level: 1,
                bypass: false
            })
        );
        assert!(
            !apply_defcon_event(&mut store, &push("s1", 1, false)),
            "an identical push changes nothing"
        );
        assert!(
            !apply_defcon_event(
                &mut store,
                &ChatStreamEvent::QueueUpdated {
                    depth: 1,
                    paused: false
                }
            ),
            "any other event is ignored"
        );

        let fetched = SessionDefconInfo {
            level: 3,
            bypass: false,
        };
        assert_eq!(
            chip_level("s1", store.as_ref(), Some(&fetched)),
            Some((1, false)),
            "a push for the on-screen session wins"
        );
        assert_eq!(
            chip_level("s2", store.as_ref(), Some(&fetched)),
            Some((3, false)),
            "a push for another session is ignored"
        );
        assert_eq!(chip_level("s1", None, Some(&fetched)), Some((3, false)));
        assert_eq!(chip_level("s2", store.as_ref(), None), None);
        assert_eq!(chip_level("s1", None, None), None, "unknown: no chip");

        assert!(clear_defcon_pushes(&mut store));
        assert!(store.is_none());
        assert!(!clear_defcon_pushes(&mut store), "nothing left to clear");
    }

    /// Structural contracts a browser alone could observe: the chip sits in the chat header
    /// and is fed by the server fn and the push; the root owns the signal; the stylesheet
    /// keeps severity colour on the border. Needles are built with `concat!`.
    #[test]
    fn defcon_chip_in_chat_header() {
        const CHAT_RS: &str = include_str!("../chat.rs");
        const MOD_RS: &str = include_str!("../../mod.rs");
        const SCREENS_CSS: &str = include_str!("../../../../../assets/screens.css");
        let chat = &CHAT_RS[..CHAT_RS.find(concat!("#[cfg(", "test)]")).unwrap()];
        for needle in [
            concat!("DefconChip", " {"),
            concat!("session_defcon_level", "("),
        ] {
            assert!(chat.contains(needle), "chat.rs lacks `{needle}`");
        }
        let at = chat
            .find(concat!("defcon_fetch = use_", "resource("))
            .expect("ScreenChat needs the defcon_fetch resource");
        let region = &chat[at..];
        let region = &region[..region.find("});").expect("the resource closure ends")];
        assert!(
            region.contains(concat!("is_ws_connected", ".read()")),
            "the resource must read the socket state so it re-runs on every reconnect"
        );
        assert!(
            region.contains(concat!("session_defcon_level", "(")),
            "the resource must call the server fn"
        );
        for needle in [
            concat!("DefconChip", "Ctx("),
            concat!("apply_defcon", "_event("),
            concat!("clear_defcon", "_pushes("),
        ] {
            assert!(MOD_RS.contains(needle), "mod.rs lacks `{needle}`");
        }

        let mut css = String::new();
        let mut rest = SCREENS_CSS;
        while let Some(i) = rest.find("/*") {
            css.push_str(&rest[..i]);
            rest = match rest[i..].find("*/") {
                Some(j) => &rest[i + j + 2..],
                None => "",
            };
        }
        css.push_str(rest);
        let mut rules = Vec::new();
        for chunk in css.split('}') {
            if let Some((sel, body)) = chunk.rsplit_once('{') {
                if sel.contains("ih-defcon-chip") {
                    rules.push((sel.trim().to_string(), body.trim().to_string()));
                }
            }
        }
        let neutral = rules
            .iter()
            .find(|(s, _)| s.ends_with(".ih-defcon-chip.is-neutral"))
            .expect("a .ih-defcon-chip.is-neutral rule");
        assert!(neutral.1.contains("var(--text-dim)"), "{neutral:?}");
        let amber = rules
            .iter()
            .find(|(s, _)| s.ends_with(".ih-defcon-chip.is-amber"))
            .expect("a .ih-defcon-chip.is-amber rule");
        assert!(amber.1.contains("var(--amber)"), "{amber:?}");
        assert!(
            amber.1.contains("color: var(--text)"),
            "the amber chip keeps its text in the text colour: {amber:?}"
        );
        for (sel, body) in &rules {
            assert!(!body.contains('#'), "`{sel}` has a hex literal: {body}");
        }
    }
}
