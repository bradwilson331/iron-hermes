// ih-screens.jsx — IronHermes feature screens
// Renders each complete screen for the design system showcase.
// Needs: icons.jsx, ih-components.jsx, components.css, tokens.css

const { useState: useS2, useEffect: useE2 } = React;

// ============================================================
// CHAT SCREEN — primary surface
// ============================================================
function ChatScreen() {
  return (
    <div style={{display:"grid", gridTemplateColumns:"240px 1fr 320px", height:"100%", background:"var(--bg-canvas)", color:"var(--fg)"}}>
      {/* Left: sessions */}
      <aside style={{borderRight:"1px solid var(--border-subtle)", padding:"var(--sp-4)", overflow:"auto", background:"var(--bg-surface)"}} className="ih-scroll">
        <div className="row gap-3" style={{marginBottom:"var(--sp-5)"}}>
          <Logo/>
          <Badge tone="success" dot>online</Badge>
        </div>
        <Input prefix={<I.Search size={12}/>} placeholder="Search sessions…" suffix={<Kbd>⌘K</Kbd>} style={{width:"100%"}}/>
        <div style={{height:"var(--sp-5)"}}/>
        <div className="t-label" style={{padding:"0 var(--sp-3) var(--sp-2)"}}>pinned</div>
        <SessionItem title="v2.0 phase planning" preview="Split phase 22 into three…" active badge="hot"/>
        <SessionItem title="Context compression tuning" preview="Iterative re-compression is" />
        <div className="t-label" style={{padding:"var(--sp-5) var(--sp-3) var(--sp-2)"}}>today · 4</div>
        <SessionItem title="/agent cron for retrospectives" preview="Scheduled for sunday 20:00"/>
        <SessionItem title="MEMORY.md capacity review" preview="1,474 / 2,200 chars used"/>
        <SessionItem title="Grafeo provider bench" preview="SQLite: 8.2ms · Grafeo: 11.1ms"/>
        <SessionItem title="Telegram gateway restart" preview="PID 40192 terminated ok"/>
        <div className="t-label" style={{padding:"var(--sp-5) var(--sp-3) var(--sp-2)"}}>yesterday</div>
        <SessionItem title="SOUL.md personality draft"/>
        <SessionItem title="Skills hub trust levels"/>
      </aside>

      {/* Center: chat transcript */}
      <main style={{display:"flex", flexDirection:"column", minHeight:0, overflow:"hidden"}}>
        <header style={{padding:"var(--sp-4) var(--sp-6)", borderBottom:"1px solid var(--border-subtle)", display:"flex", alignItems:"center", gap:"var(--sp-4)", background:"var(--bg-surface)"}}>
          <div className="col gap-1" style={{flex:1}}>
            <div className="row gap-3"><span className="t-h3">v2.0 phase planning</span><Badge dot>telegram · dm</Badge></div>
            <div className="t-micro fg-dim">session:agent:main:telegram:dm:483209 · 32 turns · 14,312 tokens</div>
          </div>
          <Btn variant="ghost" size="sm" icon={<I.Branch size={12}/>}>fork</Btn>
          <Btn variant="ghost" size="sm" icon={<I.Copy size={12}/>}>export</Btn>
          <Btn variant="ghost" size="sm" square icon={<I.More size={14}/>}/>
        </header>

        <div className="ih-scroll" style={{flex:1, overflow:"auto", padding:"var(--sp-5) var(--sp-8)"}}>
          <Msg who="user" author="brad" time="14:22">
            <p>can you take a look at the retrospective from phase 22 and tell me what i missed in the split? also run the workspace tests for the cli crate.</p>
          </Msg>

          <Msg who="agent" time="14:22">
            <div className="row gap-2" style={{marginBottom:"var(--sp-3)"}}>
              <I.Thought size={12} style={{color:"var(--fg-dim)"}}/>
              <span className="t-micro fg-dim">thinking · 1.2s</span>
            </div>
            <p>I'll read the retrospective first and then run tests on <code className="inline">ironhermes-cli</code>.</p>
          </Msg>

          <div style={{paddingLeft: 36, marginBottom: "var(--sp-5)"}}>
            <ToolCall name="read_file" status="ok" duration="12ms"
              args={<span><K c="path"/>: <S c=".planning/phases/22.RETROSPECTIVE.md"/></span>}/>
          </div>

          <div style={{paddingLeft: 36, marginBottom: "var(--sp-5)"}}>
            <ToolCall name="terminal" status="run"
              args={<span><K c="cmd"/>: <S c="cargo test -p ironhermes-cli"/>, <K c="timeout"/>: <N c="120"/></span>}>
              <pre style={{margin:0, fontSize:"var(--fs-11)", color:"var(--fg-muted)", whiteSpace:"pre-wrap"}}>{`   Compiling ironhermes-cli v2.0.0
    Finished test [unoptimized] in 4.82s
     Running unittests src/lib.rs
running 47 tests
....`}<span className="ih-caret"/></pre>
            </ToolCall>
          </div>

          <Msg who="agent" time="14:23">
            <p>Phase 22 split looks clean on the surface, but you glossed over one thing: <strong>22.2 carries the ACP session lineage contract</strong> and it needs a new hook event (<code className="inline">session:fork</code>) that doesn't exist in GW-06 yet.</p>
            <p>Three items worth lifting into 22.3:</p>
            <ul style={{margin:"0 0 var(--sp-4) var(--sp-6)"}}>
              <li>Fork event emission across parent/child session chain</li>
              <li>FTS5 snippet generation for <em>forked</em> sessions (parent context inclusion)</li>
              <li>CLI TUI indicator for active fork parent</li>
            </ul>
          </Msg>

          <Msg who="user" time="14:24"><p>good catch. draft the 22.3 plan.</p></Msg>
        </div>

        {/* Composer */}
        <footer style={{padding:"var(--sp-4) var(--sp-6) var(--sp-5)", borderTop:"1px solid var(--border-subtle)", background:"var(--bg-surface)"}}>
          <div style={{background:"var(--bg-elevated)", border:"1px solid var(--border)", borderRadius:"var(--r-4)", padding:"var(--sp-3) var(--sp-4)"}}>
            <div className="row gap-2" style={{marginBottom:"var(--sp-3)"}}>
              <Badge tone="accent">/agent</Badge>
              <span className="t-micro fg-dim">main · claude-3.7-sonnet · prompt-cache: 85%</span>
              <span style={{marginLeft:"auto"}}><Badge>cwd: ironhermes/</Badge></span>
            </div>
            <div style={{minHeight: 48, color:"var(--fg)", fontSize:"var(--fs-14)"}}>draft the 22.3 plan with session:fork hook…<span className="ih-caret"/></div>
            <div className="row gap-2" style={{marginTop:"var(--sp-3)"}}>
              <Btn variant="ghost" size="sm" square icon={<I.Attach size={12}/>}/>
              <Btn variant="ghost" size="sm" square icon={<I.Mic size={12}/>}/>
              <Btn variant="ghost" size="sm" icon={<I.Command size={12}/>}>commands</Btn>
              <div style={{flex:1}}/>
              <span className="t-micro fg-faint">⇧⏎ newline · ⌘⏎ send</span>
              <Btn variant="primary" size="sm" icon={<I.Send size={12}/>}>send</Btn>
            </div>
          </div>
        </footer>
      </main>

      {/* Right: inspector */}
      <aside style={{borderLeft:"1px solid var(--border-subtle)", padding:"var(--sp-5) var(--sp-5)", overflow:"auto", background:"var(--bg-surface)"}} className="ih-scroll">
        <div className="t-label">context pressure</div>
        <div style={{height:"var(--sp-3)"}}/>
        <Progress value={72} tone="warning" label="16k / 22k tokens" suffix="72%"/>
        <div style={{height:"var(--sp-5)"}}/>
        <div className="t-label">memory snapshot</div>
        <div style={{height:"var(--sp-3)"}}/>
        <div className="ih-card">
          <div className="ih-card-head" style={{padding:"var(--sp-3) var(--sp-4)"}}>
            <span className="mono" style={{fontSize:"var(--fs-12)"}}>MEMORY.md</span>
            <Badge tone="accent">67%</Badge>
          </div>
          <div className="ih-card-body" style={{padding:"var(--sp-4)"}}>
            <div className="stack-sm">
              <MemoryLine text="User prefers concise bullet responses over prose."/>
              <MemoryLine text="Rust 2024 edition. Tokio async. No Python bridges."/>
              <MemoryLine text="SOUL.md slot 1 — frozen snapshot; never mutate mid-session."/>
              <MemoryLine text="Telegram gateway runs as systemd unit `hermes.service`."/>
            </div>
          </div>
        </div>
        <div style={{height:"var(--sp-5)"}}/>
        <div className="t-label">active skills</div>
        <div style={{height:"var(--sp-3)"}}/>
        <div className="col gap-2">
          <SkillChip name="phase-planner" cat="project"/>
          <SkillChip name="rust-workspace" cat="dev"/>
          <SkillChip name="retrospective" cat="project"/>
        </div>
        <div style={{height:"var(--sp-5)"}}/>
        <div className="t-label">recent tools</div>
        <div style={{height:"var(--sp-3)"}}/>
        <div className="col gap-2">
          <ToolStat name="read_file" count={14}/>
          <ToolStat name="terminal" count={3}/>
          <ToolStat name="patch" count={2}/>
          <ToolStat name="memory.add" count={1}/>
        </div>
      </aside>
    </div>
  );
}

