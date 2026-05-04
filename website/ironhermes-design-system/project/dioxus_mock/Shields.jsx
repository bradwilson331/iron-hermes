// Shields.jsx — IH shield logo explorations. Hermes mythos + aged patina.
// Pure SVG, no raster. Each variant ~400×480.

// Shared defs: aged-bronze gradients, noise filter, wear mask.
function ShieldDefs({ id }) {
  return (
    <defs>
      <linearGradient id={`${id}-bronze`} x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%"  stopColor="#d4995a"/>
        <stop offset="40%" stopColor="#a6702f"/>
        <stop offset="70%" stopColor="#7a4a1c"/>
        <stop offset="100%" stopColor="#4a2d10"/>
      </linearGradient>
      <linearGradient id={`${id}-copper`} x1="0" y1="0" x2="1" y2="1">
        <stop offset="0%"  stopColor="#c88549"/>
        <stop offset="100%" stopColor="#5a3012"/>
      </linearGradient>
      <radialGradient id={`${id}-patina`} cx="0.5" cy="0.4" r="0.7">
        <stop offset="0%"  stopColor="#4a6a5a" stopOpacity="0"/>
        <stop offset="70%" stopColor="#2a4a3a" stopOpacity="0.35"/>
        <stop offset="100%" stopColor="#12251e" stopOpacity="0.6"/>
      </radialGradient>
      <filter id={`${id}-noise`} x="-10%" y="-10%" width="120%" height="120%">
        <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="3"/>
        <feColorMatrix values="0 0 0 0 0.15  0 0 0 0 0.08  0 0 0 0 0.02  0 0 0 0.4 0"/>
        <feComposite in2="SourceGraphic" operator="in"/>
      </filter>
      <filter id={`${id}-rough`} x="-5%" y="-5%" width="110%" height="110%">
        <feTurbulence type="fractalNoise" baseFrequency="0.05" numOctaves="2" seed="5"/>
        <feDisplacementMap in="SourceGraphic" scale="1.2"/>
      </filter>
    </defs>
  );
}

// Scratches layer — hairline diagonals for wear
function Scratches({ opacity = 0.18 }) {
  const lines = [
    [40, 120, 210, 80],
    [80, 250, 300, 230],
    [60, 380, 180, 360],
    [180, 450, 360, 400],
    [250, 60, 340, 50],
    [30, 200, 120, 180],
    [280, 300, 370, 340],
  ];
  return (
    <g opacity={opacity}>
      {lines.map(([x1, y1, x2, y2], i) => (
        <line key={i} x1={x1} y1={y1} x2={x2} y2={y2}
          stroke="#e8d4b2" strokeWidth="0.6" strokeLinecap="round"/>
      ))}
    </g>
  );
}

// Dents — darker flecks simulating oxidation spots
function Dents() {
  const spots = [
    [60, 140, 8], [330, 160, 6], [100, 340, 10], [310, 380, 7],
    [220, 90, 5], [70, 400, 6], [280, 80, 4], [150, 430, 5],
  ];
  return (
    <g opacity="0.35">
      {spots.map(([cx, cy, r], i) => (
        <circle key={i} cx={cx} cy={cy} r={r} fill="#2a1a0a"/>
      ))}
    </g>
  );
}

// Rivets
function Rivets({ coords }) {
  return (
    <g>
      {coords.map(([cx, cy], i) => (
        <g key={i}>
          <circle cx={cx} cy={cy} r="5" fill="#3a2410"/>
          <circle cx={cx - 1} cy={cy - 1} r="3" fill="url(#v1-copper)" opacity="0.8"/>
          <circle cx={cx - 1.5} cy={cy - 1.5} r="1" fill="#f0c89a" opacity="0.7"/>
        </g>
      ))}
    </g>
  );
}

// Standard "heater" shield path — classic medieval escutcheon
const shieldPath = "M 40 40 L 360 40 L 360 240 Q 360 380 200 450 Q 40 380 40 240 Z";
const shieldInset = "M 60 60 L 340 60 L 340 240 Q 340 362 200 426 Q 60 362 60 240 Z";

