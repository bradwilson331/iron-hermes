//! Schedules screen — ported from `app.html` `<section id="screen-schedules">`
//! (lines 1074-1147). Renders cron rows from `stub_data::schedules()` in
//! the prototype's grid table (.row-list / .sched-row / .sched-cron / .pill).
//! Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{schedules, ScheduleStub};

#[component]
pub fn ScreenSchedules(is_active: bool) -> Element {
    let schedule_list = schedules();
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-schedules",
            "data-screen-label": "09 Schedules",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 09" }
                    h1 { class: "screen-title", "Schedules" }
                    p { class: "screen-sub",
                        "Cron-driven jobs with delivery targets. Hermes runs the prompt, formats the output, and sends it where you choose."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⏵ HISTORY" }
                    button { class: "btn btn--sm", "+ NEW JOB" }
                }
            }

            div { class: "row-list",
                div { class: "sched-row head",
                    span {}
                    span { "JOB" }
                    span { "SCHEDULE" }
                    span { "DELIVERY" }
                    span { "LAST RUN" }
                    span { style: "text-align:right;", "STATE" }
                }
                for sc in schedule_list.iter() {
                    ScheduleRow { key: "{sc.id}", schedule: sc.clone() }
                }
            }
        }
    }
}

#[component]
fn ScheduleRow(schedule: ScheduleStub) -> Element {
    let (dot_color, pill_class) = match schedule.status {
        "ACTIVE" => ("var(--green)", "pill green"),
        "PAUSED" => ("var(--amber)", "pill amber"),
        _ => ("var(--gray)", "pill"),
    };
    rsx! {
        div { class: "sched-row",
            span { style: "color:{dot_color};", "●" }
            div { class: "row-main",
                span { class: "row-title", "{schedule.id}" }
                span { class: "row-sub", "—" }
            }
            span { class: "sched-cron", "{schedule.cron}" }
            span { class: "row-sub", "{schedule.target}" }
            span { class: "row-sub", "{schedule.last_run}" }
            span {
                class: "{pill_class}",
                style: "justify-self:end;",
                "{schedule.status}"
            }
        }
    }
}
