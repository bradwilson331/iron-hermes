//! Phase 01 (Three.js Viseme Avatar Core) — pure-logic seams for the avatar.
//!
//! ironhermes has no JS test runner (per the phase VALIDATION.md), so the
//! three hardest-to-eyeball avatar behaviors are implemented here as pure,
//! deterministic Rust functions and unit-tested with `#[cfg(test)]`. The JS
//! render module (`assets/avatar.js`, Plan 03) ports these verbatim, using
//! this file as its 1:1 reference:
//!
//! - [`apply_visemes`] — the Oculus-viseme → ARKit-blendshape mapping
//!   (VIS-03 / D-06). The [`OCULUS_TO_ARKIT`] table is copied byte-for-byte
//!   from the reviewed `01-RESEARCH.md` verified table (met4citizen
//!   `build-visemes-from-arkit.py`); the weights are NOT hand-tuned. This is
//!   the Rust reference for `assets/viseme-map.js`.
//! - [`select_avatar_global`] — the throwaway dev-flag → JS-global selection
//!   (D-10 / REND-01). Returns the exact string literals Plan 04's
//!   `document::eval` init/pump/destroy/setState strings interpolate.
//! - [`reduced_motion_suppressed`] — the reduced-motion gate (REND-03),
//!   mirroring `orb.js`'s `resolveToken(canvas, '--orb-motion', 'full')
//!   === 'none'` contract (`.trim()` + equality with `"none"`).
//!
//! The table indirection is kept so swapping to a Ready Player Me head in
//! Phase 2 only replaces [`OCULUS_TO_ARKIT`] (Oculus→Oculus near-identity),
//! not the apply logic (D-02, RPM-ready).
//!
//! These items are this phase's Wave-0 testable seams: they are exercised by
//! the `#[cfg(test)]` module below and consumed by later plans (the JS render
//! module in Plan 03 ports `apply_visemes`/the tables; `orb_canvas.rs` in
//! Plan 04 calls `select_avatar_global`/`reduced_motion_suppressed` from the
//! `document::eval` plumbing). Until that wiring lands they are not referenced
//! from the non-test binary, so allow `dead_code` here — matching the host's
//! `#[allow(dead_code)]` idiom for forward-referenced items in this module.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// JS global name for the avatar render module (mirrors `window.ironHermesOrb`).
pub const AVATAR_GLOBAL: &str = "ironHermesAvatar";

/// JS global name for the existing orb render module.
pub const ORB_GLOBAL: &str = "ironHermesOrb";

/// Mouth-region ARKit blendshapes that [`apply_visemes`] zeroes each call
/// before re-applying the current frame's viseme contributions, so a prior
/// frame's mouth shape does not persist into silence (Pitfall 6 / D-06).
///
/// Verbatim from `01-RESEARCH.md` §"Viseme → Blendshape Mapping … VERIFIED
/// TABLE". Blink/breath/idle shapes are deliberately NOT in this set so idle
/// motion is never clobbered by a mouth write.
pub const MOUTH_ARKIT_SHAPES: &[&str] = &[
    "jawOpen",
    "jawForward",
    "mouthPucker",
    "mouthFunnel",
    "mouthShrugUpper",
    "mouthRollUpper",
    "mouthRollLower",
    "mouthPressLeft",
    "mouthPressRight",
    "mouthDimpleLeft",
    "mouthDimpleRight",
    "mouthLowerDownLeft",
    "mouthLowerDownRight",
    "mouthUpperUpLeft",
    "mouthUpperUpRight",
    "tongueOut",
];