// ─── Variant 1: Caduceus — two serpents entwined, wings overhead ────────
function CaduceusShield() {
  return (
    <svg viewBox="0 0 400 480" width="100%" height="100%"
      style={{display: "block", background: "#1a1410"}}>
      <ShieldDefs id="v1"/>

      {/* Shield body */}
      <path d={shieldPath} fill="url(#v1-bronze)"/>
      <path d={shieldPath} fill="url(#v1-patina)" opacity="0.85"/>
      <path d={shieldPath} fill="none" stroke="#2a1508" strokeWidth="3"/>

      {/* Inner beveled frame */}
      <path d={shieldInset} fill="none" stroke="#2a1508" strokeWidth="1.5" opacity="0.8"/>
      <path d="M 60 60 L 340 60" stroke="#f0c89a" strokeWidth="1" opacity="0.3"/>

      <Rivets coords={[[60, 60], [340, 60], [60, 240], [340, 240]]}/>

      {/* Caduceus: central staff */}
      <g>
        {/* Wings — spread at top */}
        <g fill="#3a2410" stroke="#1a0a04" strokeWidth="1" opacity="0.92">
          {/* Left wing — layered feathers */}
          <path d="M 200 110 Q 160 100 130 112 Q 100 125 85 150 Q 110 142 135 140 Q 115 152 100 170 Q 130 162 150 156 Q 140 170 130 186 Q 160 172 180 162 Q 185 140 195 120 Z"/>
          <path d="M 198 115 Q 170 112 150 120 Q 135 130 128 145 L 145 140 L 140 152 L 160 148 L 158 160 L 175 154 Z" fill="#5a3620" opacity="0.7"/>
          {/* Right wing (mirror) */}
          <path d="M 200 110 Q 240 100 270 112 Q 300 125 315 150 Q 290 142 265 140 Q 285 152 300 170 Q 270 162 250 156 Q 260 170 270 186 Q 240 172 220 162 Q 215 140 205 120 Z"/>
          <path d="M 202 115 Q 230 112 250 120 Q 265 130 272 145 L 255 140 L 260 152 L 240 148 L 242 160 L 225 154 Z" fill="#5a3620" opacity="0.7"/>
        </g>

        {/* Orb at top of staff */}
        <circle cx="200" cy="108" r="8" fill="#3a2410" stroke="#1a0a04" strokeWidth="1"/>
        <circle cx="198" cy="106" r="3" fill="#e8c08a" opacity="0.7"/>

        {/* Central staff */}
        <rect x="197" y="116" width="6" height="250" fill="#3a2410"/>
        <rect x="198" y="116" width="2" height="250" fill="#6b4422" opacity="0.6"/>

        {/* Two serpents entwined — three crossings */}
        <g fill="none" stroke="#2a1508" strokeWidth="2.5" strokeLinecap="round">
          {/* Serpent A — left */}
          <path d="M 170 130 Q 150 150 180 175 Q 230 195 220 225 Q 210 255 175 275 Q 140 295 170 325 Q 195 345 185 365" 
            fill="none" stroke="#c89257" strokeWidth="9"/>
          <path d="M 170 130 Q 150 150 180 175 Q 230 195 220 225 Q 210 255 175 275 Q 140 295 170 325 Q 195 345 185 365"
            fill="none" stroke="#3a2410" strokeWidth="2.5" opacity="0.8"/>
          {/* Serpent B — right (mirror) */}
          <path d="M 230 130 Q 250 150 220 175 Q 170 195 180 225 Q 190 255 225 275 Q 260 295 230 325 Q 205 345 215 365"
            fill="none" stroke="#a8763e" strokeWidth="9"/>
          <path d="M 230 130 Q 250 150 220 175 Q 170 195 180 225 Q 190 255 225 275 Q 260 295 230 325 Q 205 345 215 365"
            fill="none" stroke="#2a1508" strokeWidth="2.5" opacity="0.8"/>
        </g>

        {/* Serpent heads */}
        <g fill="#c89257" stroke="#2a1508" strokeWidth="1.5">
          <path d="M 170 128 Q 155 118 160 108 L 172 115 L 180 110 L 175 122 Z"/>
          <path d="M 230 128 Q 245 118 240 108 L 228 115 L 220 110 L 225 122 Z"/>
        </g>
        <circle cx="165" cy="117" r="1.2" fill="#1a0a04"/>
        <circle cx="235" cy="117" r="1.2" fill="#1a0a04"/>
      </g>

      {/* IH monogram — engraved feel, brand orange */}
      <g>
        {/* I */}
        <rect x="108" y="280" width="12" height="70" fill="#f0883e"/>
        <rect x="95" y="280" width="38" height="4" fill="#f0883e"/>
        <rect x="95" y="346" width="38" height="4" fill="#f0883e"/>
        {/* H */}
        <rect x="266" y="280" width="12" height="70" fill="#f0883e"/>
        <rect x="308" y="280" width="12" height="70" fill="#f0883e"/>
        <rect x="266" y="310" width="54" height="6" fill="#f0883e"/>

        {/* Engraved shadow — offset dark copy below */}
        <g opacity="0.4">
          <rect x="108" y="282" width="12" height="70" fill="#2a1508"/>
          <rect x="266" y="282" width="12" height="70" fill="#2a1508"/>
          <rect x="308" y="282" width="12" height="70" fill="#2a1508"/>
        </g>
      </g>

      {/* Scroll banner */}
      <g>
        <path d="M 70 390 Q 200 405 330 390 L 335 420 Q 200 435 65 420 Z"
          fill="#3a2410" stroke="#1a0a04" strokeWidth="1.5"/>
        <path d="M 70 390 Q 200 405 330 390" fill="none" stroke="#6b4422" strokeWidth="1" opacity="0.6"/>
        <text x="200" y="412" textAnchor="middle"
          fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="14" fontWeight="700" fill="#e8c08a" letterSpacing="3">
          MMXXVI
        </text>
      </g>

      <Dents/>
      <Scratches/>

      {/* Chipped edge — corner wear */}
      <g opacity="0.5">
        <path d="M 40 40 L 55 50 L 45 62 L 40 55 Z" fill="#1a0a04"/>
        <path d="M 360 40 L 345 50 L 355 62 L 360 55 Z" fill="#1a0a04"/>
      </g>
    </svg>
  );
}

