// Phase 36.17.9 Plan 03 — IronHermes Audio-Reactive Orb
// Extended in Phase 40.5 Plan 05 — render modes + customization API (D-02/D-03/D-05/D-06/D-07)
// Extended in Phase 40.5 Plan 05: bloom + ascii + network modes, setStyle/setBaseHue/setSize/setGlow
//
// API: window.ironHermesOrb.init(canvasId)
//               .updateFFT(bins)      -- Float32Array or plain Array of 64 values
//               .setState(state)      -- 'idle'|'listening'|'thinking'|'speaking'
//               .setStyle(style)      -- 'classic'|'bloom'|'ascii'|'network'  (D-03)
//               .setBaseHue(hue)      -- number 0-360 (D-05)
//               .setSize(scale)       -- number 0.5-2.0 (D-02)
//               .setGlow(intensity)   -- number 0.0-1.0 (D-02)
//               .destroy()
//
// Security: no external network calls. All imports from ./three.module.js (self-hosted).
//           Effect addon imports are relative ./X.js vendored files (D-07, REND-01).
// FFT bins are numeric; no eval of user data (T-36.17.9-03-02 accepted).

import * as THREE from './three.module.js';
import { EffectComposer } from './EffectComposer.js';
import { RenderPass } from './RenderPass.js';
import { UnrealBloomPass } from './UnrealBloomPass.js';
import { OutputPass } from './OutputPass.js';
import { AsciiEffect } from './AsciiEffect.js';

// ── Module-level state ────────────────────────────────────────────────────────

let renderer = null;
let scene = null;
let camera = null;
let orbMesh = null;
let innerGlow = null;
let auraLight = null;
let clock = null;
let reducedMotion = false;
let currentState = 'idle';
let canvas = null; // stored at init() for mode-switching (D-03)

// Render style (D-03): 'classic' | 'bloom' | 'ascii' | 'network'
let currentStyle = 'classic';

// Pipeline refs — populated by initBloom / initAscii / initNetwork on mode switch
let composer = null;
let bloomPass = null;
let asciiEffect = null;
let networkGeometry = null;
let networkMaterial = null;
let networkLines = null;

// FFT bin data normalized [0,1] length 64
const fftBins = new Float32Array(64);

// Color lerp state
const currentColor = new THREE.Color();
const targetColor  = new THREE.Color();
let colorLerp = 1.0;

// Whole-orb pulse state (Phase 41.2 talk-sync gap-fix).
// The old build relied on per-vertex shader displacement alone, which reads as
// surface "bumpiness" that a viewer cannot distinguish from the always-on idle
// breath. A whole-orb SCALE pulse (the sphere visibly swells with speech
// amplitude) is the unmistakable "the orb is talking" signal. `baseScale` is
// the user's setSize() target; animate() multiplies it by the live pulse each
// frame. `smoothedEnergy` low-passes the low-band FFT so the swell is smooth,
// not jittery, and also drives bloom strength.
let baseScale = 1.0;
let smoothedEnergy = 0.0;

// ASCII renders the sphere's TRUE geometric edge (radius 1.0 → ~86% of the box)
// as glyphs, whereas bloom's visible "core" is only the bright fresnel centre
// (~57% of the box) with a glow halo around it. Same scene, but ASCII reads
// ~1.5x larger and fills the box with no breathing room. Shrinking the mesh
// only while ASCII is active makes the glyph sphere sit centred with the same
// margin as the bloom core (Phase 41.2: "ASCII needs to match the bloom
// setup"), and gives the talk pulse real headroom to swell into.
const ASCII_FRAME = 0.66;

// State -> design-token hex fallbacks (resolved from CSS at init)
const STATE_COLOR_FALLBACKS = {
  idle:      '#4ec9b0',
  listening: '#f85149',
  thinking:  '#d29922',
  speaking:  '#3fb950',
};
const stateColors = Object.assign({}, STATE_COLOR_FALLBACKS);

// Uniforms
const uTime    = { value: 0.0 };
const uFFT     = { value: fftBins };
const uColor   = { value: new THREE.Color(stateColors.idle) };
const uOpacity = { value: 0.85 };
// >0 ONLY in ASCII mode: spreads + boosts the orb's surface brightness so the
// glyph ramp resolves into varied letters AND numbers (bright camera-facing
// centre → dense glyphs like 8/0/@, dim rim → light glyphs like .,:). Stays 0
// for classic/bloom/network so those modes render exactly as tuned — the
// "bloom looks correct" contract is preserved (Phase 41.2).
const uContrast = { value: 0.0 };

