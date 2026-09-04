//! Whitelist editor (E6, nested in E2) — add/remove-row editor for a chat
//! platform's sender whitelist, with the deny-all-when-empty warning
//! (`NO WHITELIST ENTRIES` / "Empty whitelist denies all senders..." per
//! `49.3-UI-SPEC.md`'s Copywriting Contract). Filled starting Plan 03 —
//! mirrors `tools/buzz_section.rs`'s `BuzzListEditor` shape (reused as a
//! pattern, not imported directly — that component is private to the Tools
//! screen).
//!
//! # Malformed-ID handling — FLAGGED ASSUMPTION (E6 partial, decided here)
//!
//! `49.3-CONTEXT.md` marks malformed-whitelist-entry handling "Claude's
//! Discretion" and leaves it unresolved in `49.3-UI-SPEC.md` (E6 `partial`
//! row). This plan DECIDES: **accept-and-flag**. A malformed entry (for a
//! numeric-ID platform — Telegram/Discord — an entry that does not parse
//! as `u64`) is accepted into the staged list and visibly flagged with a
//! `--red` inline hint on its row; it is NOT rejected client-side.
//!
//! There is no server-side rejection either (WR-03/WR-02 correction,
//! `49.3-REVIEW.md`): `platform_config_api::validate_and_normalize_entries`
//! checks entry count, per-entry length, embedded newlines and emptiness,
//! and inspects numeric format for no platform — so a malformed numeric ID
//! is ACCEPTED and written to `config.yaml` exactly as staged here. The
//! real enforcement is a fail-safe drop at the NEXT GATEWAY BOOT: Discord's
//! adapter construction parses each whitelist entry as `u64` and counts the
//! ones that fail into `unparsed_count`
//! (`ironhermes-gateway/src/runner.rs` ~:1295-1304), logging that count
//! (`tracing::warn!`) rather than erroring or refusing to start. That count
//! is not surfaced anywhere the web UI reads, which is why the client-side
//! `⚠ not a numeric ID` row hint below is currently the operator's only
//! warning that a staged entry will never grant access — surfacing the
//! drop count through the D-08 status heartbeat is deferred (see this
//! plan's `<deferred_items>` block; it widens a versioned cross-process
//! contract and is gateway-side work, not a doc fix).
//!
//! This keeps the editor a fast, forgiving staging area — the accept-and-
//! flag decision itself is unchanged and was made deliberately here, not
//! revisited by this correction.
//!
//! # Bounded scroll (E6 overflow backstop)
//!
//! More than 6 rows scroll inside `.gw-whitelist-rows` (a fixed max-height
//! and `overflow-y: auto`) rather than growing the card — the
//! `.mem-entry`/`.kv` list-density pattern's bounded-scroll shape, applied
//! to a NEW class (screens.css, on-grid `--sp-N` spacing per D-10).

#![allow(dead_code)] // ChatPlatformKind re-export path; consumed by chat_config_form.rs

use dioxus::prelude::*;

use super::chat_platform_cards::ChatPlatformKind;

/// Whether `kind`'s whitelist entries are expected to be numeric IDs
/// (Telegram chat IDs, Discord u64 snowflake IDs parsed at the runner
/// boundary) vs. freeform sender identifiers (Slack member IDs, Buzz hex
/// pubkeys). Pure and directly unit-testable — the ONE place this
/// distinction is decided.
fn kind_expects_numeric_id(kind: ChatPlatformKind) -> bool {
    matches!(kind, ChatPlatformKind::Telegram | ChatPlatformKind::Discord)
}

/// True when `entry` does not parse as the numeric ID format `kind`
/// expects — the accept-and-flag predicate (module doc). Always `false`
/// for a freeform-ID platform (Slack/Buzz) and for a blank entry (an
/// in-progress edit, not yet a real value to judge).
fn entry_is_malformed_for_kind(kind: ChatPlatformKind, entry: &str) -> bool {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return false;
    }
    kind_expects_numeric_id(kind) && trimmed.parse::<u64>().is_err()
}