// ─── Variant 2: Petasos — winged traveler's hat, minimal monogram ─────────
function PetasosShield() {
  return (
    <svg viewBox="0 0 400 480" width="100%" height="100%"
      style={{display: "block", background: "#14110d"}}>
      <ShieldDefs id="v2"/>

      <path d={shieldPath} fill="url(#v2-bronze)"/>
      <path d={shieldPath} fill="#4a3a20" opacity="0.4"/>
      <path d={shieldPath} fill="none" stroke="#1a0a04" strokeWidth="4"/>

      {/* Double border line */}
      <path d="M 55 55 L 345 55 L 345 240 Q 345 365 200 430 Q 55 365 55 240 Z"
        fill="none" stroke="#f0c89a" strokeWidth="0.8" opacity="0.35"/>

      <Rivets coords={[[55, 55], [345, 55], [55, 240], [345, 240], [200, 55]]}/>

      {/* Petasos — winged hat centered upper half */}
      <g>
        {/* Hat dome */}
        <ellipse cx="200" cy="160" rx="68" ry="42" fill="#2a1810" stroke="#0a0604" strokeWidth="2"/>
        <ellipse cx="200" cy="158" rx="64" ry="36" fill="none" stroke="#6b4422" strokeWidth="1" opacity="0.5"/>
        {/* Hat brim */}
        <ellipse cx="200" cy="190" rx="90" ry="14" fill="#2a1810" stroke="#0a0604" strokeWidth="2"/>
        <ellipse cx="200" cy="188" rx="86" ry="8" fill="#6b4422" opacity="0.4"/>
        {/* Hat band */}
        <rect x="138" y="176" width="124" height="6" fill="#c89257"/>
        <rect x="138" y="176" width="124" height="1" fill="#f0c89a" opacity="0.6"/>

        {/* Wings — on either side of hat */}
        <g fill="#c89257" stroke="#1a0a04" strokeWidth="1.5" opacity="0.95">
          {/* Left wing */}
          <path d="M 130 170 Q 85 155 55 170 Q 75 178 95 180 Q 70 185 50 200 Q 85 195 105 192 Q 90 204 80 220 Q 115 208 130 198 Q 132 184 130 170 Z"/>
          {/* Right wing */}
          <path d="M 270 170 Q 315 155 345 170 Q 325 178 305 180 Q 330 185 350 200 Q 315 195 295 192 Q 310 204 320 220 Q 285 208 270 198 Q 268 184 270 170 Z"/>
          {/* Feather accents */}
          <path d="M 120 175 L 110 180 M 115 185 L 100 192 M 110 195 L 95 202"
            stroke="#1a0a04" strokeWidth="0.8" fill="none"/>
          <path d="M 280 175 L 290 180 M 285 185 L 300 192 M 290 195 L 305 202"
            stroke="#1a0a04" strokeWidth="0.8" fill="none"/>
        </g>
      </g>

      {/* Heavy IH monogram — serif slab, centered lower */}
      <g>
        {/* I */}
        <g>
          <rect x="122" y="250" width="16" height="100" fill="#f0883e"/>
          <rect x="104" y="250" width="52" height="8" fill="#f0883e"/>
          <rect x="104" y="342" width="52" height="8" fill="#f0883e"/>
          {/* Engraved inner highlight */}
          <rect x="126" y="254" width="2" height="92" fill="#ffb870" opacity="0.7"/>
        </g>
        {/* H */}
        <g>
          <rect x="244" y="250" width="16" height="100" fill="#f0883e"/>
          <rect x="292" y="250" width="16" height="100" fill="#f0883e"/>
          <rect x="244" y="290" width="64" height="10" fill="#f0883e"/>
          <rect x="248" y="254" width="2" height="92" fill="#ffb870" opacity="0.7"/>
          <rect x="296" y="254" width="2" height="92" fill="#ffb870" opacity="0.7"/>
        </g>

        {/* Drop shadow for engraved feel */}
        <g opacity="0.6" style={{mixBlendMode: "multiply"}}>
          <rect x="122" y="352" width="16" height="3" fill="#1a0a04"/>
          <rect x="244" y="352" width="16" height="3" fill="#1a0a04"/>
          <rect x="292" y="352" width="16" height="3" fill="#1a0a04"/>
        </g>
      </g>

      {/* Motto scroll */}
      <g>
        <path d="M 75 380 L 325 380 L 332 402 L 68 402 Z"
          fill="#1a0a04" stroke="#3a2410" strokeWidth="1"/>
        <text x="200" y="396" textAnchor="middle"
          fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="11" fontWeight="700" fill="#c89257" letterSpacing="4">
          ITER · NVNTIVS · FERRVM
        </text>
      </g>

      <Dents/>
      <Scratches opacity={0.22}/>
    </svg>
  );
}

