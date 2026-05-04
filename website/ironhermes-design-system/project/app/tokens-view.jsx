// tokens-view.jsx — Type, Color, Spacing, Icons

function TypeView() {
  const samples = [
    ["t-display", "IronHermes", "Iosekey Mono Cond · 700 · 48"],
    ["t-h1", "The self-improving agent", "Iosekey Mono Cond · 700 · 36"],
    ["t-h2", "Session storage & memory", "Sans · 600 · 22"],
    ["t-h3", "Active phase · 22.2", "Sans · 600 · 16"],
    ["t-body", "Body copy. The quick brown fox jumps.", "Sans · 400 · 14"],
    ["t-small", "Small — captions and hints.", "Sans · 400 · 12"],
    ["t-micro", "~/.ironhermes/MEMORY.md · 67%", "Iosekey Mono · 400 · 11"],
    ["t-label", "label · section header", "Iosekey Mono · 600 · 11 upper"],
  ];
  return (
    <div style={{padding:"var(--sp-10) var(--sp-12)", maxWidth: 1100}}>
      <span className="t-label">foundation / type</span>
      <h1 className="t-h1" style={{margin:"var(--sp-3) 0 var(--sp-6)"}}>Typography</h1>
      <div className="ih-card">
        {samples.map(([cls, txt, spec], i) => (
          <div key={cls} style={{display:"grid", gridTemplateColumns: "120px 1fr 220px", gap:"var(--sp-5)", alignItems:"baseline", padding:"var(--sp-5) var(--sp-6)", borderBottom: i === samples.length-1 ? 0 : "1px solid var(--border-subtle)"}}>
            <span className="mono t-micro fg-faint">.{cls}</span>
            <span className={cls}>{txt}</span>
            <span className="t-micro fg-dim">{spec}</span>
          </div>
        ))}
      </div>

      <h2 className="t-h2" style={{margin:"var(--sp-9) 0 var(--sp-4)"}}>Iosekey weights</h2>
      <div className="ih-card" style={{padding:"var(--sp-5) var(--sp-6)"}}>
        <div className="stack-sm">
          {[100,300,400,500,600,700,900].map(w => (
            <div key={w} style={{display:"grid", gridTemplateColumns:"60px 1fr", gap:"var(--sp-5)"}}>
              <span className="mono t-micro fg-faint">{w}</span>
              <span className="mono" style={{fontWeight: w, fontSize:"var(--fs-18)"}}>ironhermes —— self-improving agent</span>
            </div>
          ))}
        </div>
      </div>

      <h2 className="t-h2" style={{margin:"var(--sp-9) 0 var(--sp-4)"}}>Condensed variant</h2>
      <div className="ih-card" style={{padding:"var(--sp-7) var(--sp-6)"}}>
        <div style={{fontFamily:"var(--font-mono-cond)", fontWeight: 700, fontSize: 56, letterSpacing: "-0.015em"}}>IRONHERMES</div>
        <div style={{fontFamily:"var(--font-mono)", fontWeight: 700, fontSize: 56, letterSpacing: "-0.015em", opacity: .55, marginTop: -4}}>IRONHERMES</div>
        <div className="t-micro fg-faint" style={{marginTop: "var(--sp-4)"}}>Condensed (top) vs. regular (bottom) — used only for display and logo.</div>
      </div>
    </div>
  );
}

