/* ──────────────────────────────────────────────────────────
   IronHermes Floating Menu Wheel
   - 10 wedges mapped to app sections
   - Drag rim  → rotate (snaps to nearest wedge)
   - Drag hub  → translate the whole wheel around the page
   - Click wedge → select; LAUNCH navigates
   ────────────────────────────────────────────────────────── */

(function () {
  const SECTIONS = [
    { id: 'chat',      label: 'CHAT',      sub: 'INTELLIGENCE CONSOLE',   href: 'chat.html',         glyph: '▓' },
    { id: 'agents',    label: 'AGENTS',    sub: 'AUTONOMOUS WORKERS',     href: 'screens/Agents.tsx',    glyph: '◆' },
    { id: 'models',    label: 'MODELS',    sub: 'LANGUAGE CORES',         href: 'screens/Models.tsx',    glyph: '◇' },
    { id: 'tools',     label: 'TOOLS',     sub: 'INSTRUMENT BAY',         href: 'screens/Tools.tsx',     glyph: '◈' },
    { id: 'skills',    label: 'SKILLS',    sub: 'CAPABILITY LATTICE',     href: 'screens/Skills.tsx',    glyph: '✦' },
    { id: 'memory',    label: 'MEMORY',    sub: 'PERSISTENT CONTEXT',     href: 'screens/Memory.tsx',    glyph: '⬢' },
    { id: 'sessions',  label: 'SESSIONS',  sub: 'ACTIVE TRANSCRIPTS',     href: 'screens/Sessions.tsx',  glyph: '▣' },
    { id: 'providers', label: 'PROVIDER',  sub: 'INFERENCE GATEWAYS',     href: 'screens/Providers.tsx', glyph: '◉' },
    { id: 'gateway',   label: 'GATEWAY',   sub: 'NETWORK BRIDGE',         href: 'screens/Gateway.tsx',   glyph: '⌬' },
    { id: 'settings',  label: 'SYSTEM',    sub: 'CONFIGURATION',          href: 'screens/Settings.tsx',  glyph: '⚙' }
  ];

  const N = SECTIONS.length;
  const STEP = 360 / N;
  const SIZE = 360;          // wheel diameter in px
  const R_OUTER = SIZE / 2;
  const R_INNER = 70;        // hub radius
  const R_LABEL = 140;       // label radius
  const R_GLYPH = 110;

  // ── Build SVG ──────────────────────────────────────────
  const root = document.getElementById('wheel-root');
  if (!root) return;

  // Floating frame (draggable as a unit)
  root.innerHTML = `
    <div class="wheel-shell" id="wheel-shell">
      <div class="wheel-indicator"></div>
      <svg class="wheel-svg" viewBox="-${R_OUTER} -${R_OUTER} ${SIZE} ${SIZE}" id="wheel-svg">
        <defs>
          <radialGradient id="hub-grad" cx="40%" cy="40%" r="60%">
            <stop offset="0%"  stop-color="#1c2530"/>
            <stop offset="100%" stop-color="#0a0e14"/>
          </radialGradient>
          <linearGradient id="rim-grad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%"   stop-color="rgba(57,197,207,0.45)"/>
            <stop offset="100%" stop-color="rgba(57,197,207,0.08)"/>
          </linearGradient>
        </defs>

        <!-- Outer rim ring (decorative) -->
        <circle cx="0" cy="0" r="${R_OUTER - 2}"  fill="none" stroke="rgba(57,197,207,0.15)" stroke-width="1"/>
        <circle cx="0" cy="0" r="${R_OUTER - 12}" fill="none" stroke="rgba(57,197,207,0.10)" stroke-width="1" stroke-dasharray="2 4"/>

        <!-- Tick marks -->
        <g id="wheel-ticks"></g>

        <!-- Rotating group -->
        <g id="wheel-rot">
          <!-- Wedges -->
          <g id="wheel-wedges"></g>
          <!-- Wedge separators -->
          <g id="wheel-seps"></g>
          <!-- Glyphs + labels -->
          <g id="wheel-text"></g>
          <!-- Inner ring -->
          <circle cx="0" cy="0" r="${R_INNER + 6}" fill="none" stroke="rgba(57,197,207,0.18)" stroke-width="1"/>
        </g>

        <!-- Hub (non-rotating) -->
        <g id="wheel-hub">
          <circle cx="0" cy="0" r="${R_INNER}" fill="url(#hub-grad)" stroke="rgba(57,197,207,0.45)" stroke-width="1"/>
          <circle cx="0" cy="0" r="${R_INNER - 6}" fill="none" stroke="rgba(57,197,207,0.18)"/>
          <text id="hub-label" x="0" y="-4" text-anchor="middle"
                font-family="JetBrains Mono, monospace"
                font-size="14" font-weight="700"
                fill="#56d4dd" letter-spacing="2">${SECTIONS[0].label}</text>
          <text id="hub-sub" x="0" y="14" text-anchor="middle"
                font-family="JetBrains Mono, monospace"
                font-size="6.5" fill="#6e7681" letter-spacing="2.5">${SECTIONS[0].sub}</text>
          <text id="hub-idx" x="0" y="30" text-anchor="middle"
                font-family="JetBrains Mono, monospace"
                font-size="7" fill="#39c5cf" letter-spacing="3">01 / ${String(N).padStart(2,'0')}</text>
        </g>

        <!-- Top pointer (arrow on hub) -->
        <polygon points="0,${-(R_INNER + 10)} -5,${-(R_INNER + 22)} 5,${-(R_INNER + 22)}"
                 fill="#39c5cf" />
      </svg>

      <div class="wheel-launch">
        <button class="btn wheel-launch-btn" id="wheel-launch-btn">
          <span class="wheel-launch-glyph">▶</span>
          <span>LAUNCH</span>
          <span class="wheel-launch-target" id="wheel-launch-target">CHAT</span>
        </button>
        <div class="wheel-hint" id="wheel-hint">
          <span>↻ DRAG RIM TO ROTATE</span>
          <span>✥ DRAG HUB TO MOVE</span>
        </div>
      </div>
    </div>
  `;

  const shell    = document.getElementById('wheel-shell');
  const svg      = document.getElementById('wheel-svg');
  const rot      = document.getElementById('wheel-rot');
  const wedgesEl = document.getElementById('wheel-wedges');
  const sepsEl   = document.getElementById('wheel-seps');
  const textEl   = document.getElementById('wheel-text');
  const ticksEl  = document.getElementById('wheel-ticks');
  const hubLbl   = document.getElementById('hub-label');
  const hubSub   = document.getElementById('hub-sub');
  const hubIdx   = document.getElementById('hub-idx');
  const launchT  = document.getElementById('wheel-launch-target');
  const launchBtn= document.getElementById('wheel-launch-btn');
  const indicator= shell.querySelector('.wheel-indicator');
  const hint     = document.getElementById('wheel-hint');

  // ── Geometry helpers ───────────────────────────────────
  function polar(angDeg, r) {
    const a = (angDeg - 90) * Math.PI / 180;
    return [Math.cos(a) * r, Math.sin(a) * r];
  }
  function wedgePath(angA, angB, rInner, rOuter) {
    const [x1, y1] = polar(angA, rOuter);
    const [x2, y2] = polar(angB, rOuter);
    const [x3, y3] = polar(angB, rInner);
    const [x4, y4] = polar(angA, rInner);
    return `M ${x1} ${y1} A ${rOuter} ${rOuter} 0 0 1 ${x2} ${y2} L ${x3} ${y3} A ${rInner} ${rInner} 0 0 0 ${x4} ${y4} Z`;
  }

  // ── Tick marks (non-rotating) ──────────────────────────
  for (let i = 0; i < 60; i++) {
    const a = (i * 6 - 90) * Math.PI / 180;
    const r1 = R_OUTER - 14;
    const r2 = R_OUTER - (i % 5 === 0 ? 22 : 18);
    const x1 = Math.cos(a) * r1, y1 = Math.sin(a) * r1;
    const x2 = Math.cos(a) * r2, y2 = Math.sin(a) * r2;
    const ln = document.createElementNS('http://www.w3.org/2000/svg', 'line');
    ln.setAttribute('x1', x1); ln.setAttribute('y1', y1);
    ln.setAttribute('x2', x2); ln.setAttribute('y2', y2);
    ln.setAttribute('stroke', i % 5 === 0 ? 'rgba(57,197,207,0.55)' : 'rgba(110,118,129,0.35)');
    ln.setAttribute('stroke-width', '1');
    ticksEl.appendChild(ln);
  }

  // ── Wedges ─────────────────────────────────────────────
  SECTIONS.forEach((s, i) => {
    const angA = i * STEP - STEP / 2;
    const angB = (i + 1) * STEP - STEP / 2;
    const mid  = i * STEP;

    // Wedge path
    const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    p.setAttribute('d', wedgePath(angA, angB, R_INNER + 8, R_OUTER - 24));
    p.setAttribute('class', 'wheel-wedge');
    p.setAttribute('data-i', i);
    wedgesEl.appendChild(p);

    // Separator
    const [sx, sy] = polar(angA, R_INNER + 8);
    const [ex, ey] = polar(angA, R_OUTER - 24);
    const sep = document.createElementNS('http://www.w3.org/2000/svg', 'line');
    sep.setAttribute('x1', sx); sep.setAttribute('y1', sy);
    sep.setAttribute('x2', ex); sep.setAttribute('y2', ey);
    sep.setAttribute('stroke', 'rgba(57,197,207,0.20)');
    sep.setAttribute('stroke-width', '1');
    sepsEl.appendChild(sep);

    // Glyph at top of wedge
    const [gx, gy] = polar(mid, R_GLYPH);
    const g = document.createElementNS('http://www.w3.org/2000/svg', 'text');
    g.setAttribute('x', gx); g.setAttribute('y', gy + 4);
    g.setAttribute('text-anchor', 'middle');
    g.setAttribute('font-size', '14');
    g.setAttribute('fill', 'rgba(57,197,207,0.65)');
    g.setAttribute('class', 'wheel-glyph');
    g.setAttribute('data-i', i);
    g.setAttribute('transform', `rotate(${mid} ${gx} ${gy})`);
    g.textContent = s.glyph;
    textEl.appendChild(g);

    // Label further out, rotated so it reads radially
    const [lx, ly] = polar(mid, R_LABEL);
    const lbl = document.createElementNS('http://www.w3.org/2000/svg', 'text');
    lbl.setAttribute('x', lx); lbl.setAttribute('y', ly + 3);
    lbl.setAttribute('text-anchor', 'middle');
    lbl.setAttribute('font-family', 'JetBrains Mono, monospace');
    lbl.setAttribute('font-size', '8.5');
    lbl.setAttribute('font-weight', '700');
    lbl.setAttribute('letter-spacing', '2');
    lbl.setAttribute('fill', '#9aa4ad');
    lbl.setAttribute('class', 'wheel-label');
    lbl.setAttribute('data-i', i);
    lbl.setAttribute('transform', `rotate(${mid} ${lx} ${ly})`);
    lbl.textContent = s.label;
    textEl.appendChild(lbl);
  });

  // ── State ──────────────────────────────────────────────
  // We rotate the inner group so the SELECTED wedge sits at TOP (angle 0).
  // i.e. group rotation = -mid(i)
  let selected = 0;
  let rotation = 0;     // degrees applied to inner group
  let target   = 0;     // target rotation (animated towards)

  function angleOfWedge(i) { return -i * STEP; }

  function setSelected(i, animate = true) {
    selected = ((i % N) + N) % N;
    target   = angleOfWedge(selected);
    if (!animate) rotation = target;
    updateUI();
  }

  function updateUI() {
    const s = SECTIONS[selected];
    hubLbl.textContent = s.label;
    hubSub.textContent = s.sub;
    hubIdx.textContent = String(selected + 1).padStart(2, '0') + ' / ' + String(N).padStart(2, '0');
    launchT.textContent = '→ ' + s.label;
    // Highlight selected wedge
    wedgesEl.querySelectorAll('.wheel-wedge').forEach((el, i) => {
      el.classList.toggle('is-selected', i === selected);
    });
    textEl.querySelectorAll('.wheel-glyph,.wheel-label').forEach(el => {
      el.classList.toggle('is-selected', +el.getAttribute('data-i') === selected);
    });
  }

  // ── Animation loop ─────────────────────────────────────
  function tick() {
    const diff = ((target - rotation + 540) % 360) - 180;
    rotation += diff * 0.22;
    if (Math.abs(diff) < 0.02) rotation = target;
    rot.setAttribute('transform', `rotate(${rotation})`);
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);

  setSelected(0, false);

  // ── Click wedge to select ──────────────────────────────
  wedgesEl.addEventListener('click', (e) => {
    const w = e.target.closest('.wheel-wedge');
    if (!w) return;
    const i = +w.getAttribute('data-i');
    setSelected(i);
  });

  // ── Drag rim → rotate ──────────────────────────────────
  let rotDragging = false;
  let rotStartAng = 0;
  let rotStartRot = 0;

  function svgCenter() {
    const r = svg.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  }
  function pointerAngleDeg(e) {
    const c = svgCenter();
    return Math.atan2(e.clientY - c.y, e.clientX - c.x) * 180 / Math.PI + 90;
  }
  function pointerRadius(e) {
    const c = svgCenter();
    const dx = e.clientX - c.x, dy = e.clientY - c.y;
    // SVG view is SIZE wide; need to scale to viewBox
    const rect = svg.getBoundingClientRect();
    return Math.sqrt(dx*dx + dy*dy) * (SIZE / rect.width);
  }

  svg.addEventListener('pointerdown', (e) => {
    const r = pointerRadius(e);
    if (r < R_INNER) return; // hub handled separately
    rotDragging = true;
    rotStartAng = pointerAngleDeg(e);
    rotStartRot = rotation;
    svg.setPointerCapture(e.pointerId);
    hint && (hint.style.opacity = '0');
  });
  svg.addEventListener('pointermove', (e) => {
    if (!rotDragging) return;
    const cur = pointerAngleDeg(e);
    let next = rotStartRot + (cur - rotStartAng);
    rotation = next; target = next;
    rot.setAttribute('transform', `rotate(${rotation})`);
  });
  svg.addEventListener('pointerup', (e) => {
    if (!rotDragging) return;
    rotDragging = false;
    svg.releasePointerCapture(e.pointerId);
    // Snap to nearest wedge: pick wedge whose mid sits at angle 0 (top)
    // mid(i) = i*STEP ; group rot R means visible angle = mid + R; we want closest to 0
    const norm = ((rotation % 360) + 360) % 360;
    const k = Math.round(((360 - norm) % 360) / STEP) % N;
    setSelected(k);
  });

  // ── Drag hub → move the whole shell ────────────────────
  let posX = null, posY = null;     // top-left in px
  let moveDragging = false;
  let moveStart = null;

  // Initial position: pinned in viewport (set in CSS), but read it once
  function ensurePos() {
    if (posX !== null) return;
    const r = shell.getBoundingClientRect();
    posX = r.left; posY = r.top;
    shell.style.left = posX + 'px';
    shell.style.top  = posY + 'px';
    shell.style.right = 'auto';
    shell.style.bottom = 'auto';
  }

  shell.addEventListener('pointerdown', (e) => {
    const r = pointerRadius(e);
    if (r >= R_INNER) return; // rotate handler claims it
    ensurePos();
    moveDragging = true;
    moveStart = { x: e.clientX, y: e.clientY, px: posX, py: posY };
    shell.setPointerCapture(e.pointerId);
    shell.classList.add('is-moving');
    e.preventDefault();
  });
  shell.addEventListener('pointermove', (e) => {
    if (!moveDragging) return;
    const dx = e.clientX - moveStart.x;
    const dy = e.clientY - moveStart.y;
    posX = moveStart.px + dx;
    posY = moveStart.py + dy;
    // Clamp to viewport
    const margin = 20;
    const w = shell.offsetWidth, h = shell.offsetHeight;
    posX = Math.max(margin, Math.min(window.innerWidth  - w - margin, posX));
    posY = Math.max(margin, Math.min(window.innerHeight - h - margin, posY));
    shell.style.left = posX + 'px';
    shell.style.top  = posY + 'px';
  });
  shell.addEventListener('pointerup', (e) => {
    if (!moveDragging) return;
    moveDragging = false;
    shell.classList.remove('is-moving');
    shell.releasePointerCapture(e.pointerId);
  });

  // ── Keyboard nav (arrow keys) ──────────────────────────
  window.addEventListener('keydown', (e) => {
    if (e.target && /input|textarea/i.test(e.target.tagName)) return;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { setSelected(selected + 1); e.preventDefault(); }
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { setSelected(selected - 1); e.preventDefault(); }
    else if (e.key === 'Enter') { launchBtn.click(); }
  });

  // ── Launch ─────────────────────────────────────────────
  launchBtn.addEventListener('click', () => {
    const s = SECTIONS[selected];
    if (s.href.endsWith('.tsx')) {
      // .tsx files aren't runnable in the browser — show a stub overlay
      showStub(s);
    } else {
      window.location.href = s.href;
    }
  });

  function showStub(s) {
    let stub = document.getElementById('stub-overlay');
    if (!stub) {
      stub = document.createElement('div');
      stub.id = 'stub-overlay';
      stub.className = 'stub-overlay';
      document.body.appendChild(stub);
    }
    stub.innerHTML = `
      <div class="stub-card">
        <div class="stub-header">
          <span class="stub-tag">// MODULE</span>
          <button class="stub-close" id="stub-close">×</button>
        </div>
        <div class="stub-title">${s.label}</div>
        <div class="stub-sub">${s.sub}</div>
        <div class="stub-body">
          <div class="stub-row"><span>SOURCE</span><span>screens/${s.label === 'SYSTEM' ? 'Settings' : titleCase(s.id)}.tsx</span></div>
          <div class="stub-row"><span>RUNTIME</span><span>React + Hermes API</span></div>
          <div class="stub-row"><span>STATUS</span><span class="ok">READY</span></div>
        </div>
        <div class="stub-note">
          This module ships as a separate React component
          (<span class="ih-mono">.tsx</span>) and runs inside the IronHermes
          desktop shell. The console below is a preview of the live interface.
        </div>
        <div class="stub-actions">
          <a class="btn" href="screens/${titleCase(s.id)}.tsx" target="_blank">VIEW SOURCE</a>
          <button class="btn btn--ghost" id="stub-back">RETURN TO BRIDGE</button>
        </div>
      </div>
    `;
    stub.style.display = 'flex';
    stub.querySelector('#stub-close').onclick = closeStub;
    stub.querySelector('#stub-back').onclick  = closeStub;
    function closeStub() { stub.style.display = 'none'; }
  }

  function titleCase(s) {
    if (s === 'providers') return 'Providers';
    if (s === 'settings')  return 'Settings';
    return s.charAt(0).toUpperCase() + s.slice(1);
  }

  // Hide hint after first interaction
  let interactions = 0;
  shell.addEventListener('pointerdown', () => {
    interactions++;
    if (interactions >= 1) {
      setTimeout(() => { if (hint) hint.style.opacity = '0.4'; }, 1500);
    }
  });

  // Expose for hero buttons
  window.IH_Wheel = {
    select: setSelected,
    sections: SECTIONS,
  };
})();