// ─── Variant 3: Talaria — winged sandals, quartered shield ──────────────
function TalariaShield() {
  return (
    <svg viewBox="0 0 400 480" width="100%" height="100%"
      style={{display: "block", background: "#10120f"}}>
      <ShieldDefs id="v3"/>

      <path d={shieldPath} fill="url(#v3-bronze)"/>
      <path d={shieldPath} fill="url(#v3-patina)" opacity="0.6"/>
      <path d={shieldPath} fill="none" stroke="#1a0a04" strokeWidth="3"/>

      {/* Cross/quartered divider lines */}
      <line x1="200" y1="40" x2="200" y2="450" stroke="#1a0a04" strokeWidth="1.5" opacity="0.55"/>
      <path d="M 42 240 Q 200 230 358 240" stroke="#1a0a04" strokeWidth="1.5" opacity="0.55" fill="none"/>

      <Rivets coords={[[58, 58], [342, 58], [58, 238], [342, 238]]}/>

      {/* Top-left quadrant: left winged sandal */}
      <g transform="translate(118, 145)">
        {/* Sandal sole */}
        <path d="M -30 0 Q -35 14 -20 16 L 25 18 Q 40 17 38 4 Q 35 -8 20 -6 L -20 -10 Q -32 -10 -30 0 Z"
          fill="#2a1810" stroke="#0a0604" strokeWidth="1.5"/>
        {/* Straps */}
        <path d="M -10 -8 L -5 -18 M 5 -8 L 10 -18 M 18 -5 L 20 -15"
          stroke="#c89257" strokeWidth="1.8" fill="none" strokeLinecap="round"/>
        {/* Heel wing */}
        <g fill="#c89257" stroke="#1a0a04" strokeWidth="1">
          <path d="M -30 -4 Q -48 -14 -62 -8 Q -52 -2 -44 2 Q -54 4 -62 14 Q -48 10 -38 8 Q -44 18 -46 28 Q -34 18 -28 10 Q -28 2 -30 -4 Z"/>
        </g>
      </g>

      {/* Top-right quadrant: right winged sandal */}
      <g transform="translate(282, 145)">
        <path d="M 30 0 Q 35 14 20 16 L -25 18 Q -40 17 -38 4 Q -35 -8 -20 -6 L 20 -10 Q 32 -10 30 0 Z"
          fill="#2a1810" stroke="#0a0604" strokeWidth="1.5"/>
        <path d="M 10 -8 L 5 -18 M -5 -8 L -10 -18 M -18 -5 L -20 -15"
          stroke="#c89257" strokeWidth="1.8" fill="none" strokeLinecap="round"/>
        <g fill="#c89257" stroke="#1a0a04" strokeWidth="1">
          <path d="M 30 -4 Q 48 -14 62 -8 Q 52 -2 44 2 Q 54 4 62 14 Q 48 10 38 8 Q 44 18 46 28 Q 34 18 28 10 Q 28 2 30 -4 Z"/>
        </g>
      </g>

      {/* Bottom-left quadrant: lyre / small hermes symbol */}
      <g transform="translate(120, 320)">
        <path d="M -22 -20 Q -28 -25 -30 -18 L -28 10 Q -28 22 -20 28 L 20 28 Q 28 22 28 10 L 30 -18 Q 28 -25 22 -20"
          fill="none" stroke="#c89257" strokeWidth="2.5"/>
        <line x1="-16" y1="-18" x2="-12" y2="24" stroke="#c89257" strokeWidth="1.2"/>
        <line x1="-8"  y1="-18" x2="-4"  y2="24" stroke="#c89257" strokeWidth="1.2"/>
        <line x1="0"   y1="-18" x2="0"   y2="24" stroke="#c89257" strokeWidth="1.2"/>
        <line x1="8"   y1="-18" x2="4"   y2="24" stroke="#c89257" strokeWidth="1.2"/>
        <line x1="16"  y1="-18" x2="12"  y2="24" stroke="#c89257" strokeWidth="1.2"/>
      </g>

      {/* Bottom-right quadrant: gear-in-serpent (self-improving loop) */}
      <g transform="translate(280, 320)">
        {/* Outer serpent ring biting tail (ouroboros) */}
        <circle cx="0" cy="0" r="28" fill="none" stroke="#c89257" strokeWidth="4"/>
        <circle cx="0" cy="0" r="28" fill="none" stroke="#2a1508" strokeWidth="1.5" opacity="0.6"/>
        {/* Scale texture */}
        {[...Array(12)].map((_, i) => {
          const a = (i / 12) * Math.PI * 2;
          const x1 = Math.cos(a) * 28, y1 = Math.sin(a) * 28;
          const x2 = Math.cos(a) * 24, y2 = Math.sin(a) * 24;
          return <line key={i} x1={x1} y1={y1} x2={x2} y2={y2} stroke="#1a0a04" strokeWidth="1"/>;
        })}
        {/* Head */}
        <path d="M 22 -14 L 32 -8 L 26 -2 Z" fill="#c89257" stroke="#1a0a04" strokeWidth="1"/>
        <circle cx="28" cy="-8" r="1" fill="#1a0a04"/>
        {/* Central IH mini-mark */}
        <text x="0" y="5" textAnchor="middle"
          fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="16" fontWeight="900" fill="#f0883e">IH</text>
      </g>

      {/* Large IH on center chief — overlapping the quartering */}
      <g>
        {/* Circular medallion */}
        <circle cx="200" cy="240" r="38" fill="#1a0a04" stroke="#c89257" strokeWidth="3"/>
        <circle cx="200" cy="240" r="32" fill="none" stroke="#6b4422" strokeWidth="0.8" opacity="0.6"/>
        <text x="200" y="253" textAnchor="middle"
          fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="32" fontWeight="900" fill="#f0883e" letterSpacing="-1">IH</text>
      </g>

      <Dents/>
      <Scratches/>

      {/* Border */}
      <path d="M 48 48 L 352 48 L 352 240 Q 352 370 200 438 Q 48 370 48 240 Z"
        fill="none" stroke="#c89257" strokeWidth="1" opacity="0.5"/>
    </svg>
  );
}

