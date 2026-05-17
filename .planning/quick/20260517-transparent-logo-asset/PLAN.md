---
quick_id: 260517-qvu
slug: transparent-logo-asset
date: 2026-05-17
type: quick
status: complete
---

# Quick Task: Make i_hermes_logo.png have a transparent background

## Description

The IronHermes wings logo at `crates/iron_hermes_ui/assets/i_hermes_logo.png` had a transparency-indicator checkerboard pattern (alternating white and ~220 gray squares) baked in as opaque pixels — likely exported with the IDE's transparency-indicator showing. Every viewer was compositing the image over its own background, producing visible "double checkerboard" artifacts.

## Goal

Replace the baked-in checkerboard with true PNG alpha transparency so the logo composites cleanly on any background without halo or gray-square bleed-through.

## Files Modified

- `crates/iron_hermes_ui/assets/i_hermes_logo.png` — checkerboard removed, alpha=0 where background was, copper wings opaque

## Files NOT Committed

- `crates/iron_hermes_ui/assets/i_hermes_logo.png.bak` — local safety backup of the original; intentionally left untracked
- `crates/iron_hermes_ui/assets/i_hermes_logo_old.png` — pre-existing untracked file unrelated to this task

## Approach

1. First attempt: rembg (ML background removal) — failed because `onnxruntime` is not installed in the local Python env.
2. Second attempt: Python/PIL saturation-based mask (`max(R,G,B) - min(R,G,B) < 30 && lum > 190`) — produced visible halo because partial-alpha edge pixels retained their gray RGB and showed as milky on dark composites.
3. **Final approach (used):** ImageMagick 7 flood-fill from `(0,0)` with 18% fuzz tolerance. The 18% covers both checker tones (white→gray is 35/255 ≈ 14% distance) but stops at chromatic copper edges. Connectivity-based; no leak into the logo because wings have sharp boundaries.

   ```bash
   magick i_hermes_logo.png -alpha set \
     -fill none -fuzz 18% \
     -draw "alpha 0,0 floodfill" \
     i_hermes_logo.png
   ```

## Verification

- `magick identify` confirms all four corners and center are `srgba(0,0,0,0)` (fully transparent)
- Wing-tip pixel `srgba(27.3%, 8.4%, 2.9%, 1)` — opaque copper
- Composited on `#1a1a2e` (dark) and `#ffffff` (light) — no halo, no checker bleed-through
- File size: 5.6 MB → 3.8 MB (32% smaller; PNG compresses transparent regions efficiently)
- Dimensions unchanged: 2690×1568

## Why no agent spawn

Default /gsd-quick path is planner + executor. The asset transform was already complete at the time the quick task was scaffolded (the user requested asset edit directly, then asked for the GSD wrapper after). No executor work remained beyond `git commit`, so this PLAN.md doubles as the SUMMARY.md and the commit was issued inline.