/// The whitelist editor — takes the parent form's staged
/// `Signal<Vec<String>>` directly (`BuzzListEditor`'s precedent in
/// `tools/buzz_section.rs`: a row edit writes straight into the parent's
/// staged signal without an `EventHandler` round trip). `dirty` is bumped
/// on every add/edit/remove so the parent form's seed effect does not
/// clobber an in-progress edit.
#[component]
pub fn WhitelistEditor(
    mut items: Signal<Vec<String>>,
    kind: ChatPlatformKind,
    writable: bool,
    mut dirty: Signal<bool>,
) -> Element {
    let items_val = items.read().clone();
    let is_empty = items_val.is_empty();
    let numeric_hint = kind_expects_numeric_id(kind);

    rsx! {
        div { class: "gw-field-group",
            span { class: "gw-field-label", "WHITELIST" }
            if is_empty {
                // E6 empty: the deny-all-when-empty warning — Copywriting
                // Contract heading + body.
                div { class: "gw-whitelist-empty", role: "note",
                    p { style: "margin:0 0 4px 0; font-weight:700;", "NO WHITELIST ENTRIES" }
                    p { style: "margin:0;",
                        "Empty whitelist denies all senders on this platform — add at least one ID to allow messages through."
                    }
                }
            }
            p { class: "gw-field-help",
                if numeric_hint {
                    "Numeric IDs only — a Telegram chat ID or Discord user ID."
                } else {
                    "Freeform sender identifiers — a Slack member ID or Buzz hex pubkey."
                }
            }
            // E6 overflow backstop: bounded 6-row scroll inside the card
            // rather than growing it (.gw-whitelist-rows, screens.css).
            div { class: "gw-whitelist-rows ih-scroll",
                for (i , item) in items_val.iter().cloned().enumerate() {
                    {
                        let malformed = entry_is_malformed_for_kind(kind, &item);
                        rsx! {
                            div { key: "{i}", class: "gw-whitelist-row",
                                input {
                                    class: "gw-input",
                                    // E6 long-text: long IDs truncate with
                                    // ellipsis (.gw-input's text-overflow
                                    // rule) + the native title attribute
                                    // carries the full value; the input
                                    // itself stays fully editable.
                                    title: "{item}",
                                    "aria-label": "Whitelist entry {i} for {kind.display_name()}",
                                    disabled: !writable,
                                    value: "{item}",
                                    oninput: move |evt| {
                                        if !writable {
                                            return;
                                        }
                                        if let Some(slot) = items.write().get_mut(i) {
                                            *slot = evt.value();
                                        }
                                        dirty.set(true);
                                    },
                                }
                                if malformed {
                                    span {
                                        class: "gw-whitelist-row-flag",
                                        title: "⚠ not a numeric ID — this entry will be dropped at gateway start and will not grant access",
                                        "⚠ not a numeric ID — this entry will be dropped at gateway start and will not grant access"
                                    }
                                }
                                if writable {
                                    button {
                                        r#type: "button",
                                        class: "btn btn--ghost btn--sm",
                                        "aria-label": "Remove whitelist entry {i}",
                                        onclick: move |_| {
                                            {
                                                let mut list = items.write();
                                                if i < list.len() {
                                                    list.remove(i);
                                                }
                                            }
                                            dirty.set(true);
                                        },
                                        "×"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if writable {
                button {
                    r#type: "button",
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| {
                        items.write().push(String::new());
                        dirty.set(true);
                    },
                    "ADD ENTRY"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_and_discord_expect_numeric_ids() {
        assert!(kind_expects_numeric_id(ChatPlatformKind::Telegram));
        assert!(kind_expects_numeric_id(ChatPlatformKind::Discord));
    }

    #[test]
    fn slack_and_buzz_expect_freeform_ids() {
        assert!(!kind_expects_numeric_id(ChatPlatformKind::Slack));
        assert!(!kind_expects_numeric_id(ChatPlatformKind::Buzz));
    }

    #[test]
    fn numeric_platform_flags_a_non_numeric_entry() {
        assert!(entry_is_malformed_for_kind(
            ChatPlatformKind::Discord,
            "not-a-number"
        ));
        assert!(!entry_is_malformed_for_kind(
            ChatPlatformKind::Discord,
            "123456789"
        ));
    }

    #[test]
    fn freeform_platform_never_flags_anything() {
        assert!(!entry_is_malformed_for_kind(
            ChatPlatformKind::Slack,
            "not-a-number"
        ));
        assert!(!entry_is_malformed_for_kind(
            ChatPlatformKind::Buzz,
            "abcHEXpubkey"
        ));
    }

    #[test]
    fn a_blank_in_progress_entry_is_never_flagged() {
        assert!(!entry_is_malformed_for_kind(ChatPlatformKind::Telegram, ""));
        assert!(!entry_is_malformed_for_kind(
            ChatPlatformKind::Telegram,
            "   "
        ));
    }
}