// ─── Variant 4: Minimalist emblem — tight IH + caduceus spine ─────────
function EmblemShield() {
  return (
    <svg viewBox="0 0 400 480" width="100%" height="100%"
      style={{display: "block", background: "#0a0805"}}>
      <ShieldDefs id="v4"/>

      {/* Matte black shield with subtle bronze edge */}
      <path d={shieldPath} fill="#1a1510"/>
      <path d={shieldPath} fill="url(#v4-patina)" opacity="0.4"/>
      <path d={shieldPath} fill="none" stroke="#c89257" strokeWidth="2.5"/>
      <path d={shieldPath} fill="none" stroke="#6b4422" strokeWidth="0.8" opacity="0.8"
        transform="translate(0, 2)"/>

      {/* Inner double-line */}
      <path d="M 62 62 L 338 62 L 338 240 Q 338 360 200 425 Q 62 360 62 240 Z"
        fill="none" stroke="#c89257" strokeWidth="0.8" opacity="0.5"/>
      <path d="M 70 70 L 330 70 L 330 238 Q 330 354 200 416 Q 70 354 70 238 Z"
        fill="none" stroke="#c89257" strokeWidth="0.4" opacity="0.3"/>

      {/* Top wings only — spread as a crest */}
      <g fill="#c89257" opacity="0.9">
        <path d="M 200 130 Q 170 122 135 130 Q 100 140 80 155 Q 110 152 138 152 Q 115 160 98 178 Q 135 168 160 162 Q 148 175 140 190 Q 175 178 195 165 Q 200 148 200 130 Z"
          stroke="#1a0a04" strokeWidth="1"/>
        <path d="M 200 130 Q 230 122 265 130 Q 300 140 320 155 Q 290 152 262 152 Q 285 160 302 178 Q 265 168 240 162 Q 252 175 260 190 Q 225 178 205 165 Q 200 148 200 130 Z"
          stroke="#1a0a04" strokeWidth="1"/>
      </g>

      {/* Vertical caduceus spine between IH letters */}
      <g>
        {/* Orb top */}
        <circle cx="200" cy="125" r="5" fill="#f0883e" stroke="#1a0a04" strokeWidth="1"/>
        {/* Staff */}
        <line x1="200" y1="130" x2="200" y2="400" stroke="#c89257" strokeWidth="2.5"/>
        <line x1="200" y1="130" x2="200" y2="400" stroke="#6b4422" strokeWidth="0.8" opacity="0.8" transform="translate(0.5, 0)"/>
        {/* Serpents — two sine waves in opposite phase */}
        <path d="M 200 180 Q 180 200 200 220 Q 220 240 200 260 Q 180 280 200 300 Q 220 320 200 340 Q 180 360 200 380"
          fill="none" stroke="#c89257" strokeWidth="3.5"/>
        <path d="M 200 180 Q 220 200 200 220 Q 180 240 200 260 Q 220 280 200 300 Q 180 320 200 340 Q 220 360 200 380"
          fill="none" stroke="#a8763e" strokeWidth="3.5"/>
        {/* Serpent heads at top */}
        <path d="M 186 178 Q 178 170 184 165 L 192 172 Z" fill="#c89257" stroke="#1a0a04" strokeWidth="0.8"/>
        <path d="M 214 178 Q 222 170 216 165 L 208 172 Z" fill="#a8763e" stroke="#1a0a04" strokeWidth="0.8"/>
        {/* Base knob */}
        <rect x="195" y="398" width="10" height="10" fill="#c89257" stroke="#1a0a04" strokeWidth="1"/>
      </g>

      {/* IH — flanking the staff, engraved serif slab */}
      <g fill="#f0883e">
        {/* I (left) */}
        <rect x="110" y="220" width="18" height="110" fill="#f0883e"/>
        <rect x="90"  y="220" width="58" height="10" fill="#f0883e"/>
        <rect x="90"  y="320" width="58" height="10" fill="#f0883e"/>
        {/* inner highlight */}
        <rect x="114" y="224" width="2" height="102" fill="#ffb870" opacity="0.8"/>

        {/* H (right) */}
        <rect x="252" y="220" width="18" height="110" fill="#f0883e"/>
        <rect x="304" y="220" width="18" height="110" fill="#f0883e"/>
        <rect x="256" y="224" width="2" height="102" fill="#ffb870" opacity="0.8"/>
        <rect x="308" y="224" width="2" height="102" fill="#ffb870" opacity="0.8"/>
        {/* H has no crossbar — the central staff IS the crossbar */}

        {/* Engraved base shadow */}
        <g opacity="0.55">
          <rect x="110" y="332" width="18" height="3" fill="#2a1508"/>
          <rect x="252" y="332" width="18" height="3" fill="#2a1508"/>
          <rect x="304" y="332" width="18" height="3" fill="#2a1508"/>
        </g>
      </g>

      {/* Corner stars */}
      <g fill="#c89257" opacity="0.7">
        {[[85, 85], [315, 85], [90, 200], [310, 200]].map(([x, y], i) => (
          <polygon key={i}
            points={`${x},${y-5} ${x+1.5},${y-1.5} ${x+5},${y} ${x+1.5},${y+1.5} ${x},${y+5} ${x-1.5},${y+1.5} ${x-5},${y} ${x-1.5},${y-1.5}`}/>
        ))}
      </g>

      {/* Bottom motto */}
      <text x="200" y="405" textAnchor="middle"
        fontFamily="'Ioskeley Mono', ui-monospace, monospace"
        fontSize="10" fontWeight="700" fill="#c89257" letterSpacing="6" opacity="0.85">
        IRON · HERMES
      </text>

      <Dents/>
      <Scratches opacity={0.2}/>
    </svg>
  );
}