const SessionItem = ({title, preview, active, badge}) => (
  <div style={{
    padding:"var(--sp-3) var(--sp-3)", borderRadius:"var(--r-3)",
    background: active ? "var(--bg-selected)" : "transparent",
    cursor:"default", marginBottom: 2,
  }}>
    <div className="row gap-2">
      <span style={{color: active ? "var(--accent)" : "var(--fg-faint)"}}><I.Session size={12}/></span>
      <span className="truncate" style={{flex:1, fontSize:"var(--fs-13)", color:active ? "var(--fg)" : "var(--fg-muted)", fontWeight: active ? 500 : 400}}>{title}</span>
      {badge && <span style={{width:6, height:6, borderRadius:999, background:"var(--accent)"}}/>}
    </div>
    {preview && <div className="truncate t-micro fg-faint" style={{paddingLeft:18, marginTop:2}}>{preview}</div>}
  </div>
);

const MemoryLine = ({text}) => (
  <div style={{display:"flex", gap:"var(--sp-3)", fontSize:"var(--fs-12)", color:"var(--fg-muted)"}}>
    <span className="fg-faint mono" style={{flex:"none"}}>·</span>
    <span style={{lineHeight: 1.45}}>{text}</span>
  </div>
);

const SkillChip = ({name, cat}) => (
  <div className="row gap-2" style={{padding:"var(--sp-2) var(--sp-3)", background:"var(--bg-elevated)", border:"1px solid var(--border-subtle)", borderRadius:"var(--r-2)"}}>
    <I.Skill size={12} style={{color:"var(--accent)"}}/>
    <span className="mono" style={{fontSize:"var(--fs-12)"}}>{name}</span>
    <span style={{marginLeft:"auto"}}><Badge>{cat}</Badge></span>
  </div>
);

