// overview.jsx — overview + brand views

function OverviewView() {
  return (
    <div style={{padding: "var(--sp-10) var(--sp-12)", maxWidth: 1100}}>
      <div className="row gap-3" style={{marginBottom: "var(--sp-4)"}}>
        <Badge tone="accent" dot>self-improving rust agent</Badge>
        <Badge>v0.1.0 · design tokens</Badge>
      </div>
      <h1 className="t-display" style={{margin: "0 0 var(--sp-5)"}}>
        Iron<span style={{color:"var(--accent)"}}>Hermes</span> design system
      </h1>
      <p className="t-body fg-muted" style={{maxWidth: "62ch", fontSize: "var(--fs-16)", lineHeight: 1.6}}>
        A unified visual language for IronHermes across its four surfaces:
        the macOS desktop chat, the iOS companion app, the web admin dashboard,
        and the terminal TUI. Typography is anchored by Iosekey Mono with a
        pairing sans for UI body copy. Every color is defined in oklch so accent
        weight holds across five interchangeable themes.
      </p>

      <div style={{display:"grid", gridTemplateColumns:"repeat(3, 1fr)", gap: "var(--sp-5)", marginTop: "var(--sp-9)"}}>
        <FeatureCard icon="Soul" title="5 themes" desc="slate · iron · terminal · parchment, in dark and light variants. Swap at runtime."/>
        <FeatureCard icon="Tool" title="42 components" desc="Buttons, inputs, badges, cards, terminal blocks, tool calls, chat messages."/>
        <FeatureCard icon="Command" title="11 screens" desc="Every IronHermes surface modeled — memory, skills, cron, hooks, subagents, SOUL."/>
      </div>

      <div style={{marginTop: "var(--sp-10)"}}>
        <div className="t-label" style={{marginBottom: "var(--sp-4)"}}>principles</div>
        <div className="stack-lg">
          <Principle n="01" title="Mono is the default">Iosekey Mono for labels, metadata, code, and status. Sans only for prose and message bodies. Reach for condensed weights in display type — it earns the terminal feel without mono's kerning.</Principle>
          <Principle n="02" title="Every state has a dot">Live running, healthy, warning, error, idle. Status dots + semantic accents appear wherever a system is live. No emoji, no checkmarks-in-circles; a colored 6px circle does the job.</Principle>
          <Principle n="03" title="Density is a feature">This tool is used by engineers for hours. Lists show 6–8 items without scroll, tables use 32px row height, kbd hints live next to every action. Whitespace appears only where density breaks readability.</Principle>
          <Principle n="04" title="Rail, not border">Command blocks and tool calls use a 3px colored rail on the leading edge instead of full-surround borders. The rail carries state (running, ok, err) in your peripheral vision as you skim down a transcript.</Principle>
        </div>
      </div>
    </div>
  );
}

const FeatureCard = ({icon, title, desc}) => {
  const Ic = I[icon] || I.Mark;
  return (
    <div className="ih-card" style={{padding: "var(--sp-6)"}}>
      <span style={{color: "var(--accent)", display:"inline-block", marginBottom:"var(--sp-4)"}}><Ic size={20}/></span>
      <div className="t-h3" style={{marginBottom: "var(--sp-2)"}}>{title}</div>
      <div className="t-small fg-muted">{desc}</div>
    </div>
  );
};

const Principle = ({n, title, children}) => (
  <div style={{display:"grid", gridTemplateColumns: "80px 1fr", gap:"var(--sp-6)", padding: "var(--sp-4) 0", borderTop:"1px solid var(--border-subtle)"}}>
    <div className="mono fg-faint" style={{fontSize:"var(--fs-11)", fontWeight: 600}}>{n}</div>
    <div>
      <div className="t-h3" style={{marginBottom: "var(--sp-2)"}}>{title}</div>
      <div className="t-body fg-muted">{children}</div>
    </div>
  </div>
);

