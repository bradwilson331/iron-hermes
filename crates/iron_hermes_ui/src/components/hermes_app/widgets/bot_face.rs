//! Phase 50.1 Plan 04 (D-12): deterministic geometric avatar renderer.
//!
//! **Canonical source (D-03).** `sigil_rng` and `seeded_shape_for` are
//! ported one-to-one from the live in-tree Bot Mode plugin at
//! `~/code/hermes-agent/apps/desktop/src/plugins/hermes-bots/plugin.js`
//! (lines ~742-828 at the time of this port), NOT from the archived clone
//! in this phase's directory. Porting faithfully is the whole point — a
//! rewrite here must never change any existing bot's face.
//!
//! **Two distinct shape pools** (plugin.js:742-743): [`AVATAR_SHAPES`] is
//! the seven-entry auto-assignment pool; [`AVATAR_PICKER_SHAPES`] is those
//! seven plus `blob`, the picker-only eighth shape. [`seeded_shape_for`]
//! draws ONLY from the seven-entry pool — a bot is never auto-assigned
//! `blob`. This is the resolution of an apparent discrepancy between the
//! operator-supplied how-to article (which shows all eight picker shapes)
//! and research notes describing only seven auto-assignable ones — both
//! are correct, for different surfaces.
//!
//! **Palette re-derivation is deliberate** (UI-SPEC Color section). The
//! plugin's own ten-hex `AVATAR_COLORS` array is a Tailwind-app artifact.
//! Every identity colour here ([`AVATAR_COLOR_TOKENS`]) is an existing
//! named CSS custom-property token in THIS crate — accent, semantic and
//! brand roles doing double duty as identity colours — verified present in
//! `design-tokens.css`. [`seeded_color_for`] is therefore new code, not a
//! port: the plugin's own colour assignment goes through the desktop SDK's
//! `profileColor` helper, which is imported from an external SDK with no
//! in-tree source to port from. It reuses [`sigil_rng`] (already proven
//! deterministic and range-bound) with a decorrelating seed suffix, rather
//! than inventing a second PRNG.
//!
//! `BotFace` is a pure, prop-driven render component — no signal, no
//! context lookup, and it opens no shared context of its own. All of this
//! crate's shared context providers live at the `HermesApp` root (see
//! `widgets.rs`'s own module doc); a provider scoped here would compile
//! green and panic its consumers at runtime.
//!
//! This crate reserves inline SVG for exactly this kind of generated
//! geometry — the radial dial in `wheel.rs` is the one existing precedent
//! — and uses Unicode glyphs for controls everywhere else. No icon font or
//! icon library is introduced here.

use dioxus::prelude::*;

/// The seven-entry auto-assignment pool — [`seeded_shape_for`] draws only
/// from these. Ported verbatim from plugin.js's `AVATAR_SHAPES`.
// Under `--all-features`, `legacy-shell` swaps the root to WarpHermes,
// leaving HermesApp (and therefore BotFace and everything it calls)
// unreferenced from `main()` — a binary crate's unreferenced item does not
// suppress dead_code, so the `-D warnings` clippy gate fires there even
// though this is live in the default (non-legacy-shell) build. Same
// pattern as `KANBAN_CSS` in `screens/kanban.rs` and `ARTIFACT_SANDBOX` in
// `screens/artifact_viewer.rs`.
#[allow(dead_code)]
pub const AVATAR_SHAPES: [&str; 7] =
    ["circle", "squircle", "pill", "triangle", "hexagon", "cloud", "drop"];

/// The eight-entry picker set — [`AVATAR_SHAPES`] plus `blob`, the
/// picker-only extra shape. Ported verbatim from plugin.js's
/// `AVATAR_PICKER_SHAPES`. A strict superset of `AVATAR_SHAPES`.
#[allow(dead_code)] // see AVATAR_SHAPES's doc note above (legacy-shell reachability)
pub const AVATAR_PICKER_SHAPES: [&str; 8] =
    ["circle", "blob", "squircle", "pill", "triangle", "hexagon", "cloud", "drop"];

