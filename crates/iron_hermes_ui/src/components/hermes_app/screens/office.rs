//! Office screen — ported from `app.html` `<section id="screen-office">`
//! (lines 1266-1353). The prototype shows a Claw3d spatial-rig stage
//! plus a side column with Devices + Calibration panels. The Devices
//! list is the dynamic part — sourced from `stub_data::office_workspaces()`
//! (which models the device mesh). The stage SVG and Calibration panel
//! are static markup since they have no per-row stub data factory.
//! Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{office_workspaces, OfficeWorkspaceStub};

#[component]
pub fn ScreenOffice(is_active: bool) -> Element {
    let workspaces = office_workspaces();
    let device_count = workspaces.len();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-office",
            "data-screen-label": "11 Office",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 11" }
                    h1 { class: "screen-title", "Office" }
                    p { class: "screen-sub",
                        "Claw3d spatial interface — physical desk overlay. Calibrate cameras, manage device meshes, mirror Hermes panels onto the surface."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⌖ CALIBRATE" }
                    button { class: "btn btn--sm", "▓ ENGAGE" }
                }
            }

            div { class: "split left-wider",
                div { class: "claw-stage",
                    div { class: "claw-grid-bg" }
                    div { class: "claw-readout",
                        span { class: "k", "RIG" }
                        " "
                        span { style: "color:var(--teal);font-weight:700", "CLAW3D-02" }
                        br {}
                        span { class: "k", "CAM" }
                        " "
                        span { style: "color:var(--teal);font-weight:700", "4 / 4 LOCKED" }
                        br {}
                        span { class: "k", "FPS" }
                        " "
                        span { style: "color:var(--teal);font-weight:700", "144" }
                    }
                }

                div { style: "display:flex;flex-direction:column;gap:14px;",
                    div { class: "panel",
                        div { class: "panel-title",
                            "Devices "
                            span { class: "count", style: "color:var(--teal)", "· {device_count}" }
                        }
                        div { class: "row-list",
                            for o in workspaces.iter() {
                                DeviceRow { key: "{o.name}", workspace: o.clone() }
                            }
                        }
                    }

                    div { class: "panel",
                        div { class: "panel-title", "Calibration" }
                        dl { class: "kv",
                            dt { "Rig" } dd { "CLAW3D-02" }
                            dt { "Last calib" } dd { "2026-05-12 · 22:18 UTC" }
                            dt { "Drift" } dd { style: "color:var(--green)", "0.04 px / hr" }
                            dt { "Surface" } dd { "matte 36\"×24\"" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            style: "width:100%;justify-content:center;",
                            "RE-CALIBRATE"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DeviceRow(workspace: OfficeWorkspaceStub) -> Element {
    let pill_class = match workspace.last_active {
        "LIVE" | "LOCKED" => "pill green",
        _ => "pill",
    };
    rsx! {
        div {
            class: "row",
            style: "grid-template-columns: 1fr auto; padding: 8px 10px;",
            div { class: "row-main",
                span { class: "row-title", "{workspace.name}" }
                span { class: "row-sub", "{workspace.kind}" }
            }
            span { class: "{pill_class}", "{workspace.last_active}" }
        }
    }
}
