use dioxus::prelude::*;

/// IH brand stamp. Used in the title bar (size 18) and the agent side panel
/// head (size 20). Default size 26 if rendered standalone.
///
/// Port of `warp2ironhermes/project/app/shell.jsx` lines 53-59 per CONTEXT D-01.
/// Font-size is `size * 0.46` per the React source's inline style; rounded
/// to nearest u16 px.
#[component]
pub fn Sigil(size: u16) -> Element {
    let font_size = (size as f32 * 0.46).round() as u16;
    rsx! {
        span {
            class: "wh-sigil",
            style: "width: {size}px; height: {size}px; font-size: {font_size}px;",
            "IH"
        }
    }
}