// ── Shader sources ────────────────────────────────────────────────────────────

const VERTEX_SHADER = `
  uniform float uTime;
  uniform float uFFT[64];
  uniform vec3  uColor;
  varying vec3 vNormal;
  varying vec3 vPosition;

  float snoise(vec3 p) {
    return sin(p.x * 1.3 + uTime * 0.7)
         * cos(p.y * 1.7 + uTime * 0.5)
         * sin(p.z * 1.1 + uTime * 0.9);
  }

  void main() {
    vNormal = normalize(normalMatrix * normal);
    vec3 n = normalize(normal);

    float theta = acos(clamp(n.y, -1.0, 1.0));
    float phi   = atan(n.z, n.x) + 3.14159265;

    float thetaN = theta / 3.14159265;
    float phiN   = phi   / (2.0 * 3.14159265);

    int lowBin  = int(thetaN * 15.0);
    int highBin = 16 + int(phiN * 47.0);

    float lowAmp  = uFFT[lowBin];
    float highAmp = uFFT[highBin];

    float bigPush = lowAmp * 0.30;
    float fineTex = highAmp * 0.04 * snoise(n * 5.0);

    // Idle breath is intentionally GENTLE (0.05, was 0.08) so the whole-orb
    // scale pulse driven by speech energy (applied in animate()) clearly reads
    // as "talking" against the quiet baseline instead of blending into it
    // (Phase 41.2 talk-sync gap-fix).
    float breathFreq = 0.25;
    float breathAmp  = 0.05;
    float displacement = bigPush + fineTex
      + breathAmp * sin(uTime * 6.28318 * breathFreq);

    vec3 newPos = position + n * displacement;
    vPosition = newPos;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(newPos, 1.0);
  }
`;

const FRAGMENT_SHADER = `
  uniform vec3  uColor;
  uniform float uOpacity;
  uniform float uContrast;
  varying vec3 vNormal;
  varying vec3 vPosition;

  void main() {
    vec3 viewDir = normalize(cameraPosition - vPosition);
    float ndv = max(dot(normalize(vNormal), viewDir), 0.0);
    float fresnel = pow(1.0 - ndv, 2.5);

    vec3 col = uColor + uColor * fresnel * 0.35;
    // ASCII mode only (uContrast>0): give the sphere a strong camera-facing
    // brightness gradient and overall boost, so the AsciiEffect luminance ramp
    // spans from light punctuation at the rim through letters to dense numbers
    // (0/8) and @ at the bright centre — the classic letters-and-numbers look.
    // uContrast==0 leaves classic/bloom/network untouched.
    col *= mix(1.0, (0.30 + 0.95 * ndv) * 2.0, uContrast);
    float alpha = uOpacity - fresnel * 0.15;
    gl_FragColor = vec4(col, clamp(alpha, 0.3, 1.0));
  }
`;

// ── Helpers ───────────────────────────────────────────────────────────────────

function resolveToken(canvasEl, prop, fallback) {
  try {
    const v = getComputedStyle(canvasEl).getPropertyValue(prop).trim();
    return v || fallback;
  } catch (_) {
    return fallback;
  }
}

function onResize(canvasEl) {
  if (!renderer || !camera) return;
  const w = canvasEl.clientWidth  || canvasEl.offsetWidth  || 300;
  const h = canvasEl.clientHeight || canvasEl.offsetHeight || 300;
  renderer.setSize(w, h, false);
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
  // Bloom: resize composer too (Pitfall 4 — both renderer AND composer must update)
  if (currentStyle === 'bloom' && composer) {
    composer.setSize(w, h);
  }
  // ASCII: resize effect overlay too
  if (currentStyle === 'ascii' && asciiEffect) {
    asciiEffect.setSize(w, h);
  }
}

// ── Render pipeline inits ─────────────────────────────────────────────────────

