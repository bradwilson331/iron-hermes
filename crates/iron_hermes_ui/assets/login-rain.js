// Phase 47.3 Plan 05 (D-05, D-16a, UI-SPEC Accessibility "1d" row): the `1d`
// matrix-rain canvas — the only client script this pre-auth login surface
// ships. Genuinely from-scratch: no analog anywhere in this crate (see
// 47.3-PATTERNS.md "No Analog Found: login-rain.js" — every other JS asset
// here is either a vendored third-party library or wired to WebGL/audio).
//
// Load-bearing invariant (T-47.3-21): this script paints a canvas and
// nothing else. It never opens any kind of outbound network channel of any
// sort, ever. `rain_js_uses_only_d05_greens` and a grep acceptance
// criterion both pin this (see the plan's own verify step for the exact
// forbidden-construct list this file must never contain).
//
// D-05's exact color substitution (locked, not discretionary — see
// 47.3-UI-SPEC.md "D-05: Matrix rain canvas — exact color substitution"):
//   leading glyph (current frame)     -> #56d364 (--ansi-bright-green)
//   mid glyph (one row back)          -> #3fb950 (--ansi-green)
//   tail glyph (two rows back)        -> rgba(63, 185, 80, 0.45)
//                                         (--ansi-green at reduced alpha —
//                                         no third green hue is invented)
// These are raw string literals fed straight to the 2D context's
// `fillStyle`. Canvas 2D cannot consume a CSS custom property
// (`var(--ansi-green)`) at all, so no `var()` is attempted anywhere below —
// every color here is a literal, by necessity, not an oversight.
//
// prefers-reduced-motion (UI-SPEC Accessibility, "1d" row): CSS alone cannot
// stop a canvas `setInterval`, so this script checks
// `matchMedia('(prefers-reduced-motion: reduce)')` once at load and, if it
// matches, paints exactly one static frame (the flat base fill) instead of
// ever starting the animation loop.
//
// Failure-state re-key (47.3-05-PLAN.md "Open item for this task" / the
// backstop must_haves entry): the export's copy says "rain re-keyed to
// alarm stream" but encodes no red rain palette of its own. Resolved here
// per the plan's own recommendation — the glyph ramp swaps from the D-05
// green triad to a danger-hue triad for as long as `document.body` carries
// the `is-rain-failed` class (toggled by the shared submit script's
// `showDenied()` in `login_page.rs`), keeping the same 4-step
// leading/mid/tail structure. Recorded as a resolved backstop in
// 47.3-05-SUMMARY.md, not silently assumed.
(function () {
  "use strict";

  var CANVAS_ID = "login-rain-canvas";
  var CELL = 15;
  var TICK_MS = 60;

  // Canvas-internal rendering parameters — literal per D-04 (canvas 2D
  // fillStyle cannot consume a token at all), unchanged from the export.
  var BASE_FILL = "#050505";
  var TRAIL_FADE = "rgba(5,5,5,0.10)";

  // D-05's locked green substitution.
  var LEAD_COLOR = "#56d364";
  var MID_COLOR = "#3fb950";
  var TAIL_COLOR = "rgba(63, 185, 80, 0.45)";

  // Failure-state re-key (resolved backstop, see header doc comment) — a
  // danger hue triad, structurally identical to the green ramp above. Not a
  // "third green" (D-05's own constraint): these are red, not green.
  var FAIL_LEAD_COLOR = "#f2a9a9";
  var FAIL_MID_COLOR = "#e35d5d";
  var FAIL_TAIL_COLOR = "rgba(211, 47, 47, 0.45)";

  var GLYPHS = "01ｦｧｨｩｪｫｬｭｮｯｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﬀ7A3F9DE#/\\|<>[]{}·─█▓▒░".split("");

  function prefersReducedMotion() {
    return (
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    );
  }

  function isFailedState() {
    return !!(document.body && document.body.classList.contains("is-rain-failed"));
  }

  function paintStaticFrame(ctx, width, height) {
    ctx.fillStyle = BASE_FILL;
    ctx.fillRect(0, 0, width, height);
  }

  function startRain(canvas) {
    var ctx = canvas.getContext && canvas.getContext("2d");
    if (!ctx) {
      // No 2D context available (unsupported browser, canvas stripped by a
      // content blocker, etc.) — degrade to the flat background fill
      // beneath (T-47.3-23): do nothing further, never throw.
      return;
    }

    var dpr = Math.min(2, window.devicePixelRatio || 1);
    var dims = { width: 0, height: 0 };

    function measure() {
      var parent = canvas.parentElement;
      var width = (parent && parent.clientWidth) || window.innerWidth || 1180;
      var height = (parent && parent.clientHeight) || window.innerHeight || 720;
      canvas.width = Math.max(1, Math.round(width * dpr));
      canvas.height = Math.max(1, Math.round(height * dpr));
      canvas.style.width = width + "px";
      canvas.style.height = height + "px";
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      dims.width = width;
      dims.height = height;
    }

    measure();
    paintStaticFrame(ctx, dims.width, dims.height);

    if (prefersReducedMotion()) {
      // CSS cannot stop a canvas setInterval — this early return IS the
      // reduced-motion mechanism for `1d`. One static frame, no loop.
      return;
    }

    var cols = 0;
    var drops = [];
    var speed = [];

    // `fillViewport` distinguishes the two callers, and the distinction is
    // load-bearing (G-47.3-2). The INITIAL seed deliberately staggers every column
    // above the top edge so the rain falls into an empty screen on first paint. A
    // RESIZE reseed must not: parking the columns up to 900px overhead drains the
    // viewport for 2-6s, and dragging a window edge emits a continuous stream of
    // resize events, so the rain never gets to fall back in for as long as the drag
    // lasts. Seeding across the existing rows keeps the field populated instead.
    function reseedColumns(fillViewport) {
      cols = Math.max(1, Math.floor(dims.width / CELL));
      drops = [];
      speed = [];
      var rows = Math.max(1, Math.ceil(dims.height / CELL));
      for (var i = 0; i < cols; i++) {
        drops.push(fillViewport ? Math.random() * rows : Math.random() * -60);
        speed.push(0.6 + Math.random() * 1.2);
      }
    }

    reseedColumns();

    var interval = setInterval(function () {
      var failed = isFailedState();
      var lead = failed ? FAIL_LEAD_COLOR : LEAD_COLOR;
      var mid = failed ? FAIL_MID_COLOR : MID_COLOR;
      var tail = failed ? FAIL_TAIL_COLOR : TAIL_COLOR;

      ctx.fillStyle = TRAIL_FADE;
      ctx.fillRect(0, 0, dims.width, dims.height);
      ctx.font = CELL + "px 'Iosekey Mono', monospace";

      for (var c = 0; c < cols; c++) {
        var y = drops[c] * CELL;
        var g0 = GLYPHS[(Math.random() * GLYPHS.length) | 0];
        var g1 = GLYPHS[(Math.random() * GLYPHS.length) | 0];
        var g2 = GLYPHS[(Math.random() * GLYPHS.length) | 0];

        ctx.fillStyle = lead;
        ctx.fillText(g0, c * CELL, y);
        ctx.fillStyle = mid;
        ctx.fillText(g1, c * CELL, y - CELL);
        ctx.fillStyle = tail;
        ctx.fillText(g2, c * CELL, y - CELL * 2);

        drops[c] += speed[c];
        if (y > dims.height + 200 * Math.random()) {
          drops[c] = Math.random() * -20;
        }
      }
    }, TICK_MS);

    var resizeTimer = null;
    window.addEventListener("resize", function () {
      // IMMEDIATE repair, deliberately outside the debounce (G-47.3-2, second pass).
      // `measure()` pins the canvas to inline PIXEL dimensions, so any viewport change
      // that makes the parent larger in either axis — a phone rotating portrait to
      // landscape above all — leaves the canvas physically smaller than the shell, and
      // the decorative `.login-chat-mock` shows through the uncovered band. Repairing
      // that only in the debounced callback is not enough: rotating with the password
      // field focused streams resize events (keyboard, iOS auto-zoom, orientation) that
      // keep resetting the 150ms timer, so the gap persists for as long as they arrive.
      // Reported from a handset as a "ghost mock" after rotating while focused.
      //
      // Gated on actually being undersized so the common case — dragging a window edge
      // smaller — does NOT wipe the trails on every event and leave the rain looking
      // thin. Only growth, which is the ghost condition, pays for an immediate repaint.
      var parent = canvas.parentElement;
      var needW = (parent && parent.clientWidth) || window.innerWidth || 0;
      var needH = (parent && parent.clientHeight) || window.innerHeight || 0;
      if (dims.width < needW || dims.height < needH) {
        measure();
        paintStaticFrame(ctx, dims.width, dims.height);
      }

      if (resizeTimer) {
        clearTimeout(resizeTimer);
      }
      resizeTimer = setTimeout(function () {
        measure();
        // G-47.3-2, the load-bearing line. `measure()` assigns canvas.width and
        // canvas.height, and per spec ANY assignment to those resets the bitmap to
        // fully TRANSPARENT — including assigning the same value back. That destroys
        // the opaque BASE_FILL backdrop, and the only background paint in the draw
        // loop is TRAIL_FADE at 10% alpha, which never restores opacity. Without
        // this repaint the decorative `.login-chat-mock` layer underneath
        // (login_page.rs `render_matrix_rain`) becomes legible straight through the
        // canvas — reproduced deterministically by firing resize every 300ms, and
        // reported from the field as the login page "going to the fake page".
        // Do not remove this on the reasoning that startRain() already painted one.
        paintStaticFrame(ctx, dims.width, dims.height);
        reseedColumns(true);
      }, 150);
    });

    window.addEventListener("beforeunload", function () {
      clearInterval(interval);
    });
  }

  function init() {
    var canvas = document.getElementById(CANVAS_ID);
    if (!canvas || typeof canvas.getContext !== "function") {
      // Missing canvas element (T-47.3-23) — the flat --bg-canvas fill
      // beneath is the implicit fallback; never throw, never block login.
      return;
    }
    startRain(canvas);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