/// The eight identity colour token NAMES (UI-SPEC Color section's identity
/// table) — never a bare hex value. Every entry is confirmed present in
/// `design-tokens.css` at plan-authoring time; accent/semantic/brand roles
/// doing double duty as identity colours, per the re-derivation note above.
#[allow(dead_code)] // see AVATAR_SHAPES's doc note above (legacy-shell reachability)
pub const AVATAR_COLOR_TOKENS: [&str; 8] = [
    "--accent-primary",
    "--accent-secondary",
    "--success",
    "--warn",
    "--danger",
    "--brand",
    "--ansi-blue",
    "--fg-strong",
];

/// String-seeded xorshift PRNG — ported verbatim from plugin.js's
/// `sigilRng`: fold the seed with the FNV-1a offset basis/prime (iterating
/// Unicode scalar values, mirroring JS's `charCodeAt` iteration over
/// `for...of`), then on each call shift-xor left 13, right 17, left 5, and
/// divide by 2^32. Every returned value falls in the half-open unit
/// interval `[0, 1)`. Pure and side-effect-free.
#[allow(dead_code)] // see AVATAR_SHAPES's doc note above (legacy-shell reachability)
pub fn sigil_rng(seed: &str) -> impl FnMut() -> f64 {
    let mut h: u32 = 2166136261;
    for ch in seed.chars() {
        h ^= ch as u32;
        h = h.wrapping_mul(16777619);
    }
    let mut state: u32 = if h != 0 { h } else { 88675123 };
    move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f64) / 4_294_967_296.0
    }
}

/// Stable shape assignment for a bot name — ported verbatim from
/// plugin.js's `defaultShapeFor`: a multiply-by-31 rolling hash over the
/// name's Unicode scalar values, modulo the seven-entry [`AVATAR_SHAPES`]
/// pool length. Draws ONLY from the seven-entry pool (see module doc) —
/// never returns `blob`. Pure and deterministic: the same name always
/// yields the same shape.
#[allow(dead_code)] // see AVATAR_SHAPES's doc note above (legacy-shell reachability)
pub fn seeded_shape_for(name: &str) -> &'static str {
    let mut hash: u32 = 0;
    for ch in name.chars() {
        hash = hash.wrapping_mul(31).wrapping_add(ch as u32);
    }
    AVATAR_SHAPES[(hash as usize) % AVATAR_SHAPES.len()]
}

/// Stable colour-token assignment for a bot name (new code — see module
/// doc). Reuses [`sigil_rng`], seeded with a decorrelating suffix so a
/// bot's shape hash and colour hash never move in lockstep. Pure and
/// deterministic.
#[allow(dead_code)] // see AVATAR_SHAPES's doc note above (legacy-shell reachability)
pub fn seeded_color_for(name: &str) -> &'static str {
    let mut rng = sigil_rng(&format!("{name}::color"));
    let raw = (rng() * AVATAR_COLOR_TOKENS.len() as f64) as usize;
    AVATAR_COLOR_TOKENS[raw.min(AVATAR_COLOR_TOKENS.len() - 1)]
}