// ─── Wordmark lockup — shield + wordmark horizontal ─────────
function LockupShield() {
  return (
    <svg viewBox="0 0 720 240" width="100%" height="100%"
      style={{display: "block", background: "#10110d"}}>
      <ShieldDefs id="v5"/>

      {/* Compact shield on left */}
      <g transform="translate(20, 20) scale(0.42)">
        <path d={shieldPath} fill="#1a1510"/>
        <path d={shieldPath} fill="none" stroke="#c89257" strokeWidth="5"/>
        {/* Simplified wings */}
        <g fill="#c89257">
          <path d="M 200 140 Q 160 128 120 140 Q 90 150 75 168 Q 110 162 140 160 Q 120 172 105 190 Q 140 180 170 170 Q 200 158 200 140 Z"
            stroke="#1a0a04" strokeWidth="2"/>
          <path d="M 200 140 Q 240 128 280 140 Q 310 150 325 168 Q 290 162 260 160 Q 280 172 295 190 Q 260 180 230 170 Q 200 158 200 140 Z"
            stroke="#1a0a04" strokeWidth="2"/>
        </g>
        {/* IH */}
        <text x="200" y="310" textAnchor="middle"
          fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="130" fontWeight="900" fill="#f0883e" letterSpacing="-4">IH</text>
        {/* Central staff */}
        <line x1="200" y1="200" x2="200" y2="400" stroke="#c89257" strokeWidth="8"/>
      </g>

      {/* Wordmark */}
      <g transform="translate(230, 120)">
        <text fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="62" fontWeight="900" fill="#f0883e" letterSpacing="-2">Iron</text>
        <text x="148" fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="62" fontWeight="900" fill="#e8d4b2" letterSpacing="-2">Hermes</text>
        <text y="36" fontFamily="'Ioskeley Mono', ui-monospace, monospace"
          fontSize="13" fontWeight="400" fill="#8a7555" letterSpacing="6">
          SELF·IMPROVING · AGENT · SINCE · MMXXV
        </text>
      </g>
    </svg>
  );
}