/// The verified Oculus-viseme → ARKit-blendshape composition table
/// (VIS-03 / D-06). wawa-lipsync emits the 15 Oculus visemes; `facecap.glb`
/// is ARKit-52, so this map sits between the driver and
/// `morphTargetInfluences`. Weights are additive influence contributions,
/// clamped to `[0, 1]` per-shape by [`apply_visemes`].
///
/// **Copied byte-for-byte** from `01-RESEARCH.md` §"VERIFIED TABLE"
/// (met4citizen/TalkingHead `build-visemes-from-arkit.py`) — do NOT
/// hand-tune (D-06). `viseme_sil` (silence) maps to no shapes and is
/// therefore intentionally absent from this table; silence produces an
/// all-zero mouth because [`apply_visemes`] zeroes [`MOUTH_ARKIT_SHAPES`]
/// first and `viseme_sil` adds nothing.
///
/// Entries are returned as a [`BTreeMap`] for deterministic iteration order
/// (test stability), built once per call from these literals.
pub const OCULUS_TO_ARKIT: &[(&str, &[(&str, f32)])] = &[
    ("viseme_aa", &[("jawOpen", 0.6)]),
    (
        "viseme_E",
        &[
            ("mouthPressLeft", 0.8),
            ("mouthPressRight", 0.8),
            ("mouthDimpleLeft", 1.0),
            ("mouthDimpleRight", 1.0),
            ("jawOpen", 0.3),
        ],
    ),
    (
        "viseme_I",
        &[
            ("mouthPressLeft", 0.6),
            ("mouthPressRight", 0.6),
            ("mouthDimpleLeft", 0.6),
            ("mouthDimpleRight", 0.6),
            ("jawOpen", 0.2),
        ],
    ),
    (
        "viseme_O",
        &[("mouthPucker", 1.0), ("jawForward", 0.6), ("jawOpen", 0.2)],
    ),
    ("viseme_U", &[("mouthFunnel", 1.0)]),
    (
        "viseme_PP",
        &[
            ("mouthRollLower", 0.8),
            ("mouthRollUpper", 0.8),
            ("mouthUpperUpLeft", 0.3),
            ("mouthUpperUpRight", 0.3),
        ],
    ),
    (
        "viseme_FF",
        &[
            ("mouthPucker", 1.0),
            ("mouthShrugUpper", 1.0),
            ("mouthLowerDownLeft", 0.2),
            ("mouthLowerDownRight", 0.2),
            ("mouthDimpleLeft", 1.0),
            ("mouthDimpleRight", 1.0),
            ("mouthRollLower", 1.0),
        ],
    ),
    (
        "viseme_DD",
        &[
            ("mouthPressLeft", 0.8),
            ("mouthPressRight", 0.8),
            ("mouthFunnel", 0.5),
            ("jawOpen", 0.2),
        ],
    ),
    (
        "viseme_SS",
        &[
            ("mouthPressLeft", 0.8),
            ("mouthPressRight", 0.8),
            ("mouthLowerDownLeft", 0.5),
            ("mouthLowerDownRight", 0.5),
            ("jawOpen", 0.1),
        ],
    ),
    (
        "viseme_TH",
        &[
            ("mouthRollUpper", 0.6),
            ("jawOpen", 0.2),
            ("tongueOut", 0.4),
        ],
    ),
    ("viseme_CH", &[("mouthPucker", 0.5), ("jawOpen", 0.2)]),
    ("viseme_RR", &[("mouthPucker", 0.5), ("jawOpen", 0.2)]),
    (
        "viseme_kk",
        &[
            ("mouthLowerDownLeft", 0.4),
            ("mouthLowerDownRight", 0.4),
            ("mouthDimpleLeft", 0.3),
            ("mouthDimpleRight", 0.3),
            ("mouthFunnel", 0.3),
            ("mouthPucker", 0.3),
            ("jawOpen", 0.15),
        ],
    ),
    (
        "viseme_nn",
        &[
            ("mouthLowerDownLeft", 0.4),
            ("mouthLowerDownRight", 0.4),
            ("mouthDimpleLeft", 0.3),
            ("mouthDimpleRight", 0.3),
            ("mouthFunnel", 0.3),
            ("mouthPucker", 0.3),
            ("jawOpen", 0.15),
            ("tongueOut", 0.2),
        ],
    ),
];