/// Shape name to static SVG `<path>` `d` data, all drawn within a shared
/// `0 0 40 40` viewBox. Every entry in [`AVATAR_PICKER_SHAPES`] resolves to
/// non-empty path data; an unrecognized shape name (never produced by this
/// module's own two pools, but defensively handled for a stored value from
/// an older or hand-edited descriptor) falls back to the circle path —
/// mirrors plugin.js's `shapeNode` default arm, which also falls back to a
/// plain circle.
///
/// `squircle` and `pill` are rounded-rect *paths* here (not `<rect>`
/// elements) so every shape renders through the same single `<path>`
/// element in [`BotFace`] — a deliberate simplification over plugin.js's
/// separate `rect`/`path` element choice, since this port has no reason to
/// carry that DOM-shape distinction forward.
#[allow(dead_code)] // see AVATAR_SHAPES's doc note above (legacy-shell reachability)
pub fn shape_svg_path(shape: &str) -> &'static str {
    match shape {
        "blob" => {
            "M20.00 2.30L22.17 2.14L24.29 2.59L26.19 3.67L27.75 5.24L28.91 7.10L29.75 8.99L30.47 10.73\
             L31.24 12.24L32.21 13.59L33.40 14.92L34.68 16.38L35.86 18.07L36.70 20.00L37.05 22.07\
             L36.87 24.16L36.21 26.15L35.20 27.98L33.95 29.63L32.56 31.13L31.03 32.46L29.37 33.57\
             L27.56 34.40L25.63 34.86L23.68 34.94L21.78 34.70L20.00 34.30L18.31 33.91L16.63 33.66\
             L14.85 33.59L12.88 33.57L10.73 33.43L8.53 32.94L6.51 31.95L4.90 30.42L3.87 28.46\
             L3.48 26.27L3.61 24.04L4.09 21.93L4.70 20.00L5.29 18.21L5.80 16.50L6.29 14.80L6.86 13.11\
             L7.62 11.45L8.61 9.91L9.81 8.50L11.19 7.24L12.68 6.06L14.29 4.94L16.02 3.86L17.93 2.93Z"
        }
        "squircle" => {
            "M14 3 H26 A11 11 0 0 1 37 14 V26 A11 11 0 0 1 26 37 H14 A11 11 0 0 1 3 26 \
             V14 A11 11 0 0 1 14 3 Z"
        }
        "pill" => {
            "M15 7 H25 A13 13 0 0 1 38 20 V20 A13 13 0 0 1 25 33 H15 A13 13 0 0 1 2 20 \
             V20 A13 13 0 0 1 15 7 Z"
        }
        "triangle" => "M20 5.5 L36 33.5 L4 33.5 Z",
        "hexagon" => "M20 3.5 L34.5 11.75 L34.5 28.25 L20 36.5 L5.5 28.25 L5.5 11.75 Z",
        "cloud" => "M11 32 a7.5 7.5 0 0 1 -1 -14.9 A9.5 9.5 0 0 1 29 12.5 A7 7 0 0 1 30 32 Z",
        "drop" => "M20 3 C20 3 6 20 6 27 a14 13.5 0 0 0 28 0 C34 20 20 3 20 3 Z",
        // "circle" and every unrecognized name.
        _ => "M2.5 20 A17.5 17.5 0 1 1 37.5 20 A17.5 17.5 0 1 1 2.5 20 Z",
    }
}