// ─── Colorway variants: the same core Caduceus in 4 palettes ─────────
function ColorwayShield({ palette = "bronze" }) {
  const palettes = {
    bronze: { bg: "#1a1410", body: "#a6702f", edge: "#4a2d10", accent: "#f0883e", tint: "#c89257" },
    verdigris: { bg: "#0d1512", body: "#2a5a4a", edge: "#12251e", accent: "#4ec9b0", tint: "#6fd8c2" },
    iron: { bg: "#0a0c10", body: "#3a4a5a", edge: "#121820", accent: "#c9d1d9", tint: "#8ba0b8" },
    noir: { bg: "#080808", body: "#1a1a1a", edge: "#000000", accent: "#f0883e", tint: "#444444" },
  };
  const p = palettes[palette];
  return (
    <svg viewBox="0 0 400 480" width="100%" height="100%" style={{display: "block", background: p.bg}}>
      <defs>
        <linearGradient id={`cw-${palette}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={p.tint}/>
          <stop offset="60%" stopColor={p.body}/>
          <stop offset="100%" stopColor={p.edge}/>
        </linearGradient>
      </defs>
      <path d={shieldPath} fill={`url(#cw-${palette})`}/>
      <path d={shieldPath} fill="none" stroke={p.edge} strokeWidth="3"/>
      <path d={shieldInset} fill="none" stroke={p.tint} strokeWidth="0.8" opacity="0.4"/>

      {/* Simplified wings */}
      <g fill={p.tint} opacity="0.95">
        <path d="M 200 120 Q 160 110 128 122 Q 98 135 85 158 Q 115 152 140 150 Q 120 162 108 180 Q 140 170 160 164 Q 148 180 140 196 Q 170 184 190 172 Q 198 146 200 120 Z"
          stroke={p.edge} strokeWidth="1.2"/>
        <path d="M 200 120 Q 240 110 272 122 Q 302 135 315 158 Q 285 152 260 150 Q 280 162 292 180 Q 260 170 240 164 Q 252 180 260 196 Q 230 184 210 172 Q 202 146 200 120 Z"
          stroke={p.edge} strokeWidth="1.2"/>
      </g>

      {/* Central staff */}
      <rect x="197" y="128" width="6" height="260" fill={p.tint}/>
      <circle cx="200" cy="124" r="6" fill={p.accent} stroke={p.edge} strokeWidth="1"/>

      {/* Serpents */}
      <path d="M 170 160 Q 150 180 180 205 Q 230 220 220 250 Q 210 280 180 295 Q 150 315 175 340 Q 195 360 188 380"
        fill="none" stroke={p.tint} strokeWidth="6"/>
      <path d="M 230 160 Q 250 180 220 205 Q 170 220 180 250 Q 190 280 220 295 Q 250 315 225 340 Q 205 360 212 380"
        fill="none" stroke={p.body} strokeWidth="6"/>

      {/* IH */}
      <g>
        <rect x="108" y="240" width="14" height="80" fill={p.accent}/>
        <rect x="92"  y="240" width="46" height="6" fill={p.accent}/>
        <rect x="92"  y="314" width="46" height="6" fill={p.accent}/>
        <rect x="262" y="240" width="14" height="80" fill={p.accent}/>
        <rect x="308" y="240" width="14" height="80" fill={p.accent}/>
        <rect x="262" y="274" width="60" height="8" fill={p.accent}/>
      </g>

      <Dents/>
      <Scratches opacity={0.18}/>
    </svg>
  );
}

// ─── Application contexts ─────────
function OnDarkContext() {
  return (
    <div style={{
      background: "#0d1117", padding: 24, height: "100%",
      display: "flex", flexDirection: "column", alignItems: "center", gap: 16
    }}>
      <div style={{width: 160, height: 192}}>
        <EmblemShield/>
      </div>
      <div style={{
        fontFamily: "'Ioskeley Mono', ui-monospace, monospace",
        color: "#f0883e", fontSize: 22, fontWeight: 700, letterSpacing: -0.5,
      }}>IronHermes</div>
      <div style={{
        fontFamily: "'Ioskeley Mono', ui-monospace, monospace",
        color: "#6a727a", fontSize: 10, letterSpacing: 3,
      }}>v2.0.0 · self-improving</div>
    </div>
  );
}

function FaviconContext() {
  const sizes = [16, 32, 64, 128];
  return (
    <div style={{
      background: "#0d1117", padding: 20, height: "100%",
      display: "flex", flexDirection: "column", gap: 16, alignItems: "center", justifyContent: "center"
    }}>
      <div style={{color: "#8a9199", fontFamily: "'Ioskeley Mono', ui-monospace, monospace", fontSize: 10, letterSpacing: 2}}>
        FAVICON · APP ICON SIZES
      </div>
      <div style={{display: "flex", gap: 18, alignItems: "flex-end"}}>
        {sizes.map(s => (
          <div key={s} style={{display: "flex", flexDirection: "column", alignItems: "center", gap: 6}}>
            <div style={{width: s, height: s * 1.2, overflow: "hidden"}}>
              <EmblemShield/>
            </div>
            <div style={{color: "#8a9199", fontFamily: "'Ioskeley Mono', ui-monospace, monospace", fontSize: 9}}>
              {s}px
            </div>
          </div>
        ))}
      </div>
      <div style={{marginTop: 20, color: "#6a727a", fontFamily: "'Ioskeley Mono', ui-monospace, monospace", fontSize: 10, maxWidth: 240, textAlign: "center", lineHeight: 1.5}}>
        emblem reads down to 16px — the orange IH dominates; wings + shield read as silhouette
      </div>
    </div>
  );
}

window.CaduceusShield = CaduceusShield;
window.PetasosShield = PetasosShield;
window.TalariaShield = TalariaShield;
window.EmblemShield = EmblemShield;
window.LockupShield = LockupShield;
window.ColorwayShield = ColorwayShield;
window.OnDarkContext = OnDarkContext;
window.FaviconContext = FaviconContext;