function initBloom(w, h) {
  // Build EffectComposer pipeline: RenderPass → UnrealBloomPass → OutputPass (D-06 / D-07)
  composer = new EffectComposer(renderer);
  composer.addPass(new RenderPass(scene, camera));
  bloomPass = new UnrealBloomPass(
    new THREE.Vector2(w, h),
    1.5,   // initial strength (overridden each frame by FFT energy — D-06)
    0.5,   // radius — wider halo so the glow reads as bloom
    0.55   // threshold — only the brighter parts of the orb bloom, so it glows
           // with a halo instead of blowing out to a flat blob. 0.85 (original)
           // was above the shader luminance so nothing bloomed (flat sphere);
           // 0.0 bloomed everything (washed-out). 0.55 is the balance (41.2 gap-fix).
  );
  composer.addPass(bloomPass);
  composer.addPass(new OutputPass());
}
function initAscii(canvasEl) {
  // AsciiEffect wraps the renderer and outputs to a <div> overlay, NOT the <canvas> (D-07).
  // Charset is the classic three.js LETTERS-AND-NUMBERS ramp (space→dense:
  // . , : ; i 1 t f L C G 0 8 @) rather than the symbol ramp ' .:-+*=%@#', per
  // the 41.2 request to render the orb with letters and numbers. Same space→dense
  // ordering, so `invert:true` keeps the correct polarity (black bg → space,
  // bright orb → dense glyph — verified against AsciiEffect.asciifyImage()).
  asciiEffect = new AsciiEffect(renderer, ' .,:;i1tfLCG08@', { invert: true });
  const w = canvasEl.clientWidth  || canvasEl.offsetWidth  || 300;
  const h = canvasEl.clientHeight || canvasEl.offsetHeight || 300;
  asciiEffect.setSize(w, h);
  // AsciiEffect samples the rendered pixels; the orb's normal TRANSPARENT clear
  // (setClearColor alpha 0) makes every background pixel report alpha==0, which
  // AsciiEffect forces to max brightness → with invert:true that is the DENSEST
  // glyph ('#'), flooding the entire grid instead of showing the orb on a blank
  // field. Render ASCII over an OPAQUE black background so the background reads
  // as brightness 0 → space glyph. The transparent clear is restored on leave
  // (setStyle teardown) so bloom/classic/network keep their see-through look.
  renderer.setClearColor(0x000000, 1);
  // Overlay fills the orb box and FLEX-CENTERS the glyph grid. The AsciiEffect
  // <table> renders at its own natural size (smaller than the canvas), so
  // without centering it top-left-aligns and the sphere reads as off-centre /
  // "not lined up in a circle". width/height 100% + flex centres it.
  const ael = asciiEffect.domElement;
  ael.style.color = 'white';
  ael.style.backgroundColor = 'black';

  // ── Overlay positioning (Phase 41.2 ASCII-centering gap-fix) ──────────────
  // The overlay must sit exactly over the canvas box and flex-CENTRE the glyph
  // grid (a naturally content-sized <table>). The earlier build sized the
  // overlay `height:100%` of `canvasEl.parentElement` and hid the canvas with
  // `display:none`. In the real app that parent is `<div class="orb-region">`,
  // which has NO CSS height — it sizes to its only child, the canvas. Hiding
  // the canvas with display:none therefore COLLAPSED .orb-region to height 0,
  // so `height:100%` resolved to 0 and the glyph grid overflowed from the top
  // (it rendered pinned to the top of the overlay, over the state pill — the
  // exact "ASCII not centred / at the top" bug). A fixed-height probe container
  // hid this divergence. Fix, parent-layout-independent:
  //   1. hide the canvas with `visibility:hidden` (NOT display:none) so it keeps
  //      its layout box and .orb-region does not collapse; and
  //   2. size the overlay to the canvas's REAL measured box in px and position
  //      it at the canvas's own offset, so centring never depends on an
  //      ancestor's resolved height.
  const parent = canvasEl.parentElement;
  if (parent && getComputedStyle(parent).position === 'static') {
    parent.style.position = 'relative';
  }
  // Measure the canvas box BEFORE hiding it. offsets are relative to the nearest
  // positioned ancestor, which is now `parent` (the overlay's offset parent), so
  // top/left line the overlay up exactly over the canvas.
  const boxW = canvasEl.offsetWidth  || canvasEl.clientWidth  || w;
  const boxH = canvasEl.offsetHeight || canvasEl.clientHeight || h;
  ael.style.position = 'absolute';
  ael.style.top = canvasEl.offsetTop + 'px';
  ael.style.left = canvasEl.offsetLeft + 'px';
  ael.style.width = boxW + 'px';
  ael.style.height = boxH + 'px';
  ael.style.display = 'flex';
  ael.style.alignItems = 'center';
  ael.style.justifyContent = 'center';
  ael.style.overflow = 'hidden'; // crop any fringe glyphs; never bleed over the settings panel
  if (parent) parent.appendChild(ael);
  canvasEl.style.visibility = 'hidden'; // keep layout box; ASCII overlay shows in its place
}