function BrandView() {
  return (
    <div style={{padding: "var(--sp-10) var(--sp-12)", maxWidth: 1100}}>
      <span className="t-label" style={{marginBottom:"var(--sp-3)"}}>brand</span>
      <h1 className="t-h1" style={{margin: "var(--sp-3) 0 var(--sp-2)"}}>Wordmark & mark</h1>
      <p className="t-body fg-muted" style={{maxWidth: "60ch"}}>Condensed mono wordmark. The "Hermes" half carries the accent. The square glyph is used at small sizes where the wordmark would be illegible — app icon, favicon, nav rail.</p>

      <div style={{display:"grid", gridTemplateColumns:"1fr 1fr", gap:"var(--sp-6)", marginTop:"var(--sp-8)"}}>
        <div className="ih-card" style={{padding:"var(--sp-9)", display:"grid", placeItems:"center"}}>
          <div className="ih-logo" style={{fontSize:"var(--fs-48)"}}>
            <span className="glyph" style={{width: 52, height: 52, borderRadius: "var(--r-4)", background:"var(--accent)", color:"var(--fg-on-accent)", display:"grid", placeItems:"center", fontFamily:"var(--font-mono-cond)", fontWeight: 700, fontSize: 28}}>IH</span>
            <span className="name" style={{fontSize:"var(--fs-36)"}}>Iron<span className="fe">Hermes</span></span>
          </div>
        </div>
        <div className="ih-card" style={{padding:"var(--sp-9)", display:"grid", placeItems:"center", background:"var(--fg)"}}>
          <div className="ih-logo" style={{fontSize:"var(--fs-48)", color:"var(--bg-canvas)"}}>
            <span style={{width: 52, height: 52, borderRadius: "var(--r-4)", background:"var(--bg-canvas)", color:"var(--accent)", display:"grid", placeItems:"center", fontFamily:"var(--font-mono-cond)", fontWeight: 700, fontSize: 28}}>IH</span>
            <span className="name" style={{fontSize:"var(--fs-36)", color:"var(--bg-canvas)"}}>Iron<span style={{color:"var(--accent)"}}>Hermes</span></span>
          </div>
        </div>
      </div>

      <div style={{marginTop:"var(--sp-9)"}}>
        <span className="t-label">glyph sizes</span>
        <div className="row gap-6" style={{marginTop:"var(--sp-4)", alignItems:"flex-end"}}>
          {[16, 20, 28, 40, 64, 96].map(s => (
            <div key={s} style={{textAlign:"center"}}>
              <div style={{width:s, height:s, borderRadius: Math.max(2, s/8), background:"var(--accent)", color:"var(--fg-on-accent)", display:"grid", placeItems:"center", fontFamily:"var(--font-mono-cond)", fontWeight:700, fontSize: Math.max(8, s/2.3)}}>IH</div>
              <div className="mono t-micro fg-faint" style={{marginTop: 6}}>{s}px</div>
            </div>
          ))}
        </div>
      </div>

      <div style={{marginTop:"var(--sp-9)"}}>
        <span className="t-label">voice</span>
        <div className="stack-lg" style={{marginTop:"var(--sp-4)"}}>
          <Voice good="session fts reindexing · 8,412 rows · 14.2mb · 3.8ms p95" bad="🚀 We're reindexing your sessions! This might take a moment ✨"/>
          <Voice good="context 72% · compress before next turn" bad="Heads up — you're approaching a soft limit on context!"/>
          <Voice good="provider:fallback from=anthropic to=openrouter cause=429" bad="Our systems are experiencing high load. Switching providers."/>
        </div>
      </div>
    </div>
  );
}

const Voice = ({good, bad}) => (
  <div style={{display:"grid", gridTemplateColumns:"1fr 1fr", gap:"var(--sp-4)"}}>
    <div>
      <span className="t-label" style={{color:"var(--success)", marginBottom:"var(--sp-2)", display:"block"}}>do</span>
      <div className="mono" style={{fontSize:"var(--fs-13)", padding:"var(--sp-3)", background:"var(--bg-surface)", border:"1px solid var(--border-subtle)", borderRadius:"var(--r-3)"}}>{good}</div>
    </div>
    <div>
      <span className="t-label" style={{color:"var(--danger)", marginBottom:"var(--sp-2)", display:"block"}}>don't</span>
      <div className="mono" style={{fontSize:"var(--fs-13)", padding:"var(--sp-3)", background:"var(--bg-surface)", border:"1px solid var(--border-subtle)", borderRadius:"var(--r-3)", color:"var(--fg-dim)", textDecoration:"line-through", textDecorationColor:"var(--danger)"}}>{bad}</div>
    </div>
  </div>
);

window.OverviewView = OverviewView;
window.BrandView = BrandView;