/// Phase 50.1 Plan 04 (D-12): one bot's avatar. When `image_id` is present
/// it renders an `<img>` pointing at the byte-serving route
/// (`serve_bot_avatar`, `server/bot_avatar_route.rs`) instead of the
/// geometry — an uploaded or AI-generated raster avatar never carries the
/// work-state animation (there is no eye element to widen on a photo; this
/// is an accepted, not a deferred, limitation).
///
/// Otherwise it renders the deterministic geometric face: `shape`/
/// `color_token` default to [`seeded_shape_for`]/[`seeded_color_for`] over
/// `name` when absent, so a new bot with no stored descriptor still
/// renders a stable, name-derived face rather than one hardcoded default
/// (UI-SPEC E5 empty). The shape fill uses `color-mix(in srgb,
/// var({token}) 15%, var(--w-bg-3))` — the same primary-CTA fill formula
/// `.kn-modal-btn--submit` already uses in this crate — and the outline
/// plus eye elements render at the token's full opacity.
///
/// Work state: when `mood` is `"work"`, the container carries
/// `data-mood="work"` and sets `--kn-avatar-eye-ry` — the eye ellipses'
/// `style` reads that custom property (falling back to the idle radius),
/// widening them by the plugin's own ~13% ratio. No glow, pulse or
/// spinner — deliberately subtle.
#[component]
pub fn BotFace(
    name: String,
    size: u32,
    #[props(default)] shape: Option<String>,
    #[props(default)] color_token: Option<String>,
    #[props(default)] mood: Option<String>,
    #[props(default)] image_id: Option<String>,
    // Phase 50.2 Plan 17 (G-50.2-2a): an optional accessible name/hover
    // affordance for the avatar container. `None` (every existing call
    // site) renders byte-for-byte today's markup — `#[props(default)]`
    // guarantees every untouched caller is unaffected. `Some(name)` puts
    // that string on `.kn-bot-avatar` as BOTH `title` (hover) and
    // `aria-label` (accessible name), with `role: "img"` so the label has
    // something to attach to — the inner SVG stays `aria-hidden`, so there
    // is exactly one accessible name for the element, not two.
    #[props(default)] title: Option<String>,
) -> Element {
    let is_work = mood.as_deref() == Some("work");
    let has_title = title.is_some();

    if let Some(id) = image_id.as_ref() {
        let alt = format!("{name}'s avatar");
        // Uploaded/AI-generated avatars never carry `data-mood` — there is
        // no eye element to widen on a raster image (accepted, not a
        // deferred, limitation, D-12). `is_work` (computed above) is read
        // only by the geometric branch below. The container keeps `alt` on
        // the inner `img` as-is and additionally carries the hover
        // affordance/accessible name when `title` is supplied.
        return rsx! {
            div {
                class: "kn-bot-avatar",
                "data-size": "{size}",
                title: if let Some(t) = &title { "{t}" },
                "aria-label": if let Some(t) = &title { "{t}" },
                role: if has_title { "img" },
                img {
                    src: "/bot-avatars/{name}/{id}",
                    alt: "{alt}",
                    width: "{size}",
                    height: "{size}",
                }
            }
        };
    }

    let resolved_shape = shape.unwrap_or_else(|| seeded_shape_for(&name).to_string());
    let resolved_token = color_token.unwrap_or_else(|| seeded_color_for(&name).to_string());
    let body_path = shape_svg_path(&resolved_shape);
    let fill = format!("color-mix(in srgb, var({resolved_token}) 15%, var(--w-bg-3))");
    let full_opacity = format!("var({resolved_token})");
    // `--kn-avatar-eye-ry` is set on the svg element only in the work
    // state; each eye's own `style` reads it with a `2.3px` idle fallback,
    // so the idle render never depends on this attribute being present.
    let eye_style_var = if is_work { "--kn-avatar-eye-ry: 2.6px;" } else { "" };

    rsx! {
        div {
            class: "kn-bot-avatar",
            "data-size": "{size}",
            "data-mood": if is_work { "work" },
            title: if let Some(t) = &title { "{t}" },
            "aria-label": if let Some(t) = &title { "{t}" },
            role: if has_title { "img" },
            svg {
                "viewBox": "0 0 40 40",
                width: "{size}",
                height: "{size}",
                "aria-hidden": "true",
                style: "{eye_style_var}",
                path {
                    d: "{body_path}",
                    fill: "{fill}",
                    stroke: "{full_opacity}",
                    "stroke-width": "1.5",
                }
                ellipse {
                    cx: "15.4",
                    cy: "17",
                    rx: "2.2",
                    style: "ry: var(--kn-avatar-eye-ry, 2.3px);",
                    fill: "{full_opacity}",
                }
                ellipse {
                    cx: "24.6",
                    cy: "17",
                    rx: "2.2",
                    style: "ry: var(--kn-avatar-eye-ry, 2.3px);",
                    fill: "{full_opacity}",
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // sigil_rng
    // -------------------------------------------------------------------

    #[test]
    fn sigil_rng_same_seed_produces_same_first_five_values_every_call() {
        let mut a = sigil_rng("scout");
        let mut b = sigil_rng("scout");
        let seq_a: Vec<f64> = (0..5).map(|_| a()).collect();
        let seq_b: Vec<f64> = (0..5).map(|_| b()).collect();
        assert_eq!(seq_a, seq_b);
    }

    #[test]
    fn sigil_rng_different_seed_produces_different_first_value() {
        let mut a = sigil_rng("scout");
        let mut b = sigil_rng("ranger");
        assert_ne!(a(), b());
    }

    #[test]
    fn sigil_rng_values_fall_in_half_open_unit_interval() {
        let mut rng = sigil_rng("unit-interval-check");
        for _ in 0..200 {
            let v = rng();
            assert!((0.0..1.0).contains(&v), "value {v} out of [0, 1)");
        }
    }

    // -------------------------------------------------------------------
    // seeded_shape_for
    // -------------------------------------------------------------------

    #[test]
    fn seeded_shape_for_scout_is_stable_and_in_the_seven_entry_pool() {
        let first = seeded_shape_for("scout");
        let second = seeded_shape_for("scout");
        assert_eq!(first, second);
        assert!(AVATAR_SHAPES.contains(&first));
    }

    #[test]
    fn seeded_shape_for_never_returns_the_picker_only_extra_shape() {
        for i in 0..100 {
            let name = format!("bot-{i}");
            let shape = seeded_shape_for(&name);
            assert_ne!(shape, "blob", "seeded_shape_for must never assign the picker-only shape");
            assert!(AVATAR_SHAPES.contains(&shape), "{shape} must be a member of AVATAR_SHAPES");
        }
    }

    // -------------------------------------------------------------------
    // seeded_color_for
    // -------------------------------------------------------------------

    #[test]
    fn seeded_color_for_returns_a_member_of_the_token_list_and_is_stable() {
        let first = seeded_color_for("scout");
        let second = seeded_color_for("scout");
        assert_eq!(first, second);
        assert!(AVATAR_COLOR_TOKENS.contains(&first));
    }

    // -------------------------------------------------------------------
    // shape pools + palette
    // -------------------------------------------------------------------

    #[test]
    fn avatar_shapes_has_exactly_seven_entries() {
        assert_eq!(AVATAR_SHAPES.len(), 7);
    }

    #[test]
    fn avatar_picker_shapes_has_exactly_eight_entries() {
        assert_eq!(AVATAR_PICKER_SHAPES.len(), 8);
    }

    #[test]
    fn avatar_shapes_is_a_strict_subset_of_avatar_picker_shapes() {
        for shape in AVATAR_SHAPES {
            assert!(
                AVATAR_PICKER_SHAPES.contains(&shape),
                "{shape} from AVATAR_SHAPES must also be in AVATAR_PICKER_SHAPES"
            );
        }
        assert!(AVATAR_PICKER_SHAPES.contains(&"blob"));
        assert!(!AVATAR_SHAPES.contains(&"blob"));
    }

    #[test]
    fn shape_svg_path_returns_non_empty_path_data_for_every_picker_shape() {
        for shape in AVATAR_PICKER_SHAPES {
            let path = shape_svg_path(shape);
            assert!(!path.is_empty(), "{shape} must resolve to non-empty path data");
        }
    }

    #[test]
    fn avatar_color_tokens_has_exactly_eight_entries_and_every_entry_is_a_css_custom_property() {
        assert_eq!(AVATAR_COLOR_TOKENS.len(), 8);
        for token in AVATAR_COLOR_TOKENS {
            assert!(
                token.starts_with("--"),
                "{token} must be a CSS custom-property name, never a hex literal"
            );
        }
    }

    // -------------------------------------------------------------------
    // BotFace (rendered assertions belong to UAT — see plan verification;
    // these are the pure-logic behaviors this component composes from).
    // -------------------------------------------------------------------

    #[test]
    fn two_different_names_seed_to_different_shape_or_color() {
        // "Visibly different faces" is a UAT behavior; the pure guarantee
        // this unit test can make is that at least one of the two seeded
        // dimensions differs for two arbitrary distinct names — otherwise
        // every bot would render byte-identical geometry.
        let shape_a = seeded_shape_for("aurora");
        let color_a = seeded_color_for("aurora");
        let shape_b = seeded_shape_for("borealis");
        let color_b = seeded_color_for("borealis");
        assert!(shape_a != shape_b || color_a != color_b);
    }
}
