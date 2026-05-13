/* ──────────────────────────────────────────────────────────
   IronHermes Floating Menu Wheel — V2.1
   - Hover wedge   → activate
   - Click hub     → launch active
   - Click wedge   → launch that wedge directly
   - Drag the RIM  → translate wheel (tooltip on hover)
   - Drag the SE handle → resize the wheel
   ────────────────────────────────────────────────────────── */

(function () {
  const DEFAULT_SECTIONS = [
    { id: 'chat',      label: 'CHAT',      sub: 'INTELLIGENCE CONSOLE', href: 'chat.html',             glyph: '▓' },
    { id: 'agents',    label: 'AGENTS',    sub: 'AUTONOMOUS WORKERS',   href: 'screens/Agents.tsx',    glyph: '◆' },
    { id: 'models',    label: 'MODELS',    sub: 'LANGUAGE CORES',       href: 'screens/Models.tsx',    glyph: '◇' },
    { id: 'tools',     label: 'TOOLS',     sub: 'INSTRUMENT BAY',       href: 'screens/Tools.tsx',     glyph: '◈' },
    { id: 'skills',    label: 'SKILLS',    sub: 'CAPABILITY LATTICE',   href: 'screens/Skills.tsx',    glyph: '✦' },
    { id: 'memory',    label: 'MEMORY',    sub: 'PERSISTENT CONTEXT',   href: 'screens/Memory.tsx',    glyph: '⬢' },
    { id: 'sessions',  label: 'SESSIONS',  sub: 'ACTIVE TRANSCRIPTS',   href: 'screens/Sessions.tsx',  glyph: '▣' },
    { id: 'providers', label: 'PROVIDER',  sub: 'INFERENCE GATEWAYS',   href: 'screens/Providers.tsx', glyph: '◉' },
    { id: 'gateway',   label: 'GATEWAY',   sub: 'NETWORK BRIDGE',       href: 'screens/Gateway.tsx',   glyph: '⌬' },
    { id: 'settings',  label: 'SYSTEM',    sub: 'CONFIGURATION',        href: 'screens/Settings.tsx',  glyph: '⚙' }
  ];
  const SECTIONS = window.WHEEL_SECTIONS || DEFAULT_SECTIONS;
  const onLaunchOverride = window.WHEEL_ON_LAUNCH || null;

  const N = SECTIONS.length;
  const STEP = 360 / N;
  // Internal SVG coordinate system — kept constant; visible size is CSS.
  const SIZE = 380;
  const R_OUTER = SIZE / 2;          // wheel rim radius
  const R_INNER = 78;
  const R_LABEL = 148;
  const R_GLYPH = 118;
  const RING_GAP = 14;               // gap between rim and resize ring
  const RING_W = 4;                  // resize ring stroke width
  const PAD = 22;                    // extra svg viewBox padding (must clear ring)
  const VB = SIZE + PAD * 2;         // full viewBox size

  const root = document.getElementById('wheel-root');
  if (!root) return;

  root.innerHTML = `
    <div class="wheel-shell" id="wheel-shell">
      <svg class="wheel-svg" viewBox="-${R_OUTER + PAD} -${R_OUTER + PAD} ${VB} ${VB}" id="wheel-svg">
        <defs>
          <radialGradient id="hub-grad" cx="40%" cy="40%" r="70%">
            <stop offset="0%"  stop-color="#1f2934"/>
            <stop offset="55%" stop-color="#121820"/>
            <stop offset="100%" stop-color="#070b10"/>
          </radialGradient>
          <radialGradient id="hub-grad-hover" cx="40%" cy="40%" r="70%">
            <stop offset="0%"  stop-color="#1d3a44"/>
            <stop offset="55%" stop-color="#0c2329"/>
            <stop offset="100%" stop-color="#06141a"/>
          </radialGradient>
        </defs>

        <!-- ─── RIM (drag-to-move hit area + decoration) ─── -->
        <!-- Two decorative rings -->
        <circle cx="0" cy="0" r="${R_OUTER - 2}"  class="rim-ring rim-ring--outer" fill="none" stroke-width="1"/>
        <circle cx="0" cy="0" r="${R_OUTER - 12}" class="rim-ring rim-ring--inner" fill="none" stroke-width="1" stroke-dasharray="2 4"/>

        <!-- Ticks (non-interactive, drawn inside the rim hit area) -->
        <g id="wheel-ticks" pointer-events="none"></g>

        <!-- Invisible hit-area annulus that captures rim drag.
             Drawn AFTER the ticks so it sits on top; transparent fill catches events. -->
        <path id="wheel-rim" class="wheel-rim"
              fill="rgba(57,197,207,0.001)"
              fill-rule="evenodd"
              d="M ${R_OUTER - 1},0
                 A ${R_OUTER - 1},${R_OUTER - 1} 0 1 1 ${-(R_OUTER - 1)},0
                 A ${R_OUTER - 1},${R_OUTER - 1} 0 1 1 ${R_OUTER - 1},0 Z
                 M ${R_OUTER - 28},0
                 A ${R_OUTER - 28},${R_OUTER - 28} 0 1 0 ${-(R_OUTER - 28)},0
                 A ${R_OUTER - 28},${R_OUTER - 28} 0 1 0 ${R_OUTER - 28},0 Z"/>

        <!-- Wedges & labels -->
        <g id="wheel-wedges"></g>
        <g id="wheel-seps" pointer-events="none"></g>
        <g id="wheel-text" pointer-events="none"></g>

        <!-- Inner divider ring -->
        <circle cx="0" cy="0" r="${R_INNER + 6}" fill="none" stroke="rgba(57,197,207,0.22)" stroke-width="1" pointer-events="none"/>

        <!-- HUB (launch button) -->
        <g id="wheel-hub" class="wheel-hub">
          <circle id="hub-bg" cx="0" cy="0" r="${R_INNER}" fill="url(#hub-grad)"
                  stroke="rgba(57,197,207,0.55)" stroke-width="1"/>
          <circle cx="0" cy="0" r="${R_INNER - 6}" fill="none" stroke="rgba(57,197,207,0.18)"/>
          <circle cx="0" cy="0" r="${R_INNER - 14}" fill="none" stroke="rgba(57,197,207,0.10)" stroke-dasharray="2 3"/>
          <circle id="hub-aura" cx="0" cy="0" r="${R_INNER - 4}" fill="rgba(57,197,207,0.0)"/>

          <text id="hub-glyph" x="0" y="-22" text-anchor="middle"
                font-family="JetBrains Mono, monospace"
                font-size="16" fill="#56d4dd">▶</text>
          <text id="hub-label" x="0" y="-2" text-anchor="middle"
                font-family="JetBrains Mono, monospace"
                font-size="13" font-weight="700"
                fill="#e6edf3" letter-spacing="2.5">${SECTIONS[0].label}</text>
          <text id="hub-sub" x="0" y="14" text-anchor="middle"
                font-family="JetBrains Mono, monospace"
                font-size="6.5" fill="#6e7681" letter-spacing="3">${SECTIONS[0].sub}</text>
          <text id="hub-cta" x="0" y="30" text-anchor="middle"
                font-family="JetBrains Mono, monospace"
                font-size="7" font-weight="700"
                fill="#39c5cf" letter-spacing="3">▸ LAUNCH</text>
        </g>

        <polygon points="0,${-(R_INNER + 12)} -6,${-(R_INNER + 24)} 6,${-(R_INNER + 24)}"
                 fill="#39c5cf" id="hub-arrow" pointer-events="none"/>

        <!-- ─── Floating RESIZE RING (outside drag rim) ─── -->
        <!-- Soft outer glow (purely decorative) -->
        <circle id="resize-glow" cx="0" cy="0" r="${R_OUTER + RING_GAP}"
                fill="none" stroke="rgba(255,166,87,0.0)" stroke-width="${RING_W + 6}"
                pointer-events="none"/>
        <!-- Dashed orbit ticks (decorative) -->
        <circle id="resize-orbit" cx="0" cy="0" r="${R_OUTER + RING_GAP + 7}"
                fill="none" stroke="rgba(255,166,87,0.30)" stroke-width="1"
                stroke-dasharray="1 6" pointer-events="none"/>
        <!-- Hit ring (the actual drag-to-resize surface) -->
        <circle id="resize-ring" class="resize-ring"
                cx="0" cy="0" r="${R_OUTER + RING_GAP}"
                fill="none" stroke="rgba(255,166,87,0.55)"
                stroke-width="${RING_W}"
                pointer-events="stroke"/>
        <!-- Four orbit nodes that pulse on hover, for affordance -->
        <g id="resize-nodes" pointer-events="none">
          <circle cx="${R_OUTER + RING_GAP}"  cy="0" r="3" fill="#ffa657"/>
          <circle cx="${-(R_OUTER + RING_GAP)}" cy="0" r="3" fill="#ffa657"/>
          <circle cx="0" cy="${R_OUTER + RING_GAP}"  r="3" fill="#ffa657"/>
          <circle cx="0" cy="${-(R_OUTER + RING_GAP)}" r="3" fill="#ffa657"/>
        </g>
      </svg>

      <!-- Floating tooltip -->
      <div class="wheel-tooltip" id="wheel-tooltip" aria-hidden="true">
        <span class="wheel-tooltip-glyph">✥</span>
        <span class="wheel-tooltip-label">DRAG TO MOVE</span>
      </div>
    </div>
  `;

  const shell    = document.getElementById('wheel-shell');
  const svg      = document.getElementById('wheel-svg');
  const wedgesEl = document.getElementById('wheel-wedges');
  const sepsEl   = document.getElementById('wheel-seps');
  const textEl   = document.getElementById('wheel-text');
  const ticksEl  = document.getElementById('wheel-ticks');
  const rimEl    = document.getElementById('wheel-rim');
  const hub      = document.getElementById('wheel-hub');
  const hubBg    = document.getElementById('hub-bg');
  const hubLbl   = document.getElementById('hub-label');
  const hubSub   = document.getElementById('hub-sub');
  const hubCta   = document.getElementById('hub-cta');
  const resizeEl = document.getElementById('resize-ring');
  const tooltip  = document.getElementById('wheel-tooltip');
  const tooltipLabel = tooltip.querySelector('.wheel-tooltip-label');
  const tooltipGlyph = tooltip.querySelector('.wheel-tooltip-glyph');

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

  // ── Tick marks ─────────────────────────────────────────
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

    const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    p.setAttribute('d', wedgePath(angA, angB, R_INNER + 8, R_OUTER - 30));
    p.setAttribute('class', 'wheel-wedge');
    p.setAttribute('data-i', i);
    wedgesEl.appendChild(p);

    const [sx, sy] = polar(angA, R_INNER + 8);
    const [ex, ey] = polar(angA, R_OUTER - 30);
    const sep = document.createElementNS('http://www.w3.org/2000/svg', 'line');
    sep.setAttribute('x1', sx); sep.setAttribute('y1', sy);
    sep.setAttribute('x2', ex); sep.setAttribute('y2', ey);
    sep.setAttribute('stroke', 'rgba(57,197,207,0.22)');
    sep.setAttribute('stroke-width', '1');
    sepsEl.appendChild(sep);

    const [gx, gy] = polar(mid, R_GLYPH);
    const g = document.createElementNS('http://www.w3.org/2000/svg', 'text');
    g.setAttribute('x', gx); g.setAttribute('y', gy + 5);
    g.setAttribute('text-anchor', 'middle');
    g.setAttribute('font-size', '15');
    g.setAttribute('fill', 'rgba(57,197,207,0.65)');
    g.setAttribute('class', 'wheel-glyph');
    g.setAttribute('data-i', i);
    g.setAttribute('transform', `rotate(${mid} ${gx} ${gy})`);
    g.textContent = s.glyph;
    textEl.appendChild(g);

    const [lx, ly] = polar(mid, R_LABEL);
    const lbl = document.createElementNS('http://www.w3.org/2000/svg', 'text');
    lbl.setAttribute('x', lx); lbl.setAttribute('y', ly + 3);
    lbl.setAttribute('text-anchor', 'middle');
    lbl.setAttribute('font-family', 'JetBrains Mono, monospace');
    lbl.setAttribute('font-size', '9');
    lbl.setAttribute('font-weight', '700');
    lbl.setAttribute('letter-spacing', '2');
    lbl.setAttribute('fill', '#9aa4ad');
    lbl.setAttribute('class', 'wheel-label');
    lbl.setAttribute('data-i', i);
    lbl.setAttribute('transform', `rotate(${mid} ${lx} ${ly})`);
    lbl.textContent = s.label;
    textEl.appendChild(lbl);
  });

  // ── Active section state ───────────────────────────────
  let active = 0;
  function setActive(i) {
    active = ((i % N) + N) % N;
    const s = SECTIONS[active];
    hubLbl.textContent = s.label;
    hubSub.textContent = s.sub;
    hubCta.textContent = '▸ LAUNCH';
    wedgesEl.querySelectorAll('.wheel-wedge').forEach((el, j) => {
      el.classList.toggle('is-active', j === active);
    });
    textEl.querySelectorAll('.wheel-glyph,.wheel-label').forEach(el => {
      el.classList.toggle('is-active', +el.getAttribute('data-i') === active);
    });
  }
  setActive(0);

  // ── Wedge hover / click ────────────────────────────────
  wedgesEl.addEventListener('mouseover', (e) => {
    const w = e.target.closest('.wheel-wedge');
    if (!w) return;
    setActive(+w.getAttribute('data-i'));
    hideTooltip();
  });
  wedgesEl.addEventListener('click', (e) => {
    const w = e.target.closest('.wheel-wedge');
    if (!w) return;
    setActive(+w.getAttribute('data-i'));
    launch();
  });

  // ── Hub launch ─────────────────────────────────────────
  hub.addEventListener('click', () => { if (!suppressClick) launch(); });
  hub.addEventListener('mouseenter', () => {
    hub.classList.add('is-hover');
    hubBg.setAttribute('fill', 'url(#hub-grad-hover)');
    hideTooltip();
  });
  hub.addEventListener('mouseleave', () => {
    hub.classList.remove('is-hover');
    hubBg.setAttribute('fill', 'url(#hub-grad)');
  });

  function launch() {
    const s = SECTIONS[active];
    if (onLaunchOverride) { onLaunchOverride(s, active); return; }
    if (s.href.endsWith('.tsx')) showStub(s);
    else window.location.href = s.href;
  }

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
          <div class="stub-row"><span>SOURCE</span><span>screens/${titleCase(s.id)}.tsx</span></div>
          <div class="stub-row"><span>RUNTIME</span><span>React + Hermes API</span></div>
          <div class="stub-row"><span>STATUS</span><span class="ok">READY</span></div>
        </div>
        <div class="stub-note">
          This module ships as a separate React component
          (<span class="ih-mono">.tsx</span>) and runs inside the IronHermes desktop shell.
        </div>
        <div class="stub-actions">
          <a class="btn" href="screens/${titleCase(s.id)}.tsx" target="_blank">VIEW SOURCE</a>
          <button class="btn btn--ghost" id="stub-back">RETURN TO BRIDGE</button>
        </div>
      </div>
    `;
    stub.style.display = 'flex';
    stub.querySelector('#stub-close').onclick = () => stub.style.display = 'none';
    stub.querySelector('#stub-back').onclick  = () => stub.style.display = 'none';
  }
  function titleCase(s) {
    if (s === 'providers') return 'Providers';
    if (s === 'settings')  return 'Settings';
    return s.charAt(0).toUpperCase() + s.slice(1);
  }

  // ── Position state ─────────────────────────────────────
  let posX = null, posY = null;
  function ensurePos() {
    if (posX !== null) return;
    const r = shell.getBoundingClientRect();
    posX = r.left; posY = r.top;
    shell.style.left = posX + 'px';
    shell.style.top  = posY + 'px';
    shell.style.right = 'auto';
    shell.style.bottom = 'auto';
  }

  // ── Tooltip (shared by rim + resize ring) ──────────────
  let tooltipVisible = false;
  function showTooltip(e, mode) {
    tooltipVisible = true;
    if (mode === 'resize') {
      tooltip.classList.add('is-visible', 'is-resize');
      tooltipLabel.textContent = 'DRAG TO RESIZE';
      tooltipGlyph.textContent = '⤡';
    } else {
      tooltip.classList.add('is-visible');
      tooltip.classList.remove('is-resize');
      tooltipLabel.textContent = 'DRAG TO MOVE';
      tooltipGlyph.textContent = '✥';
    }
    moveTooltip(e);
  }
  function hideTooltip() {
    if (!tooltipVisible) return;
    tooltipVisible = false;
    tooltip.classList.remove('is-visible', 'is-resize');
  }
  function moveTooltip(e) {
    if (!tooltipVisible) return;
    const shellRect = shell.getBoundingClientRect();
    let x = e.clientX - shellRect.left + 14;
    let y = e.clientY - shellRect.top  + 14;
    tooltip.style.transform = `translate(${x}px, ${y}px)`;
  }

  rimEl.addEventListener('mouseenter', (e) => showTooltip(e, 'move'));
  rimEl.addEventListener('mousemove', moveTooltip);
  rimEl.addEventListener('mouseleave', hideTooltip);

  resizeEl.addEventListener('mouseenter', (e) => {
    showTooltip(e, 'resize');
    shell.classList.add('is-resize-hover');
  });
  resizeEl.addEventListener('mousemove', moveTooltip);
  resizeEl.addEventListener('mouseleave', () => {
    hideTooltip();
    shell.classList.remove('is-resize-hover');
  });

  // ── Drag the RIM to translate the wheel ────────────────
  let moving = false;
  let moveStart = null;
  let suppressClick = false;

  rimEl.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    ensurePos();
    moving = true;
    suppressClick = false;
    moveStart = { x: e.clientX, y: e.clientY, px: posX, py: posY, dist: 0 };
    rimEl.setPointerCapture(e.pointerId);
    shell.classList.add('is-moving');
    hideTooltip();
    e.preventDefault();
  });

  window.addEventListener('pointermove', (e) => {
    if (!moving) return;
    const dx = e.clientX - moveStart.x;
    const dy = e.clientY - moveStart.y;
    moveStart.dist = Math.max(moveStart.dist, Math.hypot(dx, dy));
    posX = moveStart.px + dx;
    posY = moveStart.py + dy;
    const margin = 12;
    const w = shell.offsetWidth, h = shell.offsetHeight;
    posX = Math.max(margin, Math.min(window.innerWidth  - w - margin, posX));
    posY = Math.max(margin, Math.min(window.innerHeight - h - margin, posY));
    shell.style.left = posX + 'px';
    shell.style.top  = posY + 'px';
  });

  window.addEventListener('pointerup', () => {
    if (!moving) return;
    moving = false;
    shell.classList.remove('is-moving');
    if (moveStart && moveStart.dist > 4) suppressClick = true;
    setTimeout(() => { suppressClick = false; }, 50);
  });

  // ── Resize handle (SE corner) ──────────────────────────
  const MIN_SIZE = 240;
  const MAX_SIZE = 640;
  let resizing = false;
  let resizeStart = null;

  function readSize() {
    const v = getComputedStyle(shell).getPropertyValue('--wheel-size').trim();
    const n = parseFloat(v);
    return isFinite(n) && n > 0 ? n : 380;
  }
  function applySize(s) {
    s = Math.max(MIN_SIZE, Math.min(MAX_SIZE, s));
    shell.style.setProperty('--wheel-size', s + 'px');
    return s;
  }

  resizeEl.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    ensurePos();
    resizing = true;
    resizeStart = { x: e.clientX, y: e.clientY, size: readSize() };
    resizeEl.setPointerCapture(e.pointerId);
    shell.classList.add('is-resizing');
    hideTooltip();
    e.stopPropagation();
    e.preventDefault();
  });
  window.addEventListener('pointermove', (e) => {
    if (!resizing) return;
    const dx = e.clientX - resizeStart.x;
    const dy = e.clientY - resizeStart.y;
    // Diagonal scaling — average of the two deltas
    const delta = (dx + dy) / 2;
    applySize(resizeStart.size + delta);
  });
  window.addEventListener('pointerup', () => {
    if (!resizing) return;
    resizing = false;
    shell.classList.remove('is-resizing');
  });

  // ── Keyboard nav ───────────────────────────────────────
  window.addEventListener('keydown', (e) => {
    if (e.target && /input|textarea/i.test(e.target.tagName)) return;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { setActive(active + 1); e.preventDefault(); }
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { setActive(active - 1); e.preventDefault(); }
    else if (e.key === 'Enter') { launch(); }
    else if (e.key === '+' || e.key === '=') { applySize(readSize() + 24); }
    else if (e.key === '-' || e.key === '_') { applySize(readSize() - 24); }
  });

  window.IH_Wheel = { activate: setActive, launch, sections: SECTIONS, setSize: applySize };
})();
