// platforms-view.jsx — macOS, iOS, Web, TUI showcases

function _Stage({title, subtitle, children}) {
  return (
    <div className="ds-stage">
      <h1 className="ds-stage-title">{title}</h1>
      <p className="ds-stage-sub">{subtitle}</p>
      <div style={{display:"flex", gap: 32, flexWrap:"wrap", alignItems:"flex-start"}}>
        {children}
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// macOS window — primary chat surface
// ─────────────────────────────────────────────────────────────
function MacOSStage() {
  const sidebar = (
    <MacSidebar>
      <MacSidebarHeader title="Sessions"/>
      <MacSidebarItem label="refactor session storage" selected={true}/>
      <MacSidebarItem label="migrate fts5 schema"/>
      <MacSidebarItem label="benchmark tokio runtime"/>
      <MacSidebarItem label="debug hook timeout"/>
      <MacSidebarHeader title="Pinned"/>
      <MacSidebarItem label="MEMORY.md"/>
      <MacSidebarItem label="USER.md"/>
      <MacSidebarItem label="SOUL.md"/>
    </MacSidebar>
  );
  return (
    <_Stage title="IronHermes · macOS"
      subtitle="The primary surface. Liquid-glass sidebar for session navigation, chat stream in the main area, live status and tool-calls rendered inline.">
      <MacWindow width={1120} height={720} title="IronHermes — refactor session storage"
                 sidebar={sidebar}>
        <div style={{height: "100%", background:"var(--bg-canvas)", color:"var(--fg)", display:"flex", flexDirection:"column"}}>
          <div style={{padding:"var(--sp-5) var(--sp-6)", borderBottom:"1px solid var(--border-subtle)", display:"flex", alignItems:"center", gap: 12}}>
            <I.Chat size={14} style={{color:"var(--accent)"}}/>
            <span style={{fontWeight: 600}}>refactor session storage</span>
            <Badge tone="success" dot>live</Badge>
            <div style={{flex: 1}}/>
            <Badge square>sonnet-4.5</Badge>
            <Badge square>ctx 67%</Badge>
          </div>
          <div style={{flex: 1, overflow:"auto", padding:"var(--sp-6)"}}>
            <div className="stack-md">
              <Msg who="user" author="bw" time="14:03">
                Pick up where we left off — the session storage refactor. The fts5 migration was blocking.
              </Msg>
              <Msg who="agent" author="ironhermes" time="14:03">
                <p>Reading the last session memory and the migration file.</p>
                <ToolCall name="read_file" status="ok" duration="0.03s"
                  args={<><K c="path"/>=<S c="migrations/003_fts.sql"/></>}/>
                <ToolCall name="read_file" status="ok" duration="0.04s"
                  args={<><K c="path"/>=<S c=".ironhermes/MEMORY.md"/></>}/>
                <p>The <code className="inline">tokenize='porter'</code> option is unavailable on the bundled sqlite build. Switching to <code className="inline">unicode61 remove_diacritics 2</code>.</p>
                <ToolCall name="patch" status="running" duration="…"
                  args={<><K c="path"/>=<S c="migrations/003_fts.sql"/> <K c="hunks"/>=<N c="1"/></>}/>
              </Msg>
            </div>
          </div>
          <div style={{padding:"var(--sp-4) var(--sp-5)", borderTop:"1px solid var(--border-subtle)", display:"flex", gap: 8, alignItems:"center"}}>
            <I.Attach size={14} style={{color:"var(--fg-faint)"}}/>
            <Input placeholder="Continue the session…" style={{flex: 1}}/>
            <Btn variant="primary" icon={<I.Send size={12}/>}>Send</Btn>
          </div>
        </div>
      </MacWindow>
    </_Stage>
  );
}

// ─────────────────────────────────────────────────────────────
// iOS phone — companion app
// ─────────────────────────────────────────────────────────────
function IOSStage() {
  return (
    <_Stage title="IronHermes · iOS"
      subtitle="The mobile companion. Read-only session stream by default, with voice-dictation for quick inputs and push-notification alerts for long-running agents that need a decision.">
      <div className="row gap-8" style={{flexWrap:"wrap"}}>
        <IOSDevice dark={true} title="Sessions">
          <div style={{padding:"0 16px 16px"}}>
            <IOSList header="ACTIVE" dark={true}>
              <IOSListRow dark={true} icon={<I.Session size={16}/>} title="refactor session storage" detail="live"/>
              <IOSListRow dark={true} icon={<I.Session size={16}/>} title="migrate fts5 schema" detail="paused"/>
              <IOSListRow dark={true} icon={<I.Session size={16}/>} title="benchmark tokio" detail="done" isLast/>
            </IOSList>
            <IOSList header="PINNED" dark={true}>
              <IOSListRow dark={true} icon={<I.Memory size={16}/>} title="MEMORY.md" detail="67%"/>
              <IOSListRow dark={true} icon={<I.Soul size={16}/>} title="SOUL.md" detail="" isLast/>
            </IOSList>
          </div>
        </IOSDevice>

        <IOSDevice dark={true} title="Chat">
          <div style={{padding:"8px 12px", color:"#fff", fontFamily:"var(--font-sans)"}}>
            <div style={{marginBottom: 12}}>
              <div style={{fontSize: 11, color:"rgba(255,255,255,0.5)", fontFamily:"var(--font-mono)", letterSpacing:"0.08em", textTransform:"uppercase", marginBottom: 4}}>you · 14:03</div>
              <div style={{background:"#0A84FF", padding:"10px 14px", borderRadius: 18, fontSize: 15, display:"inline-block", maxWidth:"80%"}}>
                Pick up where we left off — the fts5 migration was blocking.
              </div>
            </div>
            <div>
              <div style={{fontSize: 11, color:"rgba(255,255,255,0.5)", fontFamily:"var(--font-mono)", letterSpacing:"0.08em", textTransform:"uppercase", marginBottom: 4}}>ironhermes · 14:03</div>
              <div style={{background:"#1C1C1E", padding:"10px 14px", borderRadius: 18, fontSize: 14, maxWidth:"90%", border:"1px solid rgba(255,255,255,0.08)"}}>
                The tokenize='porter' option is unavailable on the bundled sqlite. Switching to unicode61.
                <div style={{marginTop: 8, padding:"6px 10px", background:"rgba(255,255,255,0.04)", borderRadius: 8, fontFamily:"var(--font-mono)", fontSize: 11, color:"rgba(255,255,255,0.7)"}}>
                  <span style={{color:"#32D74B"}}>●</span> patch · migrations/003_fts.sql
                </div>
              </div>
            </div>
          </div>
        </IOSDevice>
      </div>
    </_Stage>
  );
}

// ─────────────────────────────────────────────────────────────
// Web browser — dashboard / gateway admin
// ─────────────────────────────────────────────────────────────
function WebStage() {
  return (
    <_Stage title="IronHermes · Web"
      subtitle="The gateway admin dashboard. Multi-user, multi-tenant view of running agents, costs, and hook activity across a team.">
      <ChromeWindow
        tabs={[{title:"IronHermes · Dashboard"}, {title:"Docs"}]}
        activeIndex={0}
        url="ironhermes.dev/admin"
        width={1120}
        height={680}>
        <div style={{background:"var(--bg-canvas)", color:"var(--fg)", height:"100%", padding:"var(--sp-7) var(--sp-8)", overflow:"auto"}}>
          <div style={{display:"flex", alignItems:"center", marginBottom:"var(--sp-6)"}}>
            <h2 style={{margin: 0, fontFamily:"var(--font-mono-cond)", fontWeight: 700, fontSize: 24}}>Fleet overview</h2>
            <div style={{flex: 1}}/>
            <Segmented value="24h" options={[{value:"1h",label:"1h"},{value:"24h",label:"24h"},{value:"7d",label:"7d"}]}/>
          </div>

          <div style={{display:"grid", gridTemplateColumns:"repeat(4, 1fr)", gap:"var(--sp-4)", marginBottom:"var(--sp-6)"}}>
            {[
              {k:"Active agents", v:"12", meta:"+2 from yesterday"},
              {k:"Tokens today",  v:"2.4M", meta:"$9.82 spend"},
              {k:"Tool calls",    v:"841", meta:"94% success"},
              {k:"Hook events",   v:"1.2k", meta:"3 failures"},
            ].map((c,i) => (
              <div key={i} className="ih-card" style={{padding:"var(--sp-5)"}}>
                <div className="t-label">{c.k}</div>
                <div style={{fontFamily:"var(--font-mono-cond)", fontWeight:700, fontSize: 30, marginTop: 8}}>{c.v}</div>
                <div className="t-small fg-muted">{c.meta}</div>
              </div>
            ))}
          </div>

          <div className="ih-card">
            <div style={{padding:"var(--sp-4) var(--sp-5)", borderBottom:"1px solid var(--border-subtle)", display:"flex", alignItems:"center"}}>
              <span style={{fontWeight: 600}}>Running sessions</span>
              <div style={{flex: 1}}/>
              <Input placeholder="filter…" prefix={<I.Search size={11}/>} style={{width: 240}}/>
            </div>
            <div className="ih-listing">
              {[
                {u:"bw",     s:"refactor session storage", m:"sonnet-4.5", t:"3m",  st:"success", tk:"12.3k"},
                {u:"jane",   s:"write migration for fts5", m:"sonnet-4.5", t:"14m", st:"warning", tk:"8.1k"},
                {u:"ops",    s:"nightly cron · cleanup",   m:"haiku-4.5",  t:"1h",  st:"success", tk:"2.4k"},
                {u:"marcus", s:"debug hook timeout",       m:"opus-4.5",   t:"2h",  st:"danger",  tk:"41.2k"},
                {u:"bw",     s:"benchmark tokio",          m:"sonnet-4.5", t:"4h",  st:"success", tk:"4.1k"},
              ].map((r,i,arr) => (
                <div key={i} className="row gap-4" style={{padding:"var(--sp-3) var(--sp-5)", borderBottom: i === arr.length-1 ? 0 : "1px solid var(--border-subtle)"}}>
                  <span className="mono t-small fg-faint" style={{width: 56}}>{r.u}</span>
                  <span style={{flex: 1}}>{r.s}</span>
                  <Badge square>{r.m}</Badge>
                  <span className="mono t-small fg-faint" style={{width: 56, textAlign:"right"}}>{r.tk}</span>
                  <Badge tone={r.st} dot square>{r.st}</Badge>
                  <span className="mono t-small fg-faint" style={{width: 36, textAlign:"right"}}>{r.t}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </ChromeWindow>
    </_Stage>
  );
}

// ─────────────────────────────────────────────────────────────
// Pure TUI — terminal-only view
// ─────────────────────────────────────────────────────────────
function TuiStage() {
  return (
    <_Stage title="IronHermes · TUI"
      subtitle="Pure terminal mode. Ratatui-style panes. Every action keyboard-driven, every status shown in a single statusline at the bottom.">
      <div className="ds-tui" style={{width: 880, maxWidth: "100%"}}>
        <div><span className="c-dim">┌───────────────────────────────────────────────────────────────────────────┐</span></div>
        <div><span className="c-dim">│</span> <span className="bold c-bright">IronHermes</span> <span className="c-cyan">v0.1.0</span>  <span className="c-dim">sonnet-4.5</span>  <span className="c-dim">~/code/ironhermes</span>                  <span className="c-dim">│</span></div>
        <div><span className="c-dim">├───────────────────────────────────────────────────────────────────────────┤</span></div>
        <div><span className="c-dim">│</span> <span className="c-magenta">❯</span> <span className="c-bright">user</span> · 14:03                                                          <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>   Pick up the session storage refactor. The fts5 migration was blocking.  <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>                                                                           <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span> <span className="c-cyan">❯</span> <span className="bold c-bright">ironhermes</span> · 14:03                                                  <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>   Reading session memory and migration files.                             <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>                                                                           <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>   <span className="c-green">●</span> <span className="bold">read_file</span>  <span className="c-dim">path=</span><span className="c-yellow">"migrations/003_fts.sql"</span>     <span className="c-dim">0.03s</span>     <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>   <span className="c-green">●</span> <span className="bold">read_file</span>  <span className="c-dim">path=</span><span className="c-yellow">".ironhermes/MEMORY.md"</span>      <span className="c-dim">0.04s</span>     <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>                                                                           <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>   The <span className="c-cyan">tokenize='porter'</span> option is unavailable on bundled sqlite.         <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>   Switching to <span className="c-cyan">unicode61 remove_diacritics 2</span>.                          <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>                                                                           <span className="c-dim">│</span></div>
        <div><span className="c-dim">│</span>   <span className="c-yellow">◐</span> <span className="bold">patch</span>      <span className="c-dim">path=</span><span className="c-yellow">"migrations/003_fts.sql"</span> <span className="c-dim">hunks=1</span> <span className="c-dim">…</span>       <span className="c-dim">│</span></div>
        <div><span className="c-dim">├───────────────────────────────────────────────────────────────────────────┤</span></div>
        <div><span className="c-dim">│</span> <span className="c-magenta">›</span> <span className="c-bright">_</span>                                                                       <span className="c-dim">│</span></div>
        <div><span className="c-dim">└───────────────────────────────────────────────────────────────────────────┘</span></div>
        <div style={{marginTop: 6}}>
          <span className="c-green">●</span> <span className="c-bright">live</span>  <span className="c-dim">·</span>  <span className="c-cyan">ctx 67%</span>  <span className="c-dim">·</span>  <span className="c-dim">$0.413</span>  <span className="c-dim">·</span>  <span className="c-dim">turn 3/20</span>  <span className="c-dim">·</span>  <span className="c-dim">⏎ send  ⌃c cancel  ? help</span>
        </div>
      </div>
    </_Stage>
  );
}

window.MacOSStage = MacOSStage;
window.IOSStage = IOSStage;
window.WebStage = WebStage;
window.TuiStage = TuiStage;