function initNetwork() {
  // Node-link network using core THREE only — no addon (D-03 network preset)
  const N = 20;
  const nodePos = [];
  for (let i = 0; i < N; i++) {
    nodePos.push(
      (Math.random() - 0.5) * 2.0,
      (Math.random() - 0.5) * 2.0,
      (Math.random() - 0.5) * 2.0
    );
  }

  // Build edges: connect all pairs within distance threshold
  const edgeVerts = [];
  const THRESH = 1.1;
  for (let i = 0; i < N; i++) {
    for (let j = i + 1; j < N; j++) {
      const dx = nodePos[i*3]   - nodePos[j*3];
      const dy = nodePos[i*3+1] - nodePos[j*3+1];
      const dz = nodePos[i*3+2] - nodePos[j*3+2];
      if (Math.sqrt(dx*dx + dy*dy + dz*dz) < THRESH) {
        edgeVerts.push(
          nodePos[i*3], nodePos[i*3+1], nodePos[i*3+2],
          nodePos[j*3], nodePos[j*3+1], nodePos[j*3+2]
        );
      }
    }
  }
  // Fallback: chain all nodes if density is too low for threshold
  if (edgeVerts.length === 0) {
    for (let i = 0; i < N - 1; i++) {
      edgeVerts.push(
        nodePos[i*3], nodePos[i*3+1], nodePos[i*3+2],
        nodePos[(i+1)*3], nodePos[(i+1)*3+1], nodePos[(i+1)*3+2]
      );
    }
  }

  networkGeometry = new THREE.BufferGeometry();
  networkGeometry.setAttribute(
    'position',
    new THREE.Float32BufferAttribute(edgeVerts, 3)
  );
  networkGeometry.setDrawRange(0, 0); // start empty; animate() grows drawRange.count

  const lineColor = new THREE.Color(stateColors[currentState] || stateColors.idle);
  networkMaterial = new THREE.LineBasicMaterial({
    color: lineColor,
    transparent: true,
    opacity: 0.8,
  });

  networkLines = new THREE.LineSegments(networkGeometry, networkMaterial);
  networkLines.userData.isNetwork = true;
  scene.add(networkLines);
}

// ── Main animate loop (classic path only; mode branches added in Tasks 2-3) ──

