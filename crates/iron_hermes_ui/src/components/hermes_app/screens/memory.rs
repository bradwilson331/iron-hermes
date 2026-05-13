//! Memory screen — ported from `app.html` `<section id="screen-memory">`
//! (lines 797-882). Renders memory entries from `stub_data::memory_entries()`
//! plus the static user-profile + provider side panels from the prototype.
//! Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{memory_entries, MemoryEntryStub};

#[component]
pub fn ScreenMemory(is_active: bool) -> Element {
    let entries = memory_entries();
    let entry_count = entries.len();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-memory",
            "data-screen-label": "06 Memory",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 06" }
                    h1 { class: "screen-title", "Memory" }
                    p { class: "screen-sub",
                        "Persistent context the active agent recalls across sessions — entries, user profile, and embedding-store provider."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "EXPORT" }
                    button { class: "btn btn--sm", "+ ENTRY" }
                }
            }

            div { class: "split",
                div { class: "panel",
                    div { style: "display:flex;justify-content:space-between;align-items:center;",
                        div { class: "panel-title",
                            "Entries "
                            span { class: "count", style: "color:var(--teal)", "· {entry_count}" }
                        }
                        div { class: "search", style: "width: 220px; padding: 5px 12px;",
                            span { class: "search-glyph", "⌕" }
                            input { placeholder: "Filter…" }
                        }
                    }
                    div { class: "row-list", style: "gap: 0;",
                        for e in entries.iter() {
                            MemoryEntryRow {
                                key: "{e.scope}::{e.key}",
                                entry: e.clone()
                            }
                        }
                    }
                }

                div { style: "display: flex; flex-direction: column; gap: 14px;",
                    div { class: "panel",
                        div { class: "panel-title", "User Profile" }
                        dl { class: "kv",
                            dt { "Operator" } dd { "ALPHA-7" }
                            dt { "Locale" } dd { "en-US · America/Los_Angeles" }
                            dt { "Role" } dd { "Intelligence Analyst" }
                            dt { "Clearance" } dd { style: "color:var(--teal)", "L4" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            style: "width:100%;justify-content:center;",
                            "EDIT PROFILE"
                        }
                    }
                    div { class: "panel",
                        div { class: "panel-title", "Provider" }
                        dl { class: "kv",
                            dt { "Store" } dd { "Qdrant (local)" }
                            dt { "Embed Model" } dd { "all-mpnet-base-v2" }
                            dt { "Dimensions" } dd { "768" }
                            dt { "Entries" } dd { "412 · 14.2 MB" }
                            dt { "Retention" } dd { "infinite" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            style: "width:100%;justify-content:center;",
                            "RECONFIGURE"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MemoryEntryRow(entry: MemoryEntryStub) -> Element {
    rsx! {
        div { class: "mem-entry",
            div { class: "mem-ts", "{entry.updated}" }
            div { class: "mem-body",
                "{entry.value_preview} "
                span { class: "mem-tag", "{entry.scope}" }
            }
        }
    }
}