function ColorView() {
  const tokens = [
    { group: "surfaces", names: ["bg-canvas","bg-surface","bg-elevated","bg-raised","bg-input","bg-selected"] },
    { group: "text", names: ["fg","fg-muted","fg-dim","fg-faint","fg-on-accent"] },
    { group: "borders", names: ["border-subtle","border","border-strong"] },
    { group: "accent + semantic", names: ["accent","accent-hover","accent-soft","success","warning","danger","info"] },
  ];
  return (
    <div style={{padding:"var(--sp-10) var(--sp-12)", maxWidth: 1100}}>
      <span className="t-label">foundation / color</span>
      <h1 className="t-h1" style={{margin:"var(--sp-3) 0 var(--sp-6)"}}>Color</h1>
      <p className="t-body fg-muted" style={{maxWidth: "60ch", marginBottom:"var(--sp-7)"}}>
        All colors declared in <code className="inline">oklch()</code>. Accent chroma is kept in the 0.12–0.18 range so semantic colors hold weight across every theme.
      </p>
      {tokens.map(({group, names}) => (
        <div key={group} style={{marginBottom:"var(--sp-7)"}}>
          <span className="t-label" style={{marginBottom:"var(--sp-3)", display:"block"}}>{group}</span>
          <div className="ih-card" style={{padding:"var(--sp-4) var(--sp-5)"}}>
            {names.map(n => (
              <div className="swatch-row" key={n}>
                <div className="sw" style={{background: `var(--${n})`}}/>
                <div style={{flex:1}}>
                  <div className="name">--{n}</div>
                  <div className="meta">var(--{n})</div>
                </div>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function SpacingView() {
  const spaces = [1,2,3,4,5,6,7,8,9,10,11,12];
  const radii  = [1,2,3,4,5,"pill"];
  return (
    <div style={{padding:"var(--sp-10) var(--sp-12)", maxWidth: 1100}}>
      <span className="t-label">foundation / spacing</span>
      <h1 className="t-h1" style={{margin:"var(--sp-3) 0 var(--sp-6)"}}>Spacing & radii</h1>

      <h2 className="t-h2" style={{marginTop:"var(--sp-6)"}}>Spacing · 4pt grid</h2>
      <div className="ih-card" style={{padding:"var(--sp-5)", marginTop:"var(--sp-3)"}}>
        {spaces.map(n => (
          <div key={n} className="row gap-5" style={{padding:"var(--sp-2) 0"}}>
            <span className="mono t-micro fg-faint" style={{width:60}}>--sp-{n}</span>
            <span className="mono fg-dim" style={{width:60}}>{ [0,4,8,12,16,20,24,32,40,48,64,80,96][n] }px</span>
            <div style={{height: 8, background:"var(--accent)", width: `var(--sp-${n})`}}/>
          </div>
        ))}
      </div>

      <h2 className="t-h2" style={{marginTop:"var(--sp-9)"}}>Radii</h2>
      <div className="row gap-6" style={{marginTop:"var(--sp-4)", flexWrap:"wrap"}}>
        {radii.map(n => (
          <div key={n} style={{textAlign:"center"}}>
            <div style={{width: 80, height: 80, background:"var(--accent-soft)", border:"1px solid var(--accent)", borderRadius: `var(--r-${n})`}}/>
            <div className="mono t-micro fg-faint" style={{marginTop: 6}}>--r-{n}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

function IconsView() {
  const names = ["Chat","Session","Search","Memory","Skill","Cron","Hook","Subagent","Tool","Terminal","Provider","Soul","Gear","Plus","Close","Check","Chevron","ChevronDown","More","Copy","Send","Play","Pause","Stop","Refresh","Edit","Trash","Dot","Warn","Info","Lock","Folder","File","Code","Command","Sparkle","Thought","Mic","Attach","Branch"];
  return (
    <div style={{padding:"var(--sp-10) var(--sp-12)", maxWidth: 1100}}>
      <span className="t-label">foundation / icons</span>
      <h1 className="t-h1" style={{margin:"var(--sp-3) 0 var(--sp-6)"}}>Iconography</h1>
      <p className="t-body fg-muted" style={{maxWidth: "60ch", marginBottom:"var(--sp-7)"}}>
        16-unit SVG grid. 1.5px stroke, rounded joins, currentColor. Rendered at 12–16px in UI.
      </p>
      <div className="icon-grid">
        {names.map(n => {
          const Ic = I[n];
          if (!Ic) return null;
          return (
            <div className="cell" key={n}>
              <Ic size={18} style={{color:"var(--fg)"}}/>
              <span className="name">{n}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

window.TypeView = TypeView; window.ColorView = ColorView;
window.SpacingView = SpacingView; window.IconsView = IconsView;
