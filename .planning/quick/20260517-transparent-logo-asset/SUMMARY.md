---
quick_id: 260517-qvu
slug: transparent-logo-asset
date: 2026-05-17
status: complete
---

# Summary: transparent-logo-asset

**Outcome:** `crates/iron_hermes_ui/assets/i_hermes_logo.png` now has true PNG alpha transparency. Baked-in checkerboard pattern removed via ImageMagick flood-fill with 18% fuzz from `(0,0)`.

**Files committed:** `crates/iron_hermes_ui/assets/i_hermes_logo.png` (1 file).

**Result:** Corners + interior empty space = `srgba(0,0,0,0)`. Wings opaque, all copper highlights preserved. Clean composite on both dark and light backgrounds. File size 5.6 MB → 3.8 MB.

**Backup retained locally:** `crates/iron_hermes_ui/assets/i_hermes_logo.png.bak` (not committed). Restore with `cp` if revert needed.
