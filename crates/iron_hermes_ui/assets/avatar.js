// Phase 40.2 (Rigged talking-head avatar) — IronHermes avatar render module.
// Self-hosted Three.js r0.184.0 scene rendering facecap.glb (ARKit morph head)
// OR the rigged Groovy GLB (842-joint Auto-Rig Pro skeleton + pre-composed
// "Mouth Vis *" / "Eye * Closed" morphs), lip-synced to the live realtime voice.
// Sibling of orb.js (same public API shape so orb_canvas.rs plumbing is reused).
//
// API: window.ironHermesAvatar
//   .init(canvasId, glbUrl, preset)
//   .updateFFT(bins)            -- Float32Array/Array of 64 values (idle breath only, D-04)
//   .setState(state)            -- 'idle'|'listening'|'thinking'|'speaking'
//   .setGazeMode(mode)          -- 'manual' (cursor) | 'off'
//   .gesture(kind)              -- 'nod' (yes) | 'shake' (no): a one-shot head gesture
//   .destroy()
//
// preset (third arg) — Plan 04 passes this from Rust PRESET_REGISTRY:
//   { camPos:[x,y,z], lookAt:[x,y,z], fov:degrees, bodyType:"head"|"half"|"full",
//     visemeMap:{ "viseme_aa":"Mouth Vis Ah", ... } | null,
//     blinkMorphs:["Eye L Closed","Eye R Closed"] | null }
//
// TWO RIG PATHS, auto-detected at load:
//   • facecap (no skeleton, ARKit-52, single morph mesh): the original path —
//     OCULUS_TO_ARKIT decomposition, eyeLook* gaze morphs, whole-head-object aim.
//     Preserved byte-for-byte (it works; lip-sync confirmed). visemeMap == null.
//   • rigged Groovy (skeleton + pre-composed viseme morphs, MANY meshes that
//     SHARE blendshape names — body + lashes + brows): visemeMap != null →
//       - morphs are driven on EVERY mesh that has them (multi-mesh), because the
//         face mesh and the eyelash/brow meshes each carry the same shapes and all
//         must move together (this is why a single selected mesh left the mouth/eyes
//         frozen before);
//       - the HEAD is turned by rotating the `head.x` DEFORM BONE only, so the head
//         tracks the cursor / nods / shakes while the BODY stays put (impossible on
//         the boneless model — that rotated the whole body);
//       - idle breathing is a gentle whole-model bob (feet are off-frame in the
//         head-and-shoulders crop, so this reads purely as breath).
//
// Security: no external network calls; all imports relative (self-hosted, REND-01).
// Numeric inputs only (viseme scores, FFT bins); no eval of user data.
//
// Audio seam (Pitfall 1): wawa's element-based connect helper CANNOT consume the
// realtime audio element (srcObject, no .src, source node already taken). So this
// module builds wawa's OWN analyser on the realtime MediaStream surfaced by the
// Rust ontrack seam as window.__ihRealtimeStream and resumes the context
// (Pitfall 7). It never connects to ctx.destination (already audible).

import * as THREE from './three.module.js';
import { GLTFLoader } from './GLTFLoader.js';
import { MeshoptDecoder } from './meshopt_decoder.module.js';
import { Lipsync } from './wawa-lipsync.js';
import { applyVisemes } from './viseme-map.js';

// ── Module-level state ────────────────────────────────────────────────────────

let renderer = null;
let scene = null;
let camera = null;
let clock = null;
let keyLight = null;
let reducedMotion = false;
let currentState = 'idle';

// facecap (single-morph-mesh) handles. `head` guards the facecap animate path.
let head = null;   // facecap mesh_2 (the morph mesh)
let teeth = null;  // facecap mesh_3
let eyeL = null;   // facecap eyeLeft
let eyeR = null;   // facecap eyeRight
let arkitIndex = null;   // { morphName: index } for the facecap mesh
let influences = null;   // head.morphTargetInfluences (facecap)
let visemeTarget = null; // per-frame facecap morph target buffer

// rigged (skeleton + multi-mesh morphs) handles. `skinnedMode` guards its path.
let skinnedMode = false;
let morphMeshes = [];   // [{ infl:Float32Array, target:Float32Array }]
let morphByName = {};   // morphName -> [[meshIdx, morphIdx], ...]  (multi-mesh write)
let controlled = [];    // [{ mi, idx }] — the viseme+blink morphs we drive each frame
let headBone = null;    // the `head.x` deform bone (rigged head aim)
let sceneRoot = null;   // gltf.scene (rigged breath bob)
let sceneBaseY = 0;

// Preset-derived runtime state (set at load from the resolved preset). Null = facecap.
let activeVisemeMap = null;    // { oculus_viseme_name: morph_name } | null
let activeBlinkMorphs = null;  // [leftMorphName, rightMorphName] | null
let activeBodyType = null;     // "head"|"half"|"full" | null
let activeMaterial = null;         // "normal"|"pbr"|"matrix" | null (preset.material)
let activeExpressionMorphs = null; // { state: morphName } | null (preset.expressionMorphs)
let aimScale = 1.0;                // head-aim amplitude (0.6 for boneless bust fallback)
let matrixMaterials = [];          // emissive materials state-tinted each color lerp
const dissolveUniform = { value: 0 };  // shared by shader discard + fx_dissolve morph
let revealT0 = -1;                     // clock time reveal started (-1 = inactive)
let pulseT0 = -1;                      // clock time pulse started (-1 = inactive)

// wawa lip-sync driver + audio-seam state.
let lip = null;
let streamAttached = false;
let attachTimer = null;

// 64-bin FFT (idle breath amplitude only — D-04).
const fftBins = new Float32Array(64);
let idleBreathAmp = 0;

// Blink scheduler (randomized 2–6s interval, ~130ms ramp 0→1→0).
let nextBlinkAt = 0;
let blinkPhase = 0;
const BLINK_RAMP = 0.13;
const VISEME_LERP = 0.08;
const BREATH_PERIOD = 4.0;
const COLOR_LERP = 0.3;
const REVEAL_DUR = 2.0;   // seconds — load-in dissolve 1 → 0
const PULSE_DUR = 0.3;    // seconds — state-change glitch pulse
const PULSE_AMP = 0.15;   // peak dissolve weight of the pulse