function animate() {
  if (!renderer || !scene || !camera || !clock) return;
  const delta = clock.getDelta();
  uTime.value = clock.getElapsedTime();

  // ── Whole-orb pulse (Phase 41.2 talk-sync gap-fix) ──────────────────────
  // Low-pass the low-band FFT energy so the swell is smooth, not jittery.
  // fftBins are already normalized to [0,1] by updateFFT().
  let rawEnergy = 0.0;
  for (let i = 0; i < 8; i++) rawEnergy += fftBins[i];
  rawEnergy /= 8.0;
  smoothedEnergy += (rawEnergy - smoothedEnergy) * 0.25;

  // Talking → the orb visibly swells with voice amplitude (the unmistakable
  // "the orb is talking" signal). Thinking → there is no audio to react to, so
  // synthesize a distinct, slower breathing pulse so the thinking state still
  // reads as actively "alive" (Phase 41.2 user ask: pulse synced to talking +
  // a pulse while thinking). Idle/listening → pulse≈1.0, only the gentle
  // shader breath moves, so the talk pulse stands out against it.
  if (orbMesh && !reducedMotion) {
    let pulse;
    if (currentState === 'thinking') {
      pulse = 1.0 + 0.06 * (0.5 + 0.5 * Math.sin(uTime.value * 6.28318 * 0.8));
    } else {
      pulse = 1.0 + smoothedEnergy * 0.22;
    }
    // ASCII sphere is shrunk to match the bloom core's footprint (see ASCII_FRAME).
    const modeFrame = (currentStyle === 'ascii') ? ASCII_FRAME : 1.0;
    orbMesh.scale.setScalar(baseScale * pulse * modeFrame);
    if (innerGlow) innerGlow.scale.setScalar(baseScale * pulse * modeFrame);
  }

  // Color lerp (0.3s ease)
  if (colorLerp < 1.0) {
    colorLerp = Math.min(1.0, colorLerp + delta / 0.3);
    currentColor.lerp(targetColor, colorLerp);
    uColor.value.copy(currentColor);
    if (auraLight) auraLight.color.copy(currentColor);
    if (innerGlow && innerGlow.material) {
      innerGlow.material.color.copy(currentColor);
      innerGlow.material.emissive.copy(currentColor);
    }
    if (networkMaterial) networkMaterial.color.copy(currentColor);
  }

  // State-specific motion
  if (!reducedMotion && orbMesh && currentState === 'thinking') {
    // 30 deg/s clockwise rotation
    orbMesh.rotation.y += 0.5236 * delta;
  }

  // Render based on current style (exhaustive four-mode switch — D-03)
  if (currentStyle === 'bloom' && composer && bloomPass) {
    // D-06: drive bloom strength from the same smoothed low-band energy that
    // drives the scale pulse, so the glow brightens in lock-step with the swell
    // (Phase 41.2 talk-sync). range [0.3, 1.0] — glows at idle, brightens/pulses
    // with speech WITHOUT blowing out to a solid blob at loud peaks.
    bloomPass.strength = 0.3 + smoothedEnergy * 0.7;
    composer.render();
  } else if (currentStyle === 'ascii' && asciiEffect) {
    asciiEffect.render(scene, camera);
  } else if (currentStyle === 'network') {
    // Grow drawRange.count each frame for line-by-line entry animation
    if (networkGeometry) {
      const total = networkGeometry.attributes.position.count;
      const dr    = networkGeometry.drawRange;
      if (dr.count < total) {
        dr.count = Math.min(dr.count + 6, total);
      }
    }
    renderer.render(scene, camera);
  } else {
    // classic — unchanged from original (REND-02 default/fallback)
    renderer.render(scene, camera);
  }
}

// ── Public API ────────────────────────────────────────────────────────────────

