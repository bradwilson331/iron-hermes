// Phase 01 (Three.js Viseme Avatar Core) Plan 03 — Oculus-viseme → ARKit-blendshape map.
//
// VERBATIM JS port of the verified Rust seam
//   crates/iron_hermes_ui/src/components/hermes_app/avatar_logic.rs
// (`apply_visemes`, `OCULUS_TO_ARKIT`, `MOUTH_ARKIT_SHAPES`). Parity with
// avatar_logic.rs is the correctness contract — that module is unit-tested
// (`cargo test -p iron_hermes_ui`); this file has no separate JS test runner.
//
// The OCULUS_TO_ARKIT weights are copied byte-for-byte from the reviewed
// 01-RESEARCH.md "VERIFIED TABLE" (met4citizen/TalkingHead
// build-visemes-from-arkit.py) — NOT hand-tuned (VIS-03 / D-06).
//
// Table indirection is kept so a Phase-2 Ready Player Me head swap replaces
// only OCULUS_TO_ARKIT (Oculus→Oculus near-identity), not the apply logic
// (D-02, RPM-ready) — mirroring orb.js's const-object-table style
// (STATE_COLOR_FALLBACKS).
//
// Security: no imports (self-hosted, REND-01); numeric inputs only, no eval.

// Mouth-region ARKit blendshapes that applyVisemes zeroes each call before
// re-applying the current frame's viseme contributions, so a prior frame's
// mouth shape does not persist into silence (Pitfall 6 / D-06). Blink/breath/
// idle shapes are deliberately NOT in this set so idle motion is never
// clobbered by a mouth write.
export const MOUTH_ARKIT_SHAPES = [
  'jawOpen',
  'jawForward',
  'mouthPucker',
  'mouthFunnel',
  'mouthShrugUpper',
  'mouthRollUpper',
  'mouthRollLower',
  'mouthPressLeft',
  'mouthPressRight',
  'mouthDimpleLeft',
  'mouthDimpleRight',
  'mouthLowerDownLeft',
  'mouthLowerDownRight',
  'mouthUpperUpLeft',
  'mouthUpperUpRight',
  'tongueOut',
];

// The verified Oculus-viseme → ARKit-blendshape composition table
// (VIS-03 / D-06). wawa-lipsync emits the 15 Oculus visemes; facecap.glb is
// ARKit-52, so this map sits between the driver and morphTargetInfluences.
// Weights are additive influence contributions, clamped to [0, 1] per-shape by
// applyVisemes. viseme_sil (silence) maps to no shapes and is intentionally
// absent — silence produces an all-zero mouth because applyVisemes zeroes
// MOUTH_ARKIT_SHAPES first and viseme_sil adds nothing.
export const OCULUS_TO_ARKIT = {
  viseme_aa: { jawOpen: 0.6 },
  viseme_E: {
    mouthPressLeft: 0.8,
    mouthPressRight: 0.8,
    mouthDimpleLeft: 1.0,
    mouthDimpleRight: 1.0,
    jawOpen: 0.3,
  },
  viseme_I: {
    mouthPressLeft: 0.6,
    mouthPressRight: 0.6,
    mouthDimpleLeft: 0.6,
    mouthDimpleRight: 0.6,
    jawOpen: 0.2,
  },
  viseme_O: { mouthPucker: 1.0, jawForward: 0.6, jawOpen: 0.2 },
  viseme_U: { mouthFunnel: 1.0 },
  viseme_PP: {
    mouthRollLower: 0.8,
    mouthRollUpper: 0.8,
    mouthUpperUpLeft: 0.3,
    mouthUpperUpRight: 0.3,
  },
  viseme_FF: {
    mouthPucker: 1.0,
    mouthShrugUpper: 1.0,
    mouthLowerDownLeft: 0.2,
    mouthLowerDownRight: 0.2,
    mouthDimpleLeft: 1.0,
    mouthDimpleRight: 1.0,
    mouthRollLower: 1.0,
  },
  viseme_DD: {
    mouthPressLeft: 0.8,
    mouthPressRight: 0.8,
    mouthFunnel: 0.5,
    jawOpen: 0.2,
  },
  viseme_SS: {
    mouthPressLeft: 0.8,
    mouthPressRight: 0.8,
    mouthLowerDownLeft: 0.5,
    mouthLowerDownRight: 0.5,
    jawOpen: 0.1,
  },
  viseme_TH: {
    mouthRollUpper: 0.6,
    jawOpen: 0.2,
    tongueOut: 0.4,
  },
  viseme_CH: { mouthPucker: 0.5, jawOpen: 0.2 },
  viseme_RR: { mouthPucker: 0.5, jawOpen: 0.2 },
  viseme_kk: {
    mouthLowerDownLeft: 0.4,
    mouthLowerDownRight: 0.4,
    mouthDimpleLeft: 0.3,
    mouthDimpleRight: 0.3,
    mouthFunnel: 0.3,
    mouthPucker: 0.3,
    jawOpen: 0.15,
  },
  viseme_nn: {
    mouthLowerDownLeft: 0.4,
    mouthLowerDownRight: 0.4,
    mouthDimpleLeft: 0.3,
    mouthDimpleRight: 0.3,
    mouthFunnel: 0.3,
    mouthPucker: 0.3,
    jawOpen: 0.15,
    tongueOut: 0.2,
  },
};

// Apply one frame of Oculus-viseme scores to the ARKit `influences` array,
// in place. Pure; no Three.js (VIS-03 / D-06). Ports apply_visemes verbatim:
//
// 1. Zero every MOUTH_ARKIT_SHAPES entry whose name exists in `arkitIndex`
//    (so a prior frame's mouth shape does not persist).
// 2. For each (oculus, mix) in OCULUS_TO_ARKIT, read the score from `scores`;
//    skip if absent or zero (falsy).
// 3. For each (arkit, weight) in the mix, accumulate
//    influences[i] = Math.min(1, influences[i] + score * weight),
//    guarded by `if (i !== undefined)` so an ARKit shape missing from the
//    loaded GLB degrades gracefully (Assumption A2) instead of throwing.
//
// `arkitIndex` maps an ARKit blendshape name (e.g. "jawOpen") to its index into
// `influences` (built from the GLB's morphTargetDictionary, stripping the
// "blendShape1." prefix — see Pattern 1).
export function applyVisemes(scores, arkitIndex, influences) {
  // Zero the mouth region each call (guarded for missing index — A2).
  for (const shape of MOUTH_ARKIT_SHAPES) {
    const i = arkitIndex[shape];
    if (i !== undefined) {
      influences[i] = 0;
    }
  }

  // Accumulate this frame's viseme contributions, clamped to [0, 1].
  for (const oculus in OCULUS_TO_ARKIT) {
    const s = scores[oculus];
    if (!s) continue; // skip absent / zero score
    const mix = OCULUS_TO_ARKIT[oculus];
    for (const arkit in mix) {
      const i = arkitIndex[arkit];
      if (i !== undefined) {
        influences[i] = Math.min(1, influences[i] + s * mix[arkit]);
      }
    }
  }
}