// ── Gaze / head aim (cursor-follow) ─────────────────────────────────────────────
// One unified aim target — the `head.x` bone (rigged) or the head object
// (facecap). aimNode() rotates its captured base WORLD orientation, so the turn
// is symmetric left/right and robust to the rig's baked bone frame (this fixes
// the old "head stops in the centre, only turns one way" bug).
let gazeMode = 'manual';       // 'manual' (pointer) | 'off'
let pointerHandler = null;
let targetYaw = 0;             // desired gaze [-1,1] (+ = screen-right)
let targetPitch = 0;           // desired gaze [-1,1] (+ = up)
let lastMoveAt = -1e9;         // clock time of the last mouse move (idle detection)
// Eyes ease quickly toward the target; the head eases slowly toward a DEAD-ZONED
// target — so a small cursor move turns ONLY the eyes, and the head joins (and
// lags) only once the gaze passes the threshold. When the mouse goes idle, both
// ease back to forward.
let gazeYaw = 0;               // eased EYE gaze (fast)
let gazePitch = 0;
let headYaw = 0;               // eased HEAD gaze (slow, dead-zoned)
let headPitch = 0;
let headAimYaw = 0;            // head's actual applied world yaw (incl. sway/gesture)
let headAimPitch = 0;
const GAZE_EASE = 0.10;        // seconds — eyes ease (snappy, they track first)
const HEAD_EASE = 0.35;        // seconds — head ease (slower, follows the eyes)
const HEAD_DEADZONE = 0.30;    // gaze fraction the head ignores before it turns
const IDLE_RETURN_DELAY = 1.2; // seconds of no mouse movement → return to forward
const EYE_YAW_MAX = 0.9;       // facecap eyeLook morph influence (horizontal)
const EYE_PITCH_MAX = 0.7;     // facecap eyeLook morph influence (vertical)
const HEAD_YAW = 0.55;         // ~31° max head turn (rad at full target)
const HEAD_PITCH = 0.30;       // ~17° max head pitch
const EYE_YAW = 0.95;          // eye world yaw target (clamped in-socket below)
const EYE_PITCH = 0.60;        // eye world pitch target (clamped in-socket below)
const EYE_MAX_YAW_SOCKET = 0.42;   // max eye-in-socket yaw (rad, ~24°) — anti-googly
const EYE_MAX_PITCH_SOCKET = 0.30; // max eye-in-socket pitch (rad, ~17°)
const GAZE_YAW_SIGN = 1;       // flip to -1 if the head turns the WRONG way L/R
const GAZE_PITCH_SIGN = -1;    // flip to +1 if they look DOWN when the cursor is UP
const EYE_YAW_SIGN = 1;        // flip to -1 if the EYES track the wrong way L/R
const EYE_PITCH_SIGN = -1;     // flip to +1 if the eyes look DOWN when cursor is UP
const IDLE_SWAY = 0.05;        // gentle idle head sway amplitude (rad, ~2.9°)

// Aim target + captured base orientation (world quat + parent-inverse world quat).
let aimTarget = null;
let aimBaseWQ = null;
let aimParentInvWQ = null;
// Eyeball gaze (rigged): static eye meshes re-parented into head.x, aimed in
// world space each frame toward the cursor (recomputed parent-inverse since the
// head moves). eyeMeshes[i] ↔ eyeBaseWQ[i] (rest world orientation).
let eyeMeshes = [];
let eyeBaseWQ = [];
const _qYaw = new THREE.Quaternion();
const _qPitch = new THREE.Quaternion();
const _qAim = new THREE.Quaternion();
const _qTmp = new THREE.Quaternion();
const _AXIS_Y = new THREE.Vector3(0, 1, 0);
const _AXIS_X = new THREE.Vector3(1, 0, 0);

// One-shot gesture (nod = yes, shake = no). Driven additively on top of the gaze
// aim, with a sin envelope so it starts and ends at the rest orientation.
let gestureKind = null;        // null | 'nod' | 'shake'
let gestureT0 = 0;
const GESTURE_DUR = 0.8;       // seconds
const NOD_AMP = 0.32;          // rad (pitch)
const SHAKE_AMP = 0.38;        // rad (yaw)

// Color lerp state (key-light tint by state — mirrors orb.js color affect).
const currentColor = new THREE.Color();
const targetColor = new THREE.Color();
let colorLerp = 1.0;
const STATE_COLOR_FALLBACKS = {
  idle:      '#4ec9b0',
  listening: '#f85149',
  thinking:  '#d29922',
  speaking:  '#3fb950',
};
const stateColors = Object.assign({}, STATE_COLOR_FALLBACKS);