window.ironHermesOrb = {

  // `appearance` (optional) — {style, baseHue, size, glow} to apply DURING init so
  // the orb is born with its persisted look. Callers that don't persist appearance
  // (e.g. the avatar-error silent restore) omit it and get the classic defaults.
  init(canvasId, appearance) {
    canvas = document.getElementById(canvasId);
    if (!canvas) {
      console.warn('[ironHermesOrb] canvas not found:', canvasId);
      return;
    }

    // prefers-reduced-motion contract (UI-SPEC orb section)
    const motionPref = resolveToken(canvas, '--orb-motion', 'full');
    reducedMotion = (motionPref === 'none');

    // Resolve state colors from CSS custom properties
    stateColors.idle      = resolveToken(canvas, '--accent-primary', STATE_COLOR_FALLBACKS.idle);
    stateColors.listening = resolveToken(canvas, '--danger',         STATE_COLOR_FALLBACKS.listening);
    stateColors.thinking  = resolveToken(canvas, '--warn',           STATE_COLOR_FALLBACKS.thinking);
    stateColors.speaking  = resolveToken(canvas, '--success',        STATE_COLOR_FALLBACKS.speaking);

    // Renderer
    renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    const w = canvas.clientWidth  || canvas.offsetWidth  || 300;
    const h = canvas.clientHeight || canvas.offsetHeight || 300;
    renderer.setSize(w, h, false);
    renderer.setClearColor(0x000000, 0);

    scene = new THREE.Scene();
    clock = new THREE.Clock();

    camera = new THREE.PerspectiveCamera(45, w / h, 0.1, 100);
    camera.position.z = 3;

    // Orb geometry: IcosahedronGeometry(r=1.0, detail=4) — smooth subdivision, no face
    const geo = new THREE.IcosahedronGeometry(1.0, 4);
    const mat = new THREE.ShaderMaterial({
      vertexShader:   VERTEX_SHADER,
      fragmentShader: FRAGMENT_SHADER,
      uniforms: { uTime, uFFT, uColor, uOpacity, uContrast },
      transparent: true,
      side: THREE.FrontSide,
    });

    const initCol = new THREE.Color(stateColors.idle);
    uColor.value.copy(initCol);
    currentColor.copy(initCol);
    targetColor.copy(initCol);

    orbMesh = new THREE.Mesh(geo, mat);
    scene.add(orbMesh);

    // Inner emissive sphere for sub-surface glow (UI-SPEC: ~20% brightness)
    const innerGeo = new THREE.SphereGeometry(0.85, 16, 16);
    const innerMat = new THREE.MeshStandardMaterial({
      color: initCol,
      emissive: initCol,
      emissiveIntensity: 0.2,
      transparent: true,
      opacity: 0.15,
      side: THREE.BackSide,
    });
    innerGlow = new THREE.Mesh(innerGeo, innerMat);
    scene.add(innerGlow);

    // Aura point light
    auraLight = new THREE.PointLight(initCol, 1.5, 6);
    auraLight.position.set(0, 0, 0);
    scene.add(auraLight);

    // Ambient fill
    scene.add(new THREE.AmbientLight(0xffffff, 0.3));

    if (typeof ResizeObserver !== 'undefined') {
      const ro = new ResizeObserver(() => onResize(canvas));
      ro.observe(canvas);
    }

    // ── Phase 41.2 (G-41.2-11 full persistence): born-correct appearance ──────
    // Apply the caller-supplied persisted appearance HERE, during init, so a fresh
    // reload shows the saved Style/hue/size/glow on the very first frame. This is
    // load-bearing, not a convenience: the post-init setStyle/setBaseHue/setGlow
    // bridges fire ~before this init runs (the Rust init effect sleeps 50ms first),
    // so setStyle/setGlow hit a null `renderer`/`innerGlow` and no-op — and they
    // never re-fire without a later signal change, leaving the orb stuck on
    // classic/default on every reload. renderer, orbMesh, innerGlow and auraLight
    // all exist above this point, so the setters apply cleanly. Order: setStyle
    // first so its mode pipeline (bloom composer / ascii overlay / network mesh +
    // networkMaterial) exists before setBaseHue targets it.
    if (appearance && typeof appearance === 'object') {
      if (typeof appearance.style === 'string'
          && ['bloom', 'ascii', 'network'].includes(appearance.style)) {
        this.setStyle(appearance.style);
      }
      if (Number.isFinite(appearance.baseHue)) this.setBaseHue(appearance.baseHue);
      if (Number.isFinite(appearance.size))    this.setSize(appearance.size);
      if (Number.isFinite(appearance.glow))    this.setGlow(appearance.glow);
    }

    if (reducedMotion) {
      // setBaseHue only updates targetColor; with no animate loop nothing lerps it
      // into the live uniforms, so sync them for the single static frame (animate()
      // does this in the full-motion path). setSize/setGlow already applied their
      // reduced-motion effects inline.
      uColor.value.copy(targetColor);
      currentColor.copy(targetColor);
      if (auraLight) auraLight.color.copy(targetColor);
      if (innerGlow && innerGlow.material) {
        innerGlow.material.color.copy(targetColor);
        innerGlow.material.emissive.copy(targetColor);
      }
      if (networkMaterial) networkMaterial.color.copy(targetColor);
      // Render one static frame in the active mode (setStyle may have switched it).
      if (currentStyle === 'bloom' && composer) {
        composer.render();
      } else if (currentStyle === 'ascii' && asciiEffect) {
        asciiEffect.render(scene, camera);
      } else {
        renderer.render(scene, camera);
      }
      return;
    }

    renderer.setAnimationLoop(animate);
  },

  updateFFT(bins) {
    if (!bins || bins.length === 0) return;
    const len = Math.min(bins.length, 64);
    // Auto-detect byte domain (0-255) vs normalized (0-1)
    let maxVal = 0;
    for (let i = 0; i < len; i++) { if (bins[i] > maxVal) maxVal = bins[i]; }
    const scale = (maxVal > 1.0) ? (1.0 / 255.0) : 1.0;
    for (let i = 0; i < len; i++) { fftBins[i] = bins[i] * scale; }
    for (let i = len; i < 64; i++) { fftBins[i] = 0; }
  },

  setState(state) {
    if (currentState === state) return;
    currentState = state;

    const hexColor = stateColors[state] || stateColors.idle;
    targetColor.set(hexColor);
    colorLerp = 0.0;

    if (reducedMotion && renderer && scene && camera) {
      uColor.value.copy(targetColor);
      currentColor.copy(targetColor);
      if (auraLight) auraLight.color.copy(targetColor);
      if (innerGlow && innerGlow.material) {
        innerGlow.material.color.copy(targetColor);
        innerGlow.material.emissive.copy(targetColor);
      }
      if (networkMaterial) networkMaterial.color.copy(targetColor);
      // Render one static frame in the currently active mode
      if (currentStyle === 'bloom' && composer) {
        composer.render();
      } else if (currentStyle === 'ascii' && asciiEffect) {
        asciiEffect.render(scene, camera);
      } else {
        renderer.render(scene, camera);
      }
    }

    if (orbMesh && state !== 'thinking') {
      orbMesh.rotation.y = 0;
    }
  },

  // ── D-03: Render-mode switching ─────────────────────────────────────────────

  setStyle(style) {
    if (!renderer) return;
    // Validate against known modes only (T-40.5-05-01 input clamping)
    const validStyles = ['classic', 'bloom', 'ascii', 'network'];
    if (!validStyles.includes(style)) return;

    // ── Teardown previous mode ───────────────────────────────────────────────

    // Leave ASCII: remove overlay div, restore canvas (Pitfall 5 + T-40.5-05-03)
    if (currentStyle === 'ascii' && asciiEffect) {
      try { asciiEffect.domElement.remove(); } catch (_) {}
      asciiEffect = null;
      if (canvas) {
        // Restore visibility (initAscii used visibility:hidden, not display, so
        // .orb-region would not collapse — Phase 41.2 ASCII-centering fix). Also
        // clear display in case a legacy build left it 'none'.
        canvas.style.visibility = '';
        canvas.style.display = '';
        // AsciiEffect.setSize() resized the renderer AND wrote an inline px size
        // onto the canvas for glyph sampling. Clear it so the WebGL modes
        // (classic/bloom/network) render at the true box size again instead of
        // into a corner (41.2 ascii→other switch fix).
        canvas.style.width = '';
        canvas.style.height = '';
      }
      // Restore the transparent clear the other modes rely on (initAscii forced
      // it opaque so the ASCII background read as blank — 41.2 gap-fix).
      if (renderer) renderer.setClearColor(0x000000, 0);
      // Re-fit renderer + camera to the real box size (undo AsciiEffect.setSize).
      if (renderer && canvas) onResize(canvas);
    }

    // Leave Bloom: release composer reference
    if (currentStyle === 'bloom') {
      composer  = null;
      bloomPass = null;
    }

    // Leave Network: remove geometry from scene, dispose GL resources
    if (currentStyle === 'network') {
      if (networkLines && scene) scene.remove(networkLines);
      if (networkGeometry) { networkGeometry.dispose(); networkGeometry = null; }
      if (networkMaterial) { networkMaterial.dispose(); networkMaterial = null; }
      networkLines = null;
    }

    currentStyle = style;

    // ASCII spreads/boosts orb brightness for a varied letters+numbers ramp;
    // every other mode keeps uContrast=0 so its look is exactly as tuned.
    uContrast.value = (style === 'ascii') ? 1.0 : 0.0;

    if (!scene || !camera || !canvas) return;

    // ── Initialize new mode ──────────────────────────────────────────────────

    const w = canvas.clientWidth  || canvas.offsetWidth  || 300;
    const h = canvas.clientHeight || canvas.offsetHeight || 300;

    if (style === 'bloom')   { initBloom(w, h); }
    else if (style === 'ascii')   { initAscii(canvas); }
    else if (style === 'network') { initNetwork(); }
    // 'classic' needs no setup — renderer.render(scene,camera) is always ready

    // Reduced-motion: one static frame per mode, then stop (--orb-motion: none)
    if (reducedMotion) {
      // The animate loop (which applies ASCII_FRAME) never runs here, so apply
      // the per-mode framing scale once for this static frame.
      if (orbMesh) {
        const mf = (style === 'ascii') ? ASCII_FRAME : 1.0;
        orbMesh.scale.setScalar(baseScale * mf);
        if (innerGlow) innerGlow.scale.setScalar(baseScale * mf);
      }
      if (style === 'bloom' && composer) {
        composer.render();
      } else if (style === 'ascii' && asciiEffect) {
        asciiEffect.render(scene, camera);
      } else if (style === 'network') {
        if (networkGeometry) {
          networkGeometry.setDrawRange(0, networkGeometry.attributes.position.count);
        }
        renderer.render(scene, camera);
      } else {
        renderer.render(scene, camera);
      }
      return;
    }

    // Restart animation loop (setAnimationLoop is idempotent — no-op if running)
    renderer.setAnimationLoop(animate);
  },

  // ── D-05: Per-state hue derivation from base hue ────────────────────────────

  setBaseHue(hue) {
    // Normalize to [0, 360) and clamp (T-40.5-05-01)
    const h = ((Math.round(Number(hue) || 0) % 360) + 360) % 360;
    // Derive four distinct per-state colors as relative HSL offsets (D-05)
    stateColors.idle      = `hsl(${h}, 70%, 65%)`;
    stateColors.listening = `hsl(${(h + 150) % 360}, 90%, 55%)`;
    stateColors.thinking  = `hsl(${(h +  60) % 360}, 85%, 55%)`;
    stateColors.speaking  = `hsl(${(h - 50 + 360) % 360}, 80%, 55%)`;
    // Re-apply to current state color immediately (never one static color)
    const col = new THREE.Color(stateColors[currentState] || stateColors.idle);
    targetColor.copy(col);
    colorLerp = 0.0;
    if (networkMaterial) networkMaterial.color.copy(col);
  },

  // ── D-02: Size + Glow customization ─────────────────────────────────────────

  setSize(scale) {
    // Clamp to [0.5, 2.0] (T-40.5-05-01). Store as baseScale — animate()
    // multiplies it by the live talk/think pulse each frame (Phase 41.2), so we
    // must NOT write orbMesh.scale directly here or the pulse would be clobbered
    // every render. Reduced-motion has no animate loop, so apply once now.
    baseScale = Math.max(0.5, Math.min(2.0, Number(scale) || 1.0));
    if (reducedMotion) {
      if (orbMesh)   orbMesh.scale.setScalar(baseScale);
      if (innerGlow) innerGlow.scale.setScalar(baseScale);
    }
  },

  setGlow(intensity) {
    // Clamp to [0.0, 1.0] (T-40.5-05-01)
    const g = Math.max(0.0, Math.min(1.0, Number(intensity) || 0.0));
    if (innerGlow && innerGlow.material) {
      innerGlow.material.emissiveIntensity = g;
    }
    // Map [0,1] → [0, 3.0] for aura light intensity
    if (auraLight) auraLight.intensity = g * 3.0;
  },

  // ── Teardown ─────────────────────────────────────────────────────────────────

  destroy() {
    if (renderer) {
      renderer.setAnimationLoop(null);
      // forceContextLoss() BEFORE dispose(): clears the canvas immediately (no
      // frozen last frame when swapping back to the orb from the avatar — FE-02)
      // and frees the WebGL context slot (avoids ~16-context exhaustion — Pitfall 2)
      // when the same <canvas id="orb-canvas"> is reused by avatar.js after a swap.
      try { renderer.forceContextLoss(); } catch (_) {}
      renderer.dispose();
    }
    // Clean up ASCII overlay (Pitfall 5 + T-40.5-05-03)
    if (asciiEffect) {
      try { asciiEffect.domElement.remove(); } catch (_) {}
      asciiEffect = null;
      if (canvas) { canvas.style.visibility = ''; canvas.style.display = ''; }
    }
    // Clean up network geometry + scene objects
    if (networkLines && scene) scene.remove(networkLines);
    if (networkGeometry) { networkGeometry.dispose(); networkGeometry = null; }
    if (networkMaterial) { networkMaterial.dispose(); networkMaterial = null; }
    networkLines = null;
    // Release bloom pipeline
    composer  = null;
    bloomPass = null;

    if (orbMesh)  { orbMesh.geometry.dispose(); orbMesh.material.dispose(); }
    if (innerGlow){ innerGlow.geometry.dispose(); innerGlow.material.dispose(); }
    renderer     = null;
    scene        = null;
    camera       = null;
    orbMesh      = null;
    innerGlow    = null;
    auraLight    = null;
    clock        = null;
    canvas       = null;
    colorLerp    = 1.0;
    currentState = 'idle';
    currentStyle = 'classic';
    baseScale      = 1.0;
    smoothedEnergy = 0.0;
    uContrast.value = 0.0;
    fftBins.fill(0);
  },
};