const ToolStat = ({name, count}) => (
  <div className="row gap-2" style={{fontSize:"var(--fs-12)"}}>
    <span className="mono fg-muted" style={{flex:1}}>{name}</span>
    <span className="mono fg-faint mono-num">×{count}</span>
  </div>
);

// ============================================================
// TERMINAL — full emulator screen
// ============================================================
function TerminalScreen() {
  return (
    <div style={{background:"var(--bg-canvas)", color:"var(--fg)", height:"100%", display:"flex", flexDirection:"column"}}>
      {/* tab bar */}
      <div style={{display:"flex", alignItems:"center", borderBottom:"1px solid var(--border-subtle)", background:"var(--bg-surface)", padding:"0 var(--sp-4)", height: 34, gap:"var(--sp-3)"}}>
        <span className="mono" style={{fontSize:"var(--fs-11)", color:"var(--fg-faint)"}}>tabs</span>
        <div style={{display:"flex", gap: 2}}>
          <TermTab label="~/ironhermes" active/>
          <TermTab label="cargo watch"/>
          <TermTab label="hermes agent"/>
        </div>
        <Btn variant="ghost" size="sm" square icon={<I.Plus size={12}/>}/>
        <div style={{flex:1}}/>
        <Status kind="live">agent · claude-3.7-sonnet</Status>
      </div>

      <div className="ih-scroll" style={{flex:1, overflow:"auto", padding:"var(--sp-6) var(--sp-8)", fontFamily:"var(--font-mono)"}}>
        <div className="stack">
          <CmdBlock cwd="~/ironhermes" cmd="cargo build --release" state="ok" duration="8.41s">
            <div className="fg-muted">    Finished <span className="fg-success">`release`</span> profile [optimized] target(s) in 8.41s</div>
          </CmdBlock>

          <CmdBlock cwd="~/ironhermes" cmd="git status" state="ok" duration="0.02s">
            <div>On branch <span className="fg-accent">v2.0/phase-22.2</span></div>
            <div className="fg-muted">Your branch is up to date with 'origin/v2.0/phase-22.2'.</div>
            <div style={{height: 6}}/>
            <div>Changes not staged for commit:</div>
            <div className="fg-danger">  modified:   crates/ironhermes-cli/src/acp.rs</div>
            <div className="fg-danger">  modified:   crates/ironhermes-cli/src/session.rs</div>
            <div style={{height: 6}}/>
            <div>Untracked files:</div>
            <div className="fg-warning">  .planning/phases/22.3-PLAN.md</div>
          </CmdBlock>

          {/* Inline agent invocation */}
          <div className="ih-cmd" data-state="run">
            <div className="rail"/>
            <div className="content">
              <div className="prompt-row">
                <span className="glyph" style={{color:"var(--accent)"}}>/agent</span>
                <span className="cmd">draft phase 22.3 plan focusing on session:fork hook</span>
                <span className="meta">streaming…</span>
              </div>
              <div className="stdout">
                <div style={{padding:"var(--sp-3) 0", fontFamily:"var(--font-ui)", color:"var(--fg)", fontSize:"var(--fs-13)"}}>
                  I'll lift three items from the 22.2 retrospective into a new 22.3 phase:
                  <div style={{height: 6}}/>
                  <div>  1. Emit <span className="mono fg-accent">session:fork</span> hook event on parent→child split</div>
                  <div>  2. FTS5 snippet generation across forked session boundaries</div>
                  <div>  3. CLI TUI active-fork indicator in status bar<span className="ih-caret"/></div>
                </div>
              </div>
            </div>
          </div>

          <CmdBlock cwd="~/ironhermes" cmd="hermes skills list --active" state="ok" duration="0.14s">
            <div className="fg-muted mono-num">FOUND 7 ACTIVE SKILLS ───────────────────────</div>
            <div><span className="fg-success">●</span> phase-planner         <span className="fg-faint">project    ·  auto-activated by .hermes.md</span></div>
            <div><span className="fg-success">●</span> rust-workspace        <span className="fg-faint">dev        ·  requires: cargo, rustc</span></div>
            <div><span className="fg-success">●</span> retrospective         <span className="fg-faint">project    ·  conditional</span></div>
            <div><span className="fg-success">●</span> session-search        <span className="fg-faint">memory     ·  requires: state.db</span></div>
            <div><span className="fg-dim">○</span>   skills-hub            <span className="fg-faint">meta       ·  missing HERMES_HUB_TOKEN</span></div>
          </CmdBlock>

          {/* Current prompt */}
          <div style={{display:"flex", alignItems:"center", gap:"var(--sp-3)", padding:"var(--sp-3) var(--sp-1)", fontSize:"var(--fs-13)"}}>
            <span className="fg-accent">›</span>
            <span className="fg-dim">~/ironhermes</span>
            <span className="fg-muted">on <span className="fg-accent">v2.0/phase-22.2</span></span>
            <span><span className="ih-caret"/></span>
          </div>
        </div>
      </div>

      <div style={{borderTop:"1px solid var(--border-subtle)", padding:"var(--sp-2) var(--sp-4)", background:"var(--bg-surface)", display:"flex", alignItems:"center", gap:"var(--sp-4)", fontSize:"var(--fs-11)", color:"var(--fg-dim)"}} className="mono">
        <span>zsh</span><span>·</span><span>utf-8</span><span>·</span><span>80×24</span>
        <span style={{flex:1}}/>
        <span>mem 1474/2200</span><span>·</span><span>ctx 72%</span><span>·</span><span>⌘K commands</span>
      </div>
    </div>
  );
}

const TermTab = ({label, active}) => (
  <div style={{
    padding:"var(--sp-2) var(--sp-4)", borderRadius:"var(--r-2) var(--r-2) 0 0",
    background: active ? "var(--bg-elevated)" : "transparent",
    color: active ? "var(--fg)" : "var(--fg-muted)",
    fontFamily:"var(--font-mono)", fontSize:"var(--fs-11)",
    borderBottom: active ? "none" : "1px solid transparent",
    display:"flex", alignItems:"center", gap: 6,
  }}>
    <I.Terminal size={11} style={{color: active ? "var(--accent)" : "var(--fg-faint)"}}/>
    {label}
  </div>
);

window.ChatScreen = ChatScreen;
window.TerminalScreen = TerminalScreen;