// ── Error signal helper (FE-05, D-11) ────────────────────────────────────────
// Sets window.__ihAvatarError {code,msg,ts}. Rust polls + restores the orb.
// Data only — never eval'd. Never reload; never clear saved prefs.
function signalAvatarError(code, msg) {
  window.__ihAvatarError = { code, msg, ts: Date.now() };
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function resolveToken(canvas, prop, fallback) {
  try {
    const v = getComputedStyle(canvas).getPropertyValue(prop).trim();
    return v || fallback;
  } catch (_) {
    return fallback;
  }
}

function onResize(canvas) {
  if (!renderer || !camera) return;
  const w = canvas.clientWidth  || canvas.offsetWidth  || 300;
  const h = canvas.clientHeight || canvas.offsetHeight || 300;
  renderer.setSize(w, h, false);
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
}

function scheduleNextBlink(now) {
  nextBlinkAt = now + 2 + Math.random() * 4;
}

// facecap single-mesh morph write by name (clamped, graceful-skip).
function setMorph(name, value) {
  const i = arkitIndex && arkitIndex[name];
  if (i !== undefined) influences[i] = value < 0 ? 0 : value > 1 ? 1 : value;
}

// rigged multi-mesh morph TARGET write by name — writes the value to EVERY mesh
// that carries `name` (body + lashes + brows share the same blendshapes). The
// per-frame lerp eases live influences toward these targets.
function setMorphTargetByName(name, value) {
  const list = morphByName[name];
  if (!list) return;
  const v = value < 0 ? 0 : value > 1 ? 1 : value;
  for (let i = 0; i < list.length; i++) {
    morphMeshes[list[i][0]].target[list[i][1]] = v;
  }
}

// Aim a node: rotate its captured base WORLD orientation by yaw (about world-Y)
// and pitch (about world-X), then convert back into the node's local space so it
// composes under any baked bone frame. Symmetric in yaw/pitch by construction.
function aimNode(node, baseWQ, parentInvWQ, yaw, pitch) {
  if (!node || !baseWQ || !parentInvWQ) return;
  _qYaw.setFromAxisAngle(_AXIS_Y, yaw);
  _qPitch.setFromAxisAngle(_AXIS_X, pitch);
  _qAim.copy(_qYaw).multiply(_qPitch).multiply(baseWQ);
  node.quaternion.copy(parentInvWQ).multiply(_qAim);
}

function clamp(x, lo, hi) { return x < lo ? lo : x > hi ? hi : x; }

// Dead-zone + re-map: returns 0 while |x| <= dz, then ramps [dz,1] → [0,1] (sign
// preserved). Used so the head ignores small gaze offsets (eyes-only) and only
// starts turning past the threshold.
function deadzone(x, dz) {
  const a = Math.abs(x);
  if (a <= dz) return 0;
  return Math.sign(x) * (a - dz) / (1 - dz);
}

// Current one-shot gesture offset (yaw,pitch in rad). Clears itself when done.
function gestureOffset(t) {
  if (!gestureKind) return { yaw: 0, pitch: 0 };
  const e = (t - gestureT0) / GESTURE_DUR;
  if (e >= 1) { gestureKind = null; return { yaw: 0, pitch: 0 }; }
  const env = Math.sin(Math.PI * e);                 // 0→1→0 window
  if (gestureKind === 'nod') {
    // Yes: 2 dips, down-first (negative pitch = chin down with GAZE_PITCH_SIGN=-1).
    return { yaw: 0, pitch: Math.sin(2 * Math.PI * 2 * e) * NOD_AMP * env };
  }
  // No: ~2.5 side-to-side shakes.
  return { yaw: Math.sin(2 * Math.PI * 2.5 * e) * SHAKE_AMP * env, pitch: 0 };
}

// Ease gaze toward target and aim the head (bone or object). `eyeLookMorphs`
// drives the facecap eyeLook* shapes so its eyes lead; rigged eyes are bone
// children of head.x and turn with the head, so they need no morphs.
function driveHeadAim(t, delta, eyeLookMorphs) {
  // When the mouse has been still for IDLE_RETURN_DELAY, the target is forward
  // (0,0) so she eases back to the idle/forward pose; otherwise track the cursor.
  const active = gazeMode === 'manual' && (t - lastMoveAt) <= IDLE_RETURN_DELAY;
  const tY = active ? targetYaw : 0;
  const tP = active ? targetPitch : 0;

  // Eyes ease fast toward the FULL target (they lead).
  const eEye = Math.min(1.0, delta / GAZE_EASE);
  gazeYaw   += (tY - gazeYaw) * eEye;
  gazePitch += (tP - gazePitch) * eEye;

  // Head eases slow toward a DEAD-ZONED target (it ignores small offsets, then
  // follows once the gaze passes the threshold).
  const eHead = Math.min(1.0, delta / HEAD_EASE);
  headYaw   += (deadzone(tY, HEAD_DEADZONE) - headYaw) * eHead;
  headPitch += (deadzone(tP, HEAD_DEADZONE) - headPitch) * eHead;

  if (eyeLookMorphs) {
    // facecap eyes lead via the eyeLook* morphs (driven by the fast EYE gaze).
    const r = Math.max(0, gazeYaw), l = Math.max(0, -gazeYaw);
    const u = Math.max(0, gazePitch), d = Math.max(0, -gazePitch);
    setMorph('eyeLookIn_R',   r * EYE_YAW_MAX);
    setMorph('eyeLookOut_L',  r * EYE_YAW_MAX);
    setMorph('eyeLookOut_R',  l * EYE_YAW_MAX);
    setMorph('eyeLookIn_L',   l * EYE_YAW_MAX);
    setMorph('eyeLookUp_R',   u * EYE_PITCH_MAX);
    setMorph('eyeLookUp_L',   u * EYE_PITCH_MAX);
    setMorph('eyeLookDown_R', d * EYE_PITCH_MAX);
    setMorph('eyeLookDown_L', d * EYE_PITCH_MAX);
  }

  const g = gestureOffset(t);
  const motionScale = currentState === 'speaking' ? 0.6 : 1.0;
  const sway = Math.sin(t * 0.5) * IDLE_SWAY * motionScale;
  const breathPitch = Math.sin(t * (2 * Math.PI / BREATH_PERIOD)) * 0.025 * motionScale;
  headAimYaw   = (headYaw   * HEAD_YAW   * GAZE_YAW_SIGN   + sway + g.yaw) * aimScale;
  headAimPitch = (headPitch * HEAD_PITCH * GAZE_PITCH_SIGN + breathPitch + g.pitch) * aimScale;
  aimNode(aimTarget, aimBaseWQ, aimParentInvWQ, headAimYaw, headAimPitch);
}

// Aim the eyeball nodes in-socket toward the cursor, LEADING the head — shared by
// the rigged (Groovy) and facecap (Morph Head) paths. The eyes ride with the head
// (their parent's world includes the head turn) plus a CLAMPED in-socket offset
// so they visibly lead it without going googly. Call right after driveHeadAim()
// (which sets gazeYaw/gazePitch + headAimYaw/headAimPitch this frame).
function aimEyeballs() {
  if (!eyeMeshes.length) return;
  const socketYaw   = clamp(gazeYaw  * EYE_YAW  * EYE_YAW_SIGN  - headAimYaw,
                            -EYE_MAX_YAW_SOCKET,  EYE_MAX_YAW_SOCKET);
  const socketPitch = clamp(gazePitch * EYE_PITCH * EYE_PITCH_SIGN - headAimPitch,
                            -EYE_MAX_PITCH_SOCKET, EYE_MAX_PITCH_SOCKET);
  const eyeWorldYaw   = headAimYaw   + socketYaw;
  const eyeWorldPitch = headAimPitch + socketPitch;
  scene.updateMatrixWorld(true);   // propagate the head turn to the eyes' parents
  for (let i = 0; i < eyeMeshes.length; i++) {
    const em = eyeMeshes[i];
    const pInv = em.parent.getWorldQuaternion(_qTmp).invert();
    aimNode(em, eyeBaseWQ[i], pInv, eyeWorldYaw, eyeWorldPitch);
  }
}

// Attach wawa's analyser to the realtime MediaStream (Pitfall 1) + resume the
// context (Pitfall 7). Idempotent. NEVER connects to destination (already
// audible) and NEVER touches the audio element / element-source helper.
async function attachRealtimeStream() {
  if (streamAttached || !lip) return streamAttached;
  const stream = window.__ihRealtimeStream;
  if (!stream || typeof stream.getAudioTracks !== 'function') return false;
  try {
    const src = lip.audioContext.createMediaStreamSource(stream);
    src.connect(lip.analyser);
    await lip.audioContext.resume();
    streamAttached = true;
    if (attachTimer !== null) { clearInterval(attachTimer); attachTimer = null; }
  } catch (e) {
    console.warn('[ironHermesAvatar] realtime stream attach failed:', e);
  }
  return streamAttached;
}

// ── Render loop ────────────────────────────────────────────────────────────────

function animate() {
  try {
    if (!renderer || !scene || !camera || !clock) return;
    const delta = clock.getDelta();
    const t = clock.getElapsedTime();

    if (colorLerp < 1.0) {
      colorLerp = Math.min(1.0, colorLerp + delta / COLOR_LERP);
      currentColor.lerp(targetColor, colorLerp);
      if (keyLight) keyLight.color.copy(currentColor);
      applyMatrixTint(currentColor);
    }

    if (skinnedMode) {
      animateRigged(t, delta);
    } else if (head) {
      animateFacecap(t, delta);
    }

    renderer.render(scene, camera);
  } catch (e) {
    signalAvatarError('RENDER_THROW', String(e));
  }
}

// rigged Groovy: multi-mesh visemes + blink, bone head aim, breath bob.
function animateRigged(t, delta) {
  // ── Mouth: drive the dominant Oculus viseme morph DIRECTLY (model ships
  //    pre-composed "Mouth Vis *" shapes). Zero controlled targets first
  //    (anti-persistence), set the dominant viseme + blink, then lerp.
  for (let i = 0; i < controlled.length; i++) {
    morphMeshes[controlled[i].mi].target[controlled[i].idx] = 0;
  }

  if (lip && streamAttached) {
    lip.processAudio();
    const dominantViseme = lip.viseme || null;     // e.g. "viseme_aa"
    if (dominantViseme && dominantViseme !== 'viseme_sil') {
      const morphName = activeVisemeMap[dominantViseme];
      if (morphName) setMorphTargetByName(morphName, 1.0);
    }
  }

  // Idle blink (timer-driven, independent of audio).
  if (!reducedMotion) {
    const blinkLName = activeBlinkMorphs ? activeBlinkMorphs[0] : 'Eye L Closed';
    const blinkRName = activeBlinkMorphs ? activeBlinkMorphs[1] : 'Eye R Closed';
    if (blinkPhase > 0) {
      blinkPhase += delta;
      const half = BLINK_RAMP / 2;
      let b = blinkPhase < half ? blinkPhase / half : 1 - (blinkPhase - half) / half;
      if (b < 0) b = 0;
      if (blinkPhase >= BLINK_RAMP) { blinkPhase = 0; scheduleNextBlink(t); b = 0; }
      setMorphTargetByName(blinkLName, b);
      setMorphTargetByName(blinkRName, b);
    } else {
      if (nextBlinkAt === 0) scheduleNextBlink(t);
      if (t >= nextBlinkAt) blinkPhase = 1e-6;
    }
  }

  // Expression: hold the current state's morph (zeroed with the controlled
  // set each frame, so leaving the state eases back to neutral over the
  // same ~80 ms lerp as visemes).
  if (activeExpressionMorphs) {
    const exprName = activeExpressionMorphs[currentState];
    if (exprName) setMorphTargetByName(exprName, 0.6);
  }

  // Dissolve weight: reveal envelope (1 → 0) and state-glitch pulse share
  // one uniform; the fx_dissolve morph follows through the controlled lerp.
  if (activeMaterial === 'matrix') {
    let w = 0;
    if (revealT0 >= 0) {
      const e = (t - revealT0) / REVEAL_DUR;
      if (e >= 1) { revealT0 = -1; } else { w = 1 - e; }
    }
    if (pulseT0 >= 0) {
      const e = (t - pulseT0) / PULSE_DUR;
      if (e >= 1) { pulseT0 = -1; }
      else { w = Math.max(w, Math.sin(Math.PI * e) * PULSE_AMP); }
    }
    if (w > 1) w = 1;
    dissolveUniform.value = w;
    setMorphTargetByName('fx_dissolve', w);
  }

  // Lerp the live influences toward target (viseme-lerp ~80ms), clamp [0,1].
  const k = Math.min(1.0, delta / VISEME_LERP);
  for (let i = 0; i < controlled.length; i++) {
    const m = morphMeshes[controlled[i].mi];
    const idx = controlled[i].idx;
    m.infl[idx] += (m.target[idx] - m.infl[idx]) * k;
    if (m.infl[idx] < 0) m.infl[idx] = 0;
    else if (m.infl[idx] > 1) m.infl[idx] = 1;
  }

  // Head aim: turn the head.x bone (or whole bust) toward the cursor + gesture.
  if (!reducedMotion) {
    if (aimTarget) {
      driveHeadAim(t, delta, false);
      // Eyeball gaze: eyes lead the head toward the cursor (shared with facecap).
      aimEyeballs();
    }
    // Breathing: gentle whole-model bob — independent of head-aim so a
    // boneless model still breathes.
    if (sceneRoot) {
      const breath = Math.sin(t * (2 * Math.PI / BREATH_PERIOD));
      sceneRoot.position.y = sceneBaseY + breath * 0.02;
    }
  }
}

// facecap (ARKit head): preserved original behavior — OCULUS_TO_ARKIT
// decomposition, eyeLook gaze morphs, whole-head-object aim, breath/sway.
function animateFacecap(t, delta) {
  const speaking = currentState === 'speaking';

  if (lip && streamAttached) {
    lip.processAudio();
    const dominantViseme = lip.viseme || null;
    const scores = dominantViseme ? { [dominantViseme]: 1.0 } : {};
    applyVisemes(scores, arkitIndex, visemeTarget);
  } else {
    for (let i = 0; i < visemeTarget.length; i++) visemeTarget[i] = 0;
  }
  const k = Math.min(1.0, delta / VISEME_LERP);
  for (let i = 0; i < influences.length; i++) {
    influences[i] += (visemeTarget[i] - influences[i]) * k;
    if (influences[i] < 0) influences[i] = 0;
    else if (influences[i] > 1) influences[i] = 1;
  }

  if (!reducedMotion) {
    const blinkL = arkitIndex['eyeBlink_L'];
    const blinkR = arkitIndex['eyeBlink_R'];
    if (blinkPhase > 0) {
      blinkPhase += delta;
      const half = BLINK_RAMP / 2;
      let b = blinkPhase < half ? blinkPhase / half : 1 - (blinkPhase - half) / half;
      if (b < 0) b = 0;
      if (blinkPhase >= BLINK_RAMP) { blinkPhase = 0; scheduleNextBlink(t); b = 0; }
      if (blinkL !== undefined) influences[blinkL] = b;
      if (blinkR !== undefined) influences[blinkR] = b;
    } else {
      if (nextBlinkAt === 0) scheduleNextBlink(t);
      if (t >= nextBlinkAt) blinkPhase = 1e-6;
    }

    if (aimTarget) {
      // Eyes lead via real eyeball rotation (eyeLeft/eyeRight nodes); fall back to
      // the eyeLook* morphs only if those eye nodes weren't found.
      driveHeadAim(t, delta, eyeMeshes.length === 0);
      aimEyeballs();
      const breath = Math.sin(t * (2 * Math.PI / BREATH_PERIOD));
      const motionScale = speaking ? 0.4 : 1.0;
      aimTarget.position.y = sceneBaseY + breath * 0.012 * motionScale;
    }
  }
}

// ── Public API ────────────────────────────────────────────────────────────────

window.ironHermesAvatar = {

  init(canvasId, glbUrl, preset) {
    const canvas = document.getElementById(canvasId);
    if (!canvas) {
      console.warn('[ironHermesAvatar] canvas not found:', canvasId);
      return;
    }

    canvas.addEventListener('webglcontextlost', (e) => {
      e.preventDefault();
      signalAvatarError('CONTEXT_LOST', 'WebGL context lost');
    });

    const p = preset || {};
    const seedCamPos = (p.camPos && p.camPos.length === 3) ? p.camPos : [-1.8, 0.8, 3];
    const seedLookAt = (p.lookAt && p.lookAt.length === 3) ? p.lookAt : [0, 0.4, 0];
    // Per-preset lens: narrow fov + pulled-back camera = flat portrait
    // perspective for close face framing (fisheye fix); 45 is the
    // historical default for presets that don't specify one.
    const seedFov = (typeof p.fov === 'number' && p.fov > 0) ? p.fov : 45;
    activeBodyType    = p.bodyType  || null;
    activeVisemeMap   = p.visemeMap || null;
    activeBlinkMorphs = p.blinkMorphs || null;
    activeMaterial = p.material || null;
    activeExpressionMorphs = p.expressionMorphs || null;
    aimScale = 1.0;
    dissolveUniform.value = 0;
    revealT0 = -1;
    pulseT0 = -1;

    const motionPref = resolveToken(canvas, '--orb-motion', 'full');
    reducedMotion = (motionPref === 'none');

    stateColors.idle      = resolveToken(canvas, '--accent-primary', STATE_COLOR_FALLBACKS.idle);
    stateColors.listening = resolveToken(canvas, '--danger',         STATE_COLOR_FALLBACKS.listening);
    stateColors.thinking  = resolveToken(canvas, '--warn',           STATE_COLOR_FALLBACKS.thinking);
    stateColors.speaking  = resolveToken(canvas, '--success',        STATE_COLOR_FALLBACKS.speaking);

    renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    const w = canvas.clientWidth  || canvas.offsetWidth  || 300;
    const h = canvas.clientHeight || canvas.offsetHeight || 300;
    renderer.setSize(w, h, false);
    renderer.setClearColor(0x000000, 0);

    scene = new THREE.Scene();
    clock = new THREE.Clock();

    camera = new THREE.PerspectiveCamera(seedFov, w / h, 0.1, 100);
    camera.position.set(...seedCamPos);
    camera.lookAt(...seedLookAt);

    scene.add(new THREE.AmbientLight(0xffffff, 0.4));
    const initCol = new THREE.Color(stateColors[currentState] || stateColors.idle);
    currentColor.copy(initCol);
    targetColor.copy(initCol);
    keyLight = new THREE.DirectionalLight(initCol, 1.2);
    keyLight.position.set(-1, 1.5, 2);
    scene.add(keyLight);

    // facecap.glb declares KHR_texture_basisu (KTX2) as REQUIRED, so GLTFLoader
    // refuses to parse without a KTX2 loader — even though facecap textures are
    // discarded (MeshNormalMaterial). A real KTX2Loader needs a transcoder served
    // from a stable path, which dx's hashed assets can't provide. Satisfy the
    // loader with a stub returning a valid 1x1 texture (the texels are unused).
    const ktx2Stub = {
      load(_url, onLoad) {
        const tex = new THREE.DataTexture(new Uint8Array([255, 255, 255, 255]), 1, 1);
        tex.needsUpdate = true;
        onLoad(tex);
        return tex;
      },
    };

    new GLTFLoader().setKTX2Loader(ktx2Stub).setMeshoptDecoder(MeshoptDecoder).load(
      glbUrl,
      (gltf) => {
        if (activeVisemeMap) {
          setupRigged(gltf, seedCamPos, seedLookAt);
        } else {
          setupFacecap(gltf, seedCamPos, seedLookAt);
        }

        scene.add(gltf.scene);

        if (reducedMotion && renderer && scene && camera) {
          renderer.render(scene, camera);
        }
      },
      undefined,
      (err) => {
        console.warn('[ironHermesAvatar] GLB load failed:', err);
        signalAvatarError('GLB_LOAD_FAIL', String(err));
      },
    );

    lip = new Lipsync({ fftSize: 2048, historySize: 10 });
    if (!reducedMotion) {
      attachRealtimeStream();
      if (!streamAttached) {
        attachTimer = setInterval(() => { attachRealtimeStream(); }, 500);
      }
    }

    if (typeof ResizeObserver !== 'undefined') {
      const ro = new ResizeObserver(() => onResize(canvas));
      ro.observe(canvas);
    }

    if (reducedMotion) {
      renderer.render(scene, camera);
      return;
    }

    pointerHandler = (ev) => {
      if (gazeMode !== 'manual') return;
      const rect = canvas.getBoundingClientRect();
      const nx = (ev.clientX - (rect.left + rect.width / 2)) / (rect.width * 0.7);
      const ny = (ev.clientY - (rect.top + rect.height / 2)) / (rect.height * 0.7);
      targetYaw   = Math.max(-1, Math.min(1, nx));
      targetPitch = Math.max(-1, Math.min(1, -ny));
      lastMoveAt  = clock ? clock.getElapsedTime() : 0;   // mark activity (idle-return)
    };
    window.addEventListener('mousemove', pointerHandler);

    renderer.setAnimationLoop(animate);
  },

  updateFFT(bins) {
    if (!bins || bins.length === 0) return;
    const len = Math.min(bins.length, 64);
    let maxVal = 0;
    for (let i = 0; i < len; i++) { if (bins[i] > maxVal) maxVal = bins[i]; }
    const scale = (maxVal > 1.0) ? (1.0 / 255.0) : 1.0;
    let sum = 0;
    for (let i = 0; i < len; i++) { fftBins[i] = bins[i] * scale; sum += fftBins[i]; }
    for (let i = len; i < 64; i++) { fftBins[i] = 0; }
    idleBreathAmp = len ? sum / len : 0;
  },

  setState(state) {
    if (currentState === state) return;
    currentState = state;
    const hexColor = stateColors[state] || stateColors.idle;
    targetColor.set(hexColor);
    colorLerp = 0.0;
    if (activeMaterial === 'matrix' && !reducedMotion && clock) {
      pulseT0 = clock.getElapsedTime();
    }
    if (reducedMotion && renderer && scene && camera) {
      currentColor.copy(targetColor);
      if (keyLight) keyLight.color.copy(targetColor);
      applyMatrixTint(targetColor);
      renderer.render(scene, camera);
    }
  },

  setGazeMode(mode) {
    gazeMode = (mode === 'off') ? 'off' : 'manual';
    if (gazeMode !== 'manual') { targetYaw = 0; targetPitch = 0; }
  },

  // One-shot head gesture: 'nod' (yes) or 'shake' (no). Layered on the gaze aim
  // with a sin envelope, so it starts and ends at the current head orientation.
  // No-op under reduced motion or before the rig is ready.
  gesture(kind) {
    if (reducedMotion || !aimTarget) return;
    if (kind !== 'nod' && kind !== 'shake') return;
    gestureKind = kind;
    gestureT0 = clock ? clock.getElapsedTime() : 0;
  },

  destroy() {
    if (renderer) {
      renderer.setAnimationLoop(null);
      try { renderer.forceContextLoss(); } catch (_) {}
      renderer.dispose();
    }
    if (attachTimer !== null) { clearInterval(attachTimer); attachTimer = null; }
    if (pointerHandler) { window.removeEventListener('mousemove', pointerHandler); pointerHandler = null; }
    targetYaw = targetPitch = gazeYaw = gazePitch = 0;
    headYaw = headPitch = headAimYaw = headAimPitch = 0;
    lastMoveAt = -1e9;
    aimTarget = aimBaseWQ = aimParentInvWQ = null;
    gestureKind = null;

    for (const m of [head, teeth, eyeL, eyeR]) {
      if (m && m.geometry) m.geometry.dispose();
      if (m && m.material) m.material.dispose();
    }

    if (lip && lip.audioContext && typeof lip.audioContext.close === 'function') {
      try { lip.audioContext.close(); } catch (_) { /* already closed */ }
    }

    renderer = null;
    scene = null;
    camera = null;
    clock = null;
    keyLight = null;
    head = null;
    teeth = null;
    eyeL = null;
    eyeR = null;
    arkitIndex = null;
    influences = null;
    visemeTarget = null;
    skinnedMode = false;
    morphMeshes = [];
    morphByName = {};
    controlled = [];
    headBone = null;
    eyeMeshes = [];
    eyeBaseWQ = [];
    sceneRoot = null;
    sceneBaseY = 0;
    activeVisemeMap = null;
    activeBlinkMorphs = null;
    activeBodyType = null;
    activeMaterial = null;
    activeExpressionMorphs = null;
    aimScale = 1.0;
    matrixMaterials = [];
    dissolveUniform.value = 0;
    revealT0 = -1;
    pulseT0 = -1;
    lip = null;
    streamAttached = false;
    blinkPhase = 0;
    nextBlinkAt = 0;
    colorLerp = 1.0;
    currentState = 'idle';
    fftBins.fill(0);
  },
};

// ── facecap setup (single morph mesh, ARKit, whole-head-object aim) ────────────
function setupFacecap(gltf, seedCamPos, seedLookAt) {
  const grpNode = gltf.scene.getObjectByName('grp_transform') || gltf.scene;
  head  = grpNode.getObjectByName('mesh_2') || selectMorphMesh(gltf.scene);
  teeth = grpNode.getObjectByName('mesh_3') || null;
  eyeL  = grpNode.getObjectByName('eyeLeft') || null;
  eyeR  = grpNode.getObjectByName('eyeRight') || null;

  if (!head) {
    signalAvatarError('GLB_LOAD_FAIL', 'No morph-bearing mesh found in GLB');
    return;
  }
  head.material = new THREE.MeshNormalMaterial();
  if (teeth) teeth.material = new THREE.MeshNormalMaterial();

  influences = head.morphTargetInfluences;
  visemeTarget = new Float32Array(influences.length);
  arkitIndex = {};
  for (const [k, i] of Object.entries(head.morphTargetDictionary)) {
    arkitIndex[k.replace('blendShape1.', '')] = i;
    arkitIndex[k] = i;
  }

  // Whole-head-object aim (facecap is a head-only model — turning the head
  // assembly IS the head turn). Capture its base world orientation.
  aimTarget = grpNode;
  gltf.scene.updateMatrixWorld(true);
  captureAimBase();
  sceneBaseY = aimTarget ? aimTarget.position.y : 0;

  // Eye gaze: eyeLeft/eyeRight are transform nodes pivoted at the eye centres and
  // already sit under grp_transform, so they turn with the head; aim them
  // in-socket (leading the head) each frame — same mechanism as Groovy. No
  // re-parent needed (Groovy's eyes had to be moved off the scene root; these
  // are already nested correctly).
  eyeMeshes = [eyeL, eyeR].filter(Boolean);
  eyeBaseWQ = [];
  for (const em of eyeMeshes) eyeBaseWQ.push(em.getWorldQuaternion(new THREE.Quaternion()));
  console.log('[ironHermesAvatar] facecap eye nodes for gaze:', eyeMeshes.length);
}

// ── rigged Groovy setup (skeleton + multi-mesh morphs + head.x bone aim) ──────
function setupRigged(gltf, seedCamPos, seedLookAt) {
  skinnedMode = true;
  sceneRoot = gltf.scene;

  // Collect EVERY morph-bearing mesh and index morph names → (mesh, idx). The
  // face, eyelashes and brows are separate meshes that share blendshape names;
  // all must be driven together (single-mesh selection left mouth/eyes frozen).
  morphMeshes = [];
  morphByName = {};
  gltf.scene.traverse((obj) => {
    if (obj.isMesh && obj.morphTargetDictionary && obj.morphTargetInfluences) {
      const mi = morphMeshes.length;
      morphMeshes.push({
        infl: obj.morphTargetInfluences,
        target: new Float32Array(obj.morphTargetInfluences.length),
      });
      for (const [name, idx] of Object.entries(obj.morphTargetDictionary)) {
        (morphByName[name] || (morphByName[name] = [])).push([mi, idx]);
      }
    }
  });

  if (morphMeshes.length === 0) {
    signalAvatarError('GLB_LOAD_FAIL', 'No morph-bearing mesh found in rigged GLB');
    return;
  }

  // Build the controlled set: every viseme morph + both blink morphs, across
  // every mesh that carries them. These are the only morphs we zero+drive each
  // frame (body-shape sliders are left untouched).
  const controlledNames = new Set(Object.values(activeVisemeMap));
  if (activeBlinkMorphs) { controlledNames.add(activeBlinkMorphs[0]); controlledNames.add(activeBlinkMorphs[1]); }
  if (activeMaterial === 'matrix') controlledNames.add('fx_dissolve');
  if (activeExpressionMorphs) {
    for (const n of Object.values(activeExpressionMorphs)) controlledNames.add(n);
  }
  controlled = [];
  for (const name of controlledNames) {
    const list = morphByName[name];
    if (!list) continue;
    for (let i = 0; i < list.length; i++) controlled.push({ mi: list[i][0], idx: list[i][1] });
  }

  if (activeMaterial === 'matrix') {
    // Emissive hologram (black base + emissive code-skin texture): needs no
    // fill light. Track every mesh material so the state color-lerp tints
    // the emissive (idle teal / listening red / thinking amber / speaking
    // green — same palette as the key light).
    try {
      gltf.scene.traverse((o) => {
        if (o.isMesh && o.material && o.material.emissive) {
          setupMatrixMaterial(o.material);
        }
      });
    } catch (e) {
      // Non-fatal: un-tinted white-emissive head still talks (spec §errors).
      signalAvatarError('SHADER_FAIL', String(e));
    }
  } else {
    // Brighten for the textured PBR model (facecap uses MeshNormalMaterial
    // and needs no fill). Hemisphere fill keeps her readable while the
    // state-tinted key light still provides affect.
    scene.add(new THREE.HemisphereLight(0xffffff, 0x404040, 0.7));
    const fill = new THREE.DirectionalLight(0xffffff, 0.35);
    fill.position.set(1.5, 1.0, 1.5);
    scene.add(fill);
  }

  // Head-and-shoulders Box3 framing (full-body model). Seed framing is the
  // fallback if the box is degenerate.
  frameHeadAndShoulders(gltf, seedCamPos, seedLookAt);

  // Head aim target = the `head.x` DEFORM bone (its children include the eye and
  // jaw bones, so eyes turn with the head). Rotating ONLY this bone turns the
  // head while the body stays put.
  //
  // IMPORTANT: GLTFLoader SANITIZES Object3D `.name` (strips '.'/':' and turns
  // spaces into '_'), so the bone's `.name` is NOT 'head.x' — that's why
  // getObjectByName('head.x') found nothing and the head never turned. The
  // ORIGINAL glTF name is preserved in `userData.name`, so match on that.
  gltf.scene.updateMatrixWorld(true);
  headBone = findByGltfName(gltf.scene, 'head.x')
    || findByGltfName(gltf.scene, 'Head')
    || findByGltfName(gltf.scene, 'mixamorig:Head')
    || null;
  console.log('[ironHermesAvatar] head bone:',
    headBone ? (headBone.name + (headBone.isBone ? ' (Bone)' : ' (not a Bone)'))
             : 'NOT FOUND — head turn/idle disabled');
  aimTarget = headBone || null;   // no bone → no head aim (rotating the whole body looks wrong)
  if (!aimTarget && activeBodyType === 'half') {
    // Boneless bust (matrix): whole-object aim, same mechanism as facecap's
    // whole-head aim, at reduced amplitude so the torso turn reads as a
    // statue glancing rather than a neck-less swivel.
    aimTarget = gltf.scene;
    aimScale = 0.6;
  }
  captureAimBase();

  // Eyeball gaze: the eye meshes (Eye_Color_L/R) are STATIC and parented to the
  // scene root — they don't move with the head at all (that's why they read as
  // "stationary"). Re-parent them INTO head.x (preserving world transform via
  // .attach) so they stay seated in the sockets as the head turns, then aim them
  // in-socket toward the cursor each frame (animateRigged). Their pivot is the
  // eyeball centre, so a local rotation spins the eye in place.
  eyeMeshes = [];
  eyeBaseWQ = [];
  if (headBone) {
    for (const en of ['Eye_Color_L_4export', 'Eye_Color_R_4export']) {
      const em = findByGltfName(gltf.scene, en);
      if (em) { headBone.attach(em); eyeMeshes.push(em); }
    }
    gltf.scene.updateMatrixWorld(true);
    for (const em of eyeMeshes) eyeBaseWQ.push(em.getWorldQuaternion(new THREE.Quaternion()));
  }
  console.log('[ironHermesAvatar] eye meshes for gaze:', eyeMeshes.length);

  sceneBaseY = sceneRoot.position.y;

  // Load-in reveal: form out of the code rain (skipped under reduced motion —
  // the model simply appears whole).
  if (activeMaterial === 'matrix' && !reducedMotion && clock) {
    revealT0 = clock.getElapsedTime();
    dissolveUniform.value = 1;
  }
}

// Matrix state tint is MULTIPLICATIVE over the green emissive code texture,
// so a saturated non-green state color (listening red, thinking amber)
// multiplies the texture toward black — live UAT: "the head goes very dark".
// Soften: blend the state color toward white before it reaches the emissive,
// so the head keeps most of its texture brightness and takes a hue CAST
// instead of a full recolor. Non-matrix presets keep the raw color.
const MATRIX_TINT_SOFTEN = 0.65;   // 0 = raw state color, 1 = tint disabled
const matrixTintColor = new THREE.Color();
const MATRIX_TINT_WHITE = new THREE.Color(1, 1, 1);
function applyMatrixTint(color) {
  matrixTintColor.copy(color).lerp(MATRIX_TINT_WHITE, MATRIX_TINT_SOFTEN);
  for (let i = 0; i < matrixMaterials.length; i++) {
    matrixMaterials[i].emissive.copy(matrixTintColor);
  }
}

// Track a matrix-preset emissive material for state tinting (Task 11 adds
// the dissolve injection here). Morph targets require no material opt-in in
// three r184 (morphTargets flags are automatic from geometry).
function setupMatrixMaterial(mat) {
  matrixTintColor.copy(currentColor).lerp(MATRIX_TINT_WHITE, MATRIX_TINT_SOFTEN);
  mat.emissive.copy(matrixTintColor);
  // Screen-space hash dissolve: pixels burn away as uDissolve rises, while
  // the fx_dissolve morph scatters the geometry (same weight, one uniform).
  mat.onBeforeCompile = (shader) => {
    shader.uniforms.uDissolve = dissolveUniform;
    shader.fragmentShader = shader.fragmentShader
      .replace('#include <common>',
        '#include <common>\nuniform float uDissolve;\n' +
        'float ihHash(vec2 p){return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);}')
      .replace('#include <dithering_fragment>',
        '#include <dithering_fragment>\n' +
        'if (ihHash(floor(gl_FragCoord.xy * 0.5)) < uDissolve) discard;');
  };
  mat.needsUpdate = true;
  matrixMaterials.push(mat);
}

// Capture the aim target's base world quaternion + parent-inverse world quat,
// so aimNode() can rotate it in world space regardless of the baked bone frame.
function captureAimBase() {
  if (!aimTarget) { aimBaseWQ = aimParentInvWQ = null; return; }
  aimBaseWQ = aimTarget.getWorldQuaternion(new THREE.Quaternion());
  aimParentInvWQ = aimTarget.parent
    ? aimTarget.parent.getWorldQuaternion(new THREE.Quaternion()).invert()
    : new THREE.Quaternion();
}

// Head-and-shoulders camera crop for a full-body model (uses the real vertical
// FOV so the crop is correct for any lens). Falls back to seed framing on a
// degenerate box.
function frameHeadAndShoulders(gltf, seedCamPos, seedLookAt) {
  if (activeBodyType !== 'full') return;
  try {
    gltf.scene.updateMatrixWorld(true);
    const box = new THREE.Box3().setFromObject(gltf.scene);
    const size = new THREE.Vector3();
    const center = new THREE.Vector3();
    box.getSize(size);
    box.getCenter(center);

    if (size.length() < 0.001) {
      camera.position.set(...seedCamPos);
      camera.lookAt(...seedLookAt);
      return;
    }
    const HEAD_FRAC = 0.20;                       // head ≈ top fifth (stylized = big head)
    const headCenterY = box.max.y - size.y * HEAD_FRAC * 0.5;
    const lookAtVec = new THREE.Vector3(center.x, headCenterY, center.z);
    const frameH = size.y * HEAD_FRAC * 2.0;      // head + some shoulders
    const fovRad = ((camera.fov || 45) * Math.PI) / 180;
    const dist = (frameH * 0.5) / Math.tan(fovRad / 2);
    camera.position.set(center.x, headCenterY, center.z + dist);
    camera.lookAt(lookAtVec);
  } catch (boxErr) {
    console.warn('[ironHermesAvatar] Box3 auto-frame failed, using seed framing:', boxErr);
    camera.position.set(...seedCamPos);
    camera.lookAt(...seedLookAt);
  }
}

// Find a node by its ORIGINAL glTF name. GLTFLoader sanitizes Object3D `.name`
// (strips '.'/':' , spaces → '_') but preserves the raw name in `userData.name`,
// so 'head.x' / 'Mouth Vis Ah' style names must be matched there — NOT via
// getObjectByName('head.x'), which silently returns null.
function findByGltfName(root, gltfName) {
  let found = null;
  root.traverse((o) => {
    if (found) return;
    const orig = o.userData && o.userData.name;
    if (orig === gltfName || o.name === gltfName) found = o;
  });
  return found;
}

// Pick the mesh with the most morph targets (facecap fallback when mesh_2 is
// absent). Kept minimal — the rigged path uses multi-mesh, not this.
function selectMorphMesh(gltfScene) {
  let best = null, bestCount = -1;
  gltfScene.traverse((obj) => {
    if (obj.isMesh && obj.morphTargetDictionary && obj.morphTargetInfluences) {
      const c = Object.keys(obj.morphTargetDictionary).length;
      if (c > bestCount) { bestCount = c; best = obj; }
    }
  });
  return best;
}