/// Apply one frame of Oculus-viseme scores to the ARKit `influences` slice,
/// in place. Pure; no Three.js (VIS-03 / D-06).
///
/// Steps (the exact sequence `assets/avatar.js`'s `applyVisemes` ports):
/// 1. Zero every [`MOUTH_ARKIT_SHAPES`] entry whose name exists in
///    `arkit_index` (so a prior frame's mouth shape does not persist).
/// 2. For each `(oculus, mix)` in [`OCULUS_TO_ARKIT`], read the score from
///    `scores`; skip if absent or zero.
/// 3. For each `(arkit, weight)` in the mix, accumulate
///    `influences[idx] = (influences[idx] + score * weight).min(1.0)`,
///    guarded by `arkit_index.get(arkit)` so an ARKit shape missing from the
///    loaded GLB degrades gracefully (Assumption A2) instead of panicking.
///
/// `arkit_index` maps an ARKit blendshape name (e.g. `"jawOpen"`) to its
/// index into `influences` (built from the GLB's `morphTargetDictionary`,
/// stripping the `"blendShape1."` prefix — see Pattern 1).
pub fn apply_visemes(
    scores: &BTreeMap<&str, f32>,
    arkit_index: &BTreeMap<&str, usize>,
    influences: &mut [f32],
) {
    // Zero the mouth region each call (guarded for missing/out-of-range idx).
    for shape in MOUTH_ARKIT_SHAPES {
        if let Some(&idx) = arkit_index.get(shape) {
            if idx < influences.len() {
                influences[idx] = 0.0;
            }
        }
    }

    // Accumulate this frame's viseme contributions, clamped to [0, 1].
    for (oculus, mix) in OCULUS_TO_ARKIT {
        let score = match scores.get(oculus) {
            Some(&s) if s != 0.0 => s,
            _ => continue,
        };
        for (arkit, weight) in *mix {
            if let Some(&idx) = arkit_index.get(arkit) {
                if idx < influences.len() {
                    influences[idx] = (influences[idx] + score * weight).min(1.0);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Head preset registry (Phase 40.2, ID-01, D-07, D-09)
// ---------------------------------------------------------------------------

/// Per-preset camera framing constants (D-09).
///
/// Values are applied by `avatar.js`'s `init(canvasId, glbUrl, framing)` third
/// parameter via `camera.position.set(cam_pos)` / `camera.lookAt(look_at)`.
/// Using compile-time constants here keeps the registry data-driven and
/// testable without any Three.js / WASM dependency.
pub struct FramingData {
    /// Camera world-space position `(x, y, z)`.
    pub cam_pos: (f32, f32, f32),
    /// Camera look-at target `(x, y, z)`.
    pub look_at: (f32, f32, f32),
    /// Vertical field of view in degrees (perspective "lens length").
    ///
    /// 45.0 is the historical default. Close-framed face presets need a
    /// NARROW fov with the camera pulled proportionally back: a wide fov
    /// at short range renders a face with selfie/fisheye distortion (the
    /// nose is much nearer the camera than the ears, so it looms). Keep
    /// `2 * dist * tan(fov/2)` constant to change lens without changing
    /// the crop.
    pub fov: f32,
}

/// Rough body-type classification for the head preset (D-07).
///
/// Used by downstream UI (Plan 04 dropdown) and future plans that may need
/// to adjust orb-canvas crop or aspect ratio per body type.
pub enum BodyType {
    /// Head-only model (e.g. FaceCap — no shoulders visible).
    Head,
    /// Half-body model (e.g. RPM — head + shoulders).
    Half,
    /// Full-body model (reserved for future presets).
    Full,
}

/// Per-preset material treatment applied by `avatar.js` at load
/// (spec 2026-07-13, serialized as "normal" | "pbr" | "matrix").
pub enum MaterialKind {
    /// MeshNormalMaterial override (facecap's existing behavior).
    Normal,
    /// Keep the GLB's own PBR materials + hemisphere/fill lights (groovy).
    Pbr,
    /// Keep the GLB's own emissive material, skip fill lights, state-tint
    /// the emissive, enable the dissolve shader injection (matrix).
    MatrixHologram,
}

/// Oculus-viseme → GLB morph-name map for the Groovy avatar preset (ID-01).
///
/// The Groovy model has no ARKit shapes; it ships pre-composed whole-viseme
/// morphs under display names. `avatar.js` uses this map to drive the
/// dominant Oculus viseme morph directly (at weight 1.0), bypassing the
/// `OCULUS_TO_ARKIT` decomposition path used for `facecap`. Names are exact
/// (case + spaces) as they appear in the GLB `extras.targetNames` /
/// three.js `morphTargetDictionary` (addendum §4).
///
/// `viseme_sil` is intentionally absent — silence means all mapped morphs
/// reset to 0 (same anti-persistence discipline as `MOUTH_ARKIT_SHAPES`).
pub const GROOVY_VISEME_MAP: &[(&str, &str)] = &[
    ("viseme_PP", "Mouth Vis B"),
    ("viseme_FF", "Mouth Vis Ff"),
    ("viseme_TH", "Mouth Vis Th"),
    ("viseme_DD", "Mouth Vis C D G K N S T X Y Z"),
    ("viseme_kk", "Mouth Vis C D G K N S T X Y Z"),
    ("viseme_CH", "Mouth Vis Ch"),
    ("viseme_SS", "Mouth Vis C D G K N S T X Y Z"),
    ("viseme_nn", "Mouth Vis LL"),
    ("viseme_RR", "Mouth Vis LL"),
    ("viseme_aa", "Mouth Vis Ah"),
    ("viseme_E", "Mouth Vis Ee"),
    ("viseme_I", "Mouth Vis Ee"),
    ("viseme_O", "Mouth Vis Oh"),
    ("viseme_U", "Mouth Vis Oo"),
];

/// Oculus-viseme → shape-key map for the Matrix Woman preset.
///
/// Shape keys are NAMED AFTER the Oculus visemes (authored that way in
/// Blender — see 3d_models spec 2026-07-13), so this map is identity for
/// the 10 authored shapes; the remaining 4 visemes share their nearest
/// authored neighbour (same consolidation logic as GROOVY_VISEME_MAP).
pub const MATRIX_VISEME_MAP: &[(&str, &str)] = &[
    ("viseme_PP", "viseme_PP"),
    ("viseme_FF", "viseme_FF"),
    ("viseme_TH", "viseme_TH"),
    ("viseme_DD", "viseme_DD"),
    ("viseme_kk", "viseme_DD"),
    ("viseme_CH", "viseme_CH"),
    ("viseme_SS", "viseme_DD"),
    ("viseme_nn", "viseme_nn"),
    ("viseme_RR", "viseme_nn"),
    ("viseme_aa", "viseme_aa"),
    ("viseme_E", "viseme_E"),
    ("viseme_I", "viseme_E"),
    ("viseme_O", "viseme_O"),
    ("viseme_U", "viseme_U"),
];

/// VoiceModeState name → expression shape key, driven by avatar.js
/// setState for the matrix preset. States absent here relax to neutral.
pub const MATRIX_EXPRESSION_MAP: &[(&str, &str)] =
    &[("listening", "brow_raise"), ("thinking", "brow_furrow")];

/// A single entry in [`PRESET_REGISTRY`].
///
/// GLB URLs are NOT stored here — they are resolved at runtime via
/// `app.rs` helpers (`facecap_glb_url()` / `groovy_glb_url()`) so the
/// registry stays wasm-free and testable on the native target (D-07).
pub struct HeadPreset {
    /// Unique stable identifier. This is the legal value for
    /// `AvatarPrefs::head_id`; Plan 04's security validation iterates this.
    pub id: &'static str,
    /// Human-readable label shown in the Voice Settings dropdown (Plan 04).
    pub display_name: &'static str,
    /// Rough body coverage for future layout adjustments.
    pub body_type: BodyType,
    /// Per-preset camera framing applied by `avatar.js` `init()`.
    pub framing: FramingData,
    /// Per-preset Oculus-viseme → GLB-morph-name map (addendum §4).
    ///
    /// `None` means the preset uses the standard `OCULUS_TO_ARKIT`
    /// decomposition path (i.e. the model has ARKit blendshapes).
    /// `Some(map)` means `avatar.js` drives the listed morph directly from
    /// the dominant Oculus viseme, bypassing `OCULUS_TO_ARKIT`.
    pub viseme_map: Option<&'static [(&'static str, &'static str)]>,
    /// Per-preset blink morph names `(left, right)` (addendum §3).
    ///
    /// `None` means the preset uses the default `eyeBlink_L` / `eyeBlink_R`
    /// ARKit names. `Some((left, right))` provides the model-specific names.
    pub blink_morphs: Option<(&'static str, &'static str)>,
    /// Material treatment avatar.js applies at load (spec 2026-07-13).
    pub material: MaterialKind,
    /// Per-state expression morphs (state name → shape key), or None.
    pub expression_morphs: Option<&'static [(&'static str, &'static str)]>,
}

/// Compile-time, data-driven registry of all supported head presets (D-07).
///
/// This is the single source of truth for legal `AvatarPrefs::head_id` values.
/// Plan 04's head dropdown iterates this for options; its security guard
/// validates a user-supplied `head_id` against it before writing to the signal.
///
/// Adding a new preset requires only a new entry here — no other Rust changes.
pub const PRESET_REGISTRY: &[HeadPreset] = &[
    HeadPreset {
        id: "facecap",
        display_name: "Morph Head",
        body_type: BodyType::Head,
        framing: FramingData {
            // Front-on (x=0): the head faces +Z, so a centred camera shows it
            // looking STRAIGHT AHEAD at rest. The original off-axis (-1.8) view
            // made a forward-facing head look permanently turned right (and made
            // the symmetric head-turn look one-directional). z=3.5 keeps the
            // original ~3.5 framing distance; y=0.8→0.4 is a slight downward tilt.
            cam_pos: (0.0, 0.8, 3.5),
            look_at: (0.0, 0.4, 0.0),
            fov: 45.0,
        },
        // Facecap is ARKit-52; uses the OCULUS_TO_ARKIT decomposition path.
        viseme_map: None,
        // Facecap uses default eyeBlink_L / eyeBlink_R ARKit names.
        blink_morphs: None,
        material: MaterialKind::Normal,
        expression_morphs: None,
    },
    HeadPreset {
        id: "groovy",
        display_name: "Groovy Girl",
        body_type: BodyType::Full,
        framing: FramingData {
            // Full-body; avatar.js Box3 auto-frames at runtime, these are
            // fallback seeds (addendum §3 / Research Priority Area 2).
            cam_pos: (0.0, 1.5, 2.5),
            look_at: (0.0, 1.5, 0.0),
            fov: 45.0,
        },
        // Groovy has no ARKit shapes; drive whole-viseme morphs directly
        // from the dominant Oculus viseme (addendum §4).
        viseme_map: Some(GROOVY_VISEME_MAP),
        // Groovy blink morph names (addendum §3).
        blink_morphs: Some(("Eye L Closed", "Eye R Closed")),
        material: MaterialKind::Pbr,
        expression_morphs: None,
    },
    HeadPreset {
        id: "matrix",
        display_name: "Matrix Woman",
        body_type: BodyType::Half,
        framing: FramingData {
            // Bust model. Seed derived from the packed model's measured
            // bounds (bust height 1.895, eyes at y~1.45, face +Z).
            // UAT-tunable; must stay non-zero
            // (preset_registry_framing_nonzero).
            //
            // Live-UAT fix (2026-07-14, "fisheye / nose very pronounced"):
            // portrait lens — camera pulled back 1.4→3.3 flattens the face
            // perspective to match the concept art (flatness comes from
            // DISTANCE; fov only sets the crop). User then approved the
            // full-bust composition and asked for it to fill the frame:
            // fov 34° centered on the bust's middle (y≈0.95) shows
            // 2·3.3·tan(17°) ≈ 2.0 units — the whole 1.9-unit bust with a
            // hair of margin, filling the circular canvas.
            cam_pos: (0.0, 1.05, 3.3),
            look_at: (0.0, 0.95, 0.0),
            fov: 34.0,
        },
        // Shape keys are named after Oculus visemes; direct-drive path.
        viseme_map: Some(MATRIX_VISEME_MAP),
        blink_morphs: Some(("blink_L", "blink_R")),
        material: MaterialKind::MatrixHologram,
        expression_morphs: Some(MATRIX_EXPRESSION_MAP),
    },
];

/// Throwaway dev-flag → JS-global selection (D-10 / REND-01). Returns the
/// avatar global when the flag is on, else the orb global. These exact
/// literals are what Plan 04's `document::eval` init/pump/destroy/setState
/// strings interpolate (Pitfall 5: the pump must target the selected global).
pub const fn select_avatar_global(avatar_flag: bool) -> &'static str {
    if avatar_flag {
        AVATAR_GLOBAL
    } else {
        ORB_GLOBAL
    }
}

/// Reduced-motion gate (REND-03). Returns `true` iff the trimmed motion
/// token equals `"none"`, mirroring `orb.js`'s
/// `resolveToken(canvas, '--orb-motion', 'full') === 'none'` (the
/// `resolveToken` helper `.trim()`s; an empty/unset token falls back to
/// `"full"`, i.e. motion on → not suppressed).
pub fn reduced_motion_suppressed(motion_token: &str) -> bool {
    motion_token.trim() == "none"
}

// =============================================================================
// Phase 40.5 (D-01/D-03): Orb identity registry
// =============================================================================

/// A single entry in [`ORB_PRESET_REGISTRY`].
///
/// Orbs have no rig, visemes, or blink morphs — only visual knobs (style,
/// colour, size, glow). This struct is intentionally lighter than [`HeadPreset`].
///
/// Each entry is both a **named preset** (selectable in the UI) AND a
/// **bindable identity slug** that can carry its own voice profile in
/// `config.yaml` (D-01/D-03/D-17).
pub struct OrbPreset {
    /// Stable slug used as the identity key (e.g. `"orb_bloom"`).
    /// Legal value for `AvatarPrefs::active_identity`.
    pub id: &'static str,
    /// Human-readable label shown in the identity selector.
    pub display_name: &'static str,
    /// three.js render-mode string passed to `setStyle()` in `orb.js`.
    pub default_style: &'static str,
    /// Default idle base hue (0–360). Listening/speaking/thinking shift relative
    /// to this (D-05 per-state feedback preserved).
    pub default_hue: u16,
    /// Default scale factor (0.5–2.0).
    pub default_size: f32,
    /// Default glow intensity (0.0–1.0).
    pub default_glow: f32,
}

/// Compile-time, data-driven registry of all supported orb presets (D-03).
///
/// This is the single source of truth for legal orb identity slugs.
/// The four entries map to the four three.js render modes:
///
/// - `orb_classic` → current `IcosahedronGeometry` + custom shader
/// - `orb_bloom`   → `EffectComposer` + `UnrealBloomPass` (+ speech breathing, D-06)
/// - `orb_ascii`   → `AsciiEffect` wrapper
/// - `orb_network` → animated `BufferGeometry` drawRange lines/points
///
/// Adding a new orb preset = adding a new entry here. No other Rust change needed.
pub const ORB_PRESET_REGISTRY: &[OrbPreset] = &[
    OrbPreset {
        id: "orb_classic",
        display_name: "Classic",
        default_style: "classic",
        default_hue: 186,
        default_size: 1.0,
        default_glow: 0.5,
    },
    OrbPreset {
        id: "orb_bloom",
        display_name: "Bloom",
        default_style: "bloom",
        default_hue: 280,
        default_size: 1.0,
        default_glow: 0.8,
    },
    OrbPreset {
        id: "orb_ascii",
        display_name: "ASCII",
        default_style: "ascii",
        default_hue: 120,
        default_size: 1.0,
        default_glow: 0.3,
    },
    OrbPreset {
        id: "orb_network",
        display_name: "Network",
        default_style: "network",
        default_hue: 200,
        default_size: 1.2,
        default_glow: 0.6,
    },
];

/// Validate an identity slug against the combined orb + head registry.
///
/// Returns `true` when `slug` matches any [`ORB_PRESET_REGISTRY`] id OR any
/// [`PRESET_REGISTRY`] id. Used as the single reusable security gate for:
/// - Hydrating `AvatarPrefs.active_identity` from localStorage (T-40.5-01-03)
/// - Server-side `identity_slug` validation in `api.rs` (Plan 03)
/// - UI slug writes in Plans 06/07
///
/// Any slug that is not in either registry (including path-traversal strings,
/// empty strings, and arbitrary user input) returns `false`.
pub fn is_known_identity(slug: &str) -> bool {
    ORB_PRESET_REGISTRY.iter().any(|p| p.id == slug) || PRESET_REGISTRY.iter().any(|p| p.id == slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the ARKit name→index map covering every mouth shape plus a
    /// couple of non-mouth shapes, so tests can assert mouth writes land and
    /// non-mouth shapes stay untouched. Indices are assigned deterministically.
    fn full_arkit_index() -> (BTreeMap<&'static str, usize>, usize) {
        let mut idx = BTreeMap::new();
        let mut next = 0usize;
        for shape in MOUTH_ARKIT_SHAPES {
            idx.insert(*shape, next);
            next += 1;
        }
        // Non-mouth shapes the viseme map must never touch.
        for shape in ["eyeBlinkLeft", "eyeBlinkRight", "browInnerUp"] {
            idx.insert(shape, next);
            next += 1;
        }
        (idx, next)
    }

    fn scores_of(pairs: &[(&'static str, f32)]) -> BTreeMap<&'static str, f32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn viseme_aa_sets_jaw_open_and_leaves_non_mouth_untouched() {
        let (arkit, n) = full_arkit_index();
        let mut influences = vec![0.0f32; n];
        let blink = arkit["eyeBlinkLeft"];
        influences[blink] = 0.42; // pre-existing non-mouth value must survive

        apply_visemes(&scores_of(&[("viseme_aa", 1.0)]), &arkit, &mut influences);

        assert_eq!(
            influences[arkit["jawOpen"]], 0.6,
            "jawOpen = table weight 0.6"
        );
        assert_eq!(
            influences[blink], 0.42,
            "non-mouth shape (eyeBlinkLeft) must be untouched by the viseme map",
        );
    }

    #[test]
    fn viseme_o_sets_pucker_jaw_forward_and_jaw_open() {
        let (arkit, n) = full_arkit_index();
        let mut influences = vec![0.0f32; n];

        apply_visemes(&scores_of(&[("viseme_O", 1.0)]), &arkit, &mut influences);

        assert_eq!(influences[arkit["mouthPucker"]], 1.0);
        assert_eq!(influences[arkit["jawForward"]], 0.6);
        assert_eq!(influences[arkit["jawOpen"]], 0.2);
    }

    #[test]
    fn overlapping_contributions_clamp_to_one() {
        let (arkit, n) = full_arkit_index();
        let mut influences = vec![0.0f32; n];

        // viseme_E (jawOpen 0.3) + viseme_aa (jawOpen 0.6) at full score = 0.9;
        // push higher with viseme_O (jawOpen 0.2) → 1.1 raw, must clamp to 1.0.
        apply_visemes(
            &scores_of(&[("viseme_aa", 1.0), ("viseme_E", 1.0), ("viseme_O", 1.0)]),
            &arkit,
            &mut influences,
        );

        assert_eq!(
            influences[arkit["jawOpen"]], 1.0,
            "summed jawOpen contributions must clamp to exactly 1.0 (Pitfall 6)",
        );
    }

    #[test]
    fn mouth_shapes_zeroed_each_call_silence_clears_prior_frame() {
        let (arkit, n) = full_arkit_index();
        let mut influences = vec![0.0f32; n];

        // Frame 1: speaking — jawOpen lifts.
        apply_visemes(&scores_of(&[("viseme_aa", 1.0)]), &arkit, &mut influences);
        assert_eq!(influences[arkit["jawOpen"]], 0.6);

        // Frame 2: silence (viseme_sil maps to nothing). Mouth must reset to 0.
        apply_visemes(&scores_of(&[("viseme_sil", 1.0)]), &arkit, &mut influences);
        assert_eq!(
            influences[arkit["jawOpen"]], 0.0,
            "prior frame's jawOpen must not persist into silence",
        );
    }

    #[test]
    fn missing_arkit_shape_is_skipped_without_panic() {
        // arkit_index deliberately OMITS tongueOut; viseme_TH maps to it.
        let mut arkit: BTreeMap<&str, usize> = BTreeMap::new();
        arkit.insert("mouthRollUpper", 0);
        arkit.insert("jawOpen", 1);
        // tongueOut intentionally absent (A2 graceful degrade).
        let mut influences = vec![0.0f32; 2];

        apply_visemes(&scores_of(&[("viseme_TH", 1.0)]), &arkit, &mut influences);

        // Present shapes still applied; missing tongueOut simply skipped.
        assert_eq!(influences[arkit["mouthRollUpper"]], 0.6);
        assert_eq!(influences[arkit["jawOpen"]], 0.2);
    }

    #[test]
    fn viseme_sil_produces_all_zero_mouth() {
        let (arkit, n) = full_arkit_index();
        let mut influences = vec![0.0f32; n];

        apply_visemes(&scores_of(&[("viseme_sil", 1.0)]), &arkit, &mut influences);

        for shape in MOUTH_ARKIT_SHAPES {
            assert_eq!(
                influences[arkit[shape]], 0.0,
                "{shape} must be zero on silence",
            );
        }
    }

    #[test]
    fn oculus_to_arkit_table_has_fifteen_visemes_including_sil() {
        // 14 entries in OCULUS_TO_ARKIT + viseme_sil (no shapes) = 15 Oculus
        // visemes total (the verified RESEARCH table).
        assert_eq!(
            OCULUS_TO_ARKIT.len(),
            14,
            "viseme_sil maps to no shapes so it is absent; the other 14 are pinned",
        );
        for (oculus, _) in OCULUS_TO_ARKIT {
            assert!(
                oculus.starts_with("viseme_"),
                "{oculus} must be an Oculus viseme name"
            );
        }
    }

    #[test]
    fn select_avatar_global_returns_correct_global_for_each_flag_state() {
        assert_eq!(select_avatar_global(true), "ironHermesAvatar");
        assert_eq!(select_avatar_global(false), "ironHermesOrb");
    }

    #[test]
    fn reduced_motion_suppressed_true_for_none() {
        assert!(reduced_motion_suppressed("none"));
    }

    #[test]
    fn reduced_motion_suppressed_false_for_full_and_empty() {
        assert!(!reduced_motion_suppressed("full"));
        assert!(
            !reduced_motion_suppressed(""),
            "empty token falls back to motion-on (orb.js '--orb-motion' fallback 'full')",
        );
    }

    #[test]
    fn reduced_motion_suppressed_trims_token() {
        assert!(
            reduced_motion_suppressed("  none  "),
            "token is trimmed before comparison, mirroring resolveToken's .trim()",
        );
    }

    // --- Phase 40.2 Plan 01 Task 2: PRESET_REGISTRY tests (RED) ---

    #[test]
    fn preset_registry_has_three_entries() {
        // ID-01, D-07: registry is data-driven; three entries as of the
        // Matrix Woman preset (spec 2026-07-13).
        assert_eq!(PRESET_REGISTRY.len(), 3);
        assert_eq!(PRESET_REGISTRY[0].id, "facecap");
        assert_eq!(PRESET_REGISTRY[1].id, "groovy");
        assert_eq!(PRESET_REGISTRY[2].id, "matrix");
    }

    #[test]
    fn matrix_preset_shape() {
        let m = PRESET_REGISTRY.iter().find(|p| p.id == "matrix").unwrap();
        assert_eq!(m.display_name, "Matrix Woman");
        assert!(matches!(m.body_type, BodyType::Half));
        assert!(matches!(m.material, MaterialKind::MatrixHologram));
        assert_eq!(m.blink_morphs, Some(("blink_L", "blink_R")));
        // The viseme map covers all 14 sounding Oculus visemes and only
        // references shape keys we author (identity-or-shared names).
        let map = m.viseme_map.expect("matrix must use the direct-drive path");
        assert_eq!(map.len(), 14);
        let authored = [
            "viseme_PP",
            "viseme_FF",
            "viseme_TH",
            "viseme_DD",
            "viseme_CH",
            "viseme_nn",
            "viseme_aa",
            "viseme_E",
            "viseme_O",
            "viseme_U",
            "blink_L",
            "blink_R",
            "brow_raise",
            "brow_furrow",
            "fx_dissolve",
        ];
        for (oculus, morph) in map {
            assert!(oculus.starts_with("viseme_"));
            assert!(
                authored.contains(morph),
                "viseme map references unauthored morph '{morph}'"
            );
        }
        // Expression morphs target real setState state names.
        let expr = m.expression_morphs.expect("matrix has expression morphs");
        for (state, _) in expr {
            assert!(["idle", "listening", "thinking", "speaking"].contains(state));
        }
        for (_, morph) in expr {
            assert!(
                authored.contains(morph),
                "expression map references unauthored morph '{morph}'"
            );
        }
        let (bl, br) = m.blink_morphs.expect("matrix has blink morphs");
        assert!(authored.contains(&bl) && authored.contains(&br));
    }

    #[test]
    fn preset_registry_framing_nonzero() {
        // D-09: every preset must have a non-zero cam_pos for meaningful framing.
        for p in PRESET_REGISTRY {
            let (x, y, z) = p.framing.cam_pos;
            assert!(
                x != 0.0 || y != 0.0 || z != 0.0,
                "cam_pos must be nonzero for preset '{}'",
                p.id
            );
        }
    }

    #[test]
    fn preset_registry_ids_match_avatar_prefs_default() {
        // Cross-check: the default head_id from AvatarPrefs must resolve to a
        // real registry entry so an unknown default can never ship.
        let default_id = crate::ui_prefs::AvatarPrefs::default().head_id;
        assert!(
            PRESET_REGISTRY.iter().any(|p| p.id == default_id),
            "AvatarPrefs::default().head_id='{}' not found in PRESET_REGISTRY",
            default_id
        );
    }
}

// =============================================================================
// Phase 40.5 orb-preset registry tests (D-01/D-03)
// =============================================================================
#[cfg(test)]
mod orb_preset_tests {
    use super::*;

    /// D-03: ORB_PRESET_REGISTRY must have exactly four render-mode entries.
    #[test]
    fn orb_preset_registry_has_four_entries() {
        assert_eq!(
            ORB_PRESET_REGISTRY.len(),
            4,
            "ORB_PRESET_REGISTRY must have exactly 4 entries (D-03)"
        );
    }

    /// D-03: The four orb preset ids must be the canonical render-mode slugs, all unique.
    #[test]
    fn orb_preset_ids_unique_and_prefixed() {
        let expected = ["orb_classic", "orb_bloom", "orb_ascii", "orb_network"];
        for slug in expected {
            assert!(
                ORB_PRESET_REGISTRY.iter().any(|p| p.id == slug),
                "ORB_PRESET_REGISTRY missing expected id '{slug}'"
            );
        }
        // Uniqueness: all ids distinct
        let ids: Vec<&str> = ORB_PRESET_REGISTRY.iter().map(|p| p.id).collect();
        let deduped: std::collections::HashSet<&&str> = ids.iter().collect();
        assert_eq!(
            ids.len(),
            deduped.len(),
            "ORB_PRESET_REGISTRY ids must all be unique"
        );
    }

    /// 41.2 gap-fix regression guard. `OrbStyleCtx` holds the render-mode NAME
    /// ("classic"/"bloom"/"ascii"/"network" — the tile `style_key` and each
    /// preset's `default_style`), and the orb_canvas eval bridge validates that
    /// value against the registry before dispatching `setStyle`. Those names
    /// match `default_style`, NOT the prefixed `id` ("orb_*"). Commit fb684d71a
    /// validated against `p.id`, which never matched a style name, so setStyle
    /// was silently never dispatched and the orb stuck on classic. This fails if
    /// a future edit re-points the dispatch guard at `id` or lets default_style
    /// drift out of sync with orb.js's validStyles.
    #[test]
    fn orb_render_mode_names_match_default_style_not_id() {
        for name in ["classic", "bloom", "ascii", "network"] {
            assert!(
                ORB_PRESET_REGISTRY.iter().any(|p| p.default_style == name),
                "render-mode name '{name}' must match a preset default_style — the orb_canvas dispatch guard validates against default_style"
            );
            assert!(
                !ORB_PRESET_REGISTRY.iter().any(|p| p.id == name),
                "render-mode name '{name}' must NOT equal any preset id (ids are 'orb_*'); validating the dispatch guard against `id` silently blocks setStyle (41.2 gap-fix)"
            );
        }
    }

    /// D-01: is_known_identity must accept orb slugs and head slugs, reject unknowns.
    #[test]
    fn is_known_identity_accepts_orb_and_head() {
        // Known orb preset
        assert!(
            is_known_identity("orb_bloom"),
            "orb_bloom must be a known identity"
        );
        // Known head preset
        assert!(
            is_known_identity("facecap"),
            "facecap must be a known identity"
        );
        // Path-traversal-like slug — must be rejected
        assert!(
            !is_known_identity("../etc"),
            "../etc must NOT be a known identity"
        );
        // Empty — must be rejected
        assert!(
            !is_known_identity(""),
            "empty slug must NOT be a known identity"
        );
        // Unknown slug — must be rejected
        assert!(
            !is_known_identity("unknown_identity"),
            "unknown slug must NOT be a known identity"
        );
    }
}
