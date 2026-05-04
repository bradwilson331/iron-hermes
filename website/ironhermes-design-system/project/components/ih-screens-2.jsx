// ih-screens-2.jsx — Remaining feature screens
// Needs: icons.jsx, ih-components.jsx, ih-screens.jsx

// ============================================================
// MEMORY VIEWER
// ============================================================
function MemoryScreen() {
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">Memory</span>
        <Badge>bounded stores</Badge>
        <div style={{flex:1}}/>
        <Segmented options={[{value:"mem",label:"MEMORY.md"},{value:"usr",label:"USER.md"},{value:"prov",label:"provider"}]} value="mem"/>
      </div>
      <div style={{display:"grid", gridTemplateColumns:"1fr 320px", gap:"var(--sp-6)"}}>
        <div className="ih-card">
          <div className="ih-card-head">
            <div className="row gap-3">
              <I.Memory size={14} style={{color:"var(--accent)"}}/>
              <span className="mono" style={{fontSize:"var(--fs-13)"}}>~/.ironhermes/MEMORY.md</span>
              <Badge tone="success" dot>synced</Badge>
            </div>
            <div className="row gap-3">
              <span className="t-micro fg-dim mono-num">1,474 / 2,200 chars</span>
              <Btn variant="ghost" size="sm" icon={<I.Edit size={12}/>}>edit</Btn>
            </div>
          </div>
          <div className="ih-card-body">
            <Progress value={67} tone="warning"/>
            <div style={{height:"var(--sp-5)"}}/>
            <div className="stack">
              <MemEntry hash="a3f2" added="3 days ago" text="User prefers concise bullet responses over prose; ≤4 items per bullet list."/>
              <MemEntry hash="c7d1" added="2 days ago" text="IronHermes ships as a single Rust binary. No Python bridges. cargo build --release is the release command."/>
              <MemEntry hash="9e44" added="yesterday" text="Telegram gateway runs as systemd unit `hermes.service` on vps-01. Restart via `systemctl restart hermes`."/>
              <MemEntry hash="1a08" added="12h ago" text="Phase 22 split into 22/22.1/22.2. ACP work lives in 22.2."/>
              <MemEntry hash="5fd2" added="45m ago" text="SOUL.md is a frozen snapshot — never mutate mid-session or Anthropic prompt cache invalidates."/>
            </div>
          </div>
          <div className="ih-card-foot">
            <Btn variant="ghost" size="sm">auto-prune</Btn>
            <Btn variant="ghost" size="sm" icon={<I.Search size={12}/>}>search</Btn>
            <Btn size="sm" icon={<I.Plus size={12}/>}>add entry</Btn>
          </div>
        </div>

        <div className="col gap-4">
          <div className="ih-card">
            <div className="ih-card-head"><span className="mono" style={{fontSize:"var(--fs-12)"}}>USER.md</span><Badge>42%</Badge></div>
            <div className="ih-card-body">
              <Progress value={42}/>
              <div style={{height:"var(--sp-4)"}}/>
              <div className="mono" style={{fontSize:"var(--fs-11)", color:"var(--fg-muted)", lineHeight:1.6}}>
                <div>· brad — ironhermes lead</div>
                <div>· timezone: america/los_angeles</div>
                <div>· editor: zed</div>
                <div>· preferred model: claude-3.7-sonnet</div>
              </div>
            </div>
          </div>
          <div className="ih-card">
            <div className="ih-card-head"><span className="mono" style={{fontSize:"var(--fs-12)"}}>provider</span></div>
            <div className="ih-card-body stack-sm">
              <div className="row gap-2"><Badge tone="accent">sqlite</Badge><span className="t-small fg-muted">active</span></div>
              <div className="row gap-2"><Badge>grafeo</Badge><span className="t-small fg-faint">available</span></div>
              <div className="row gap-2"><Badge>duckdb</Badge><span className="t-small fg-faint">feature-gated</span></div>
            </div>
          </div>
          <div className="ih-card">
            <div className="ih-card-head"><span className="mono" style={{fontSize:"var(--fs-12)"}}>fts5 index</span></div>
            <div className="ih-card-body">
              <div className="stack-sm mono" style={{fontSize:"var(--fs-11)", color:"var(--fg-muted)"}}>
                <div className="row" style={{justifyContent:"space-between"}}><span>rows</span><span className="mono-num fg">8,412</span></div>
                <div className="row" style={{justifyContent:"space-between"}}><span>size</span><span className="mono-num fg">14.2 mb</span></div>
                <div className="row" style={{justifyContent:"space-between"}}><span>p95 query</span><span className="mono-num fg">3.8 ms</span></div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

const MemEntry = ({hash, added, text}) => (
  <div style={{padding:"var(--sp-4)", background:"var(--bg-surface)", border:"1px solid var(--border-subtle)", borderRadius:"var(--r-3)"}}>
    <div className="row gap-3" style={{marginBottom:"var(--sp-2)"}}>
      <span className="mono t-micro fg-faint">#{hash}</span>
      <span className="t-micro fg-dim">{added}</span>
      <div style={{flex:1}}/>
      <Btn variant="ghost" size="sm" square icon={<I.Edit size={11}/>}/>
      <Btn variant="ghost" size="sm" square icon={<I.Trash size={11}/>}/>
    </div>
    <div style={{fontSize:"var(--fs-13)", color:"var(--fg)"}}>{text}</div>
  </div>
);

// ============================================================
// SKILLS BROWSER
// ============================================================
function SkillsScreen() {
  const skills = [
    { name: "phase-planner", cat: "project", ver:"1.4.0", active:true, desc:"Drafts GSD phase plans with transitions, retrospectives, and milestone boundaries.", tools:["read_file","patch","todo"] },
    { name: "rust-workspace", cat: "dev", ver:"2.1.0", active:true, desc:"Workspace-aware cargo operations: build, test, clippy with crate filtering.", tools:["terminal"] },
    { name: "retrospective", cat: "project", ver:"0.9.1", active:true, desc:"Generates phase retrospectives following the 5-column format.", tools:["read_file","write_file"] },
    { name: "session-search", cat: "memory", ver:"1.0.0", active:true, desc:"FTS5-backed session search with snippet generation and date filters.", tools:["session_search"] },
    { name: "skills-hub", cat: "meta", ver:"0.3.0", active:false, desc:"Install and publish skills to external repositories.", tools:[], missing:"HERMES_HUB_TOKEN" },
    { name: "telegram-admin", cat: "gateway", ver:"1.0.0", active:false, desc:"Administrative commands for the Telegram gateway runtime.", tools:["terminal"], platform:"linux" },
    { name: "code-review", cat: "dev", ver:"0.7.0", active:true, desc:"Structured code review with severity, category, and fix suggestions.", tools:["read_file","search_files"] },
    { name: "web-scrape", cat: "web", ver:"1.2.1", active:true, desc:"Firecrawl-backed scraping with local SSRF-safe fallback.", tools:["web_search"] },
  ];
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">Skills</span>
        <Badge>8 total · 6 active</Badge>
        <div style={{flex:1}}/>
        <Input prefix={<I.Search size={12}/>} placeholder="Filter skills…" style={{width: 240}}/>
        <Btn size="sm" icon={<I.Plus size={12}/>}>install</Btn>
      </div>
      <div className="ih-tabs" style={{marginBottom:"var(--sp-5)"}}>
        <div className="ih-tab" aria-selected="true">all <Badge>8</Badge></div>
        <div className="ih-tab">project <Badge>2</Badge></div>
        <div className="ih-tab">dev <Badge>2</Badge></div>
        <div className="ih-tab">memory <Badge>1</Badge></div>
        <div className="ih-tab">gateway <Badge>1</Badge></div>
        <div className="ih-tab">meta <Badge>1</Badge></div>
        <div className="ih-tab">web <Badge>1</Badge></div>
      </div>
      <div style={{display:"grid", gridTemplateColumns:"repeat(2, 1fr)", gap:"var(--sp-4)"}}>
        {skills.map(s => <SkillCard key={s.name} {...s}/>)}
      </div>
    </div>
  );
}

const SkillCard = ({name, cat, ver, active, desc, tools, missing, platform}) => (
  <div className="ih-card" style={{padding:"var(--sp-5)"}}>
    <div className="row gap-3" style={{marginBottom:"var(--sp-3)"}}>
      <span style={{color: active ? "var(--accent)" : "var(--fg-faint)"}}><I.Skill size={16}/></span>
      <span className="mono" style={{fontSize:"var(--fs-14)", fontWeight:600}}>{name}</span>
      <span className="t-micro fg-faint mono-num">v{ver}</span>
      <div style={{flex:1}}/>
      <Badge>{cat}</Badge>
      {active ? <Toggle checked={true}/> : <Toggle checked={false}/>}
    </div>
    <div className="t-small fg-muted" style={{marginBottom:"var(--sp-3)"}}>{desc}</div>
    <div className="row gap-2" style={{flexWrap:"wrap"}}>
      {tools.length>0 && tools.map(t => <Badge key={t} square>{t}</Badge>)}
      {missing && <Badge tone="warning" dot>missing {missing}</Badge>}
      {platform && <Badge tone="info">{platform} only</Badge>}
    </div>
  </div>
);

// ============================================================
// CRON / SCHEDULED TASKS
// ============================================================
function CronScreen() {
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">Scheduled tasks</span>
        <div style={{flex:1}}/>
        <Btn size="sm" icon={<I.Plus size={12}/>}>new task</Btn>
      </div>
      <div className="ih-card">
        <table style={{width:"100%", borderCollapse:"collapse", fontSize:"var(--fs-13)"}}>
          <thead>
            <tr style={{textAlign:"left", color:"var(--fg-dim)"}}>
              {["","name","schedule","next run","platform","last status",""].map((h,i)=>
                <th key={i} style={{padding:"var(--sp-3) var(--sp-4)", fontWeight:500, fontSize:"var(--fs-11)", textTransform:"uppercase", letterSpacing:"0.08em", borderBottom:"1px solid var(--border-subtle)"}}>{h}</th>
              )}
            </tr>
          </thead>
          <tbody>
            <CronRow enabled name="daily-retrospective" schedule="0 20 * * 0" next="sun 20:00" plat="telegram" status="ok" last="6h ago"/>
            <CronRow enabled name="memory-prune" schedule="0 3 * * *" next="tomorrow 03:00" plat="—" status="ok" last="21h ago"/>
            <CronRow enabled name="session-fts-reindex" schedule="0 4 * * 0" next="sun 04:00" plat="—" status="ok" last="6d ago"/>
            <CronRow enabled={false} name="news-digest" schedule="0 9 * * 1-5" next="paused" plat="telegram" status="warn" last="3d ago"/>
            <CronRow enabled name="soul-drift-check" schedule="*/30 * * * *" next="in 12 min" plat="—" status="ok" last="18 min ago"/>
            <CronRow enabled name="context-compression-stats" schedule="@hourly" next="in 32 min" plat="—" status="err" last="32 min ago"/>
          </tbody>
        </table>
      </div>
    </div>
  );
}

const CronRow = ({enabled, name, schedule, next, plat, status, last}) => (
  <tr style={{borderBottom:"1px solid var(--border-subtle)"}}>
    <td style={{padding:"var(--sp-3) var(--sp-4)"}}><Toggle checked={enabled}/></td>
    <td style={{padding:"var(--sp-3) var(--sp-4)"}}>
      <div className="row gap-2"><I.Cron size={12} style={{color:"var(--fg-dim)"}}/><span className="mono">{name}</span></div>
    </td>
    <td style={{padding:"var(--sp-3) var(--sp-4)", color:"var(--fg-muted)"}} className="mono t-small">{schedule}</td>
    <td style={{padding:"var(--sp-3) var(--sp-4)", color:"var(--fg)"}} className="mono t-small">{next}</td>
    <td style={{padding:"var(--sp-3) var(--sp-4)"}}>{plat !== "—" ? <Badge tone="info">{plat}</Badge> : <span className="fg-faint mono">—</span>}</td>
    <td style={{padding:"var(--sp-3) var(--sp-4)"}}>
      <Badge tone={status==="ok"?"success":status==="warn"?"warning":"danger"} dot>
        {status==="ok" ? "success" : status==="warn" ? "paused" : "error"}
      </Badge>
      <span className="t-micro fg-faint" style={{marginLeft:"var(--sp-3)"}}>{last}</span>
    </td>
    <td style={{padding:"var(--sp-3) var(--sp-4)"}}><Btn variant="ghost" size="sm" square icon={<I.More size={12}/>}/></td>
  </tr>
);

// ============================================================
// HOOKS / EVENT LOG
// ============================================================
function HooksScreen() {
  const events = [
    { t:"14:32:18.042", ev:"agent:step", src:"agent", lvl:"info", meta:"turn=3 tool=terminal" },
    { t:"14:32:17.901", ev:"tool:call", src:"agent", lvl:"info", meta:"name=terminal cmd=\"cargo test\"" },
    { t:"14:32:17.890", ev:"agent:thinking", src:"agent", lvl:"dbg", meta:"tokens=142" },
    { t:"14:32:16.204", ev:"session:start", src:"gateway.telegram", lvl:"info", meta:"chat=483209 session=agent:main:telegram:dm:483209" },
    { t:"14:32:16.198", ev:"guard:allow", src:"gateway.telegram", lvl:"info", meta:"user=brad policy=allowlist" },
    { t:"14:32:02.011", ev:"cron:trigger", src:"cron", lvl:"info", meta:"task=memory-prune next=tomorrow" },
    { t:"14:31:58.100", ev:"context:pressure", src:"agent", lvl:"warn", meta:"ratio=0.72 threshold=0.50" },
    { t:"14:31:57.944", ev:"memory:add", src:"agent", lvl:"info", meta:"hash=5fd2 chars=87" },
    { t:"14:31:12.302", ev:"provider:fallback", src:"provider", lvl:"warn", meta:"from=anthropic to=openrouter cause=429" },
    { t:"14:30:00.001", ev:"gateway:startup", src:"gateway.telegram", lvl:"info", meta:"bot=@ironhermesbot pid=40192" },
  ];
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">Hooks</span>
        <Badge tone="success" dot>streaming</Badge>
        <div style={{flex:1}}/>
        <Input prefix={<I.Search size={12}/>} placeholder="filter ev: prefix, src:, lvl:…" style={{width: 300}} code/>
        <Segmented options={[{value:"all",label:"all"},{value:"info",label:"info"},{value:"warn",label:"warn"},{value:"err",label:"err"}]} value="all"/>
      </div>
      <div className="ih-card" style={{padding: 0, overflow:"hidden"}}>
        <div style={{display:"grid", gridTemplateColumns:"auto 180px 180px 60px 1fr", padding:"var(--sp-3) var(--sp-4)", borderBottom:"1px solid var(--border-subtle)", background:"var(--bg-surface)", fontSize:"var(--fs-11)", color:"var(--fg-dim)", textTransform:"uppercase", letterSpacing:"0.08em", fontFamily:"var(--font-mono)"}}>
          <span>time</span><span>event</span><span>source</span><span>lvl</span><span>metadata</span>
        </div>
        {events.map((e,i) => (
          <div key={i} style={{display:"grid", gridTemplateColumns:"auto 180px 180px 60px 1fr", padding:"var(--sp-3) var(--sp-4)", borderBottom:"1px solid var(--border-subtle)", fontFamily:"var(--font-mono)", fontSize:"var(--fs-12)"}}>
            <span className="fg-faint" style={{marginRight:"var(--sp-5)"}}>{e.t}</span>
            <span className={e.ev.startsWith("context:")?"fg-warning":e.ev.startsWith("tool:")?"fg-accent":"fg"}>{e.ev}</span>
            <span className="fg-muted">{e.src}</span>
            <span><Badge tone={e.lvl==="warn"?"warning":e.lvl==="err"?"danger":e.lvl==="dbg"?"info":undefined}>{e.lvl}</Badge></span>
            <span className="fg-muted">{e.meta}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

// ============================================================
// SUBAGENT TREE
// ============================================================
function SubagentScreen() {
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">Subagents</span>
        <Badge tone="accent">3 active</Badge>
        <div style={{flex:1}}/>
        <span className="t-micro fg-dim mono-num">budget · 42 / 100 iterations</span>
      </div>
      <div style={{display:"grid", gridTemplateColumns:"360px 1fr", gap:"var(--sp-6)"}}>
        <div className="ih-card">
          <div className="ih-card-head"><span className="mono" style={{fontSize:"var(--fs-12)"}}>delegation tree</span></div>
          <div className="ih-card-body" style={{padding:"var(--sp-4)"}}>
            <Tree>
              <TNode icon={<I.Subagent/>} label="main" meta="brad" status="running" depth={0} active/>
              <TNode icon={<I.Subagent/>} label="review-phase-22.3" meta="claude-haiku" status="running" depth={1}/>
              <TNode icon={<I.Tool/>} label="read_file" meta="retro.md" status="ok" depth={2} leaf/>
              <TNode icon={<I.Tool/>} label="search_files" meta="pattern=hook" status="ok" depth={2} leaf/>
              <TNode icon={<I.Subagent/>} label="bench-providers" meta="claude-sonnet" status="running" depth={1}/>
              <TNode icon={<I.Tool/>} label="terminal" meta="cargo bench" status="run" depth={2} leaf/>
              <TNode icon={<I.Subagent/>} label="docs-lint" meta="claude-haiku" status="ok" depth={1}/>
            </Tree>
          </div>
        </div>
        <div className="ih-card">
          <div className="ih-card-head">
            <div className="row gap-3"><I.Subagent size={14} style={{color:"var(--accent)"}}/><span className="mono" style={{fontSize:"var(--fs-13)"}}>review-phase-22.3</span><Status kind="live">running</Status></div>
            <div className="row gap-3"><Badge>depth 1</Badge><Badge>isolated ctx</Badge></div>
          </div>
          <div className="ih-card-body">
            <div style={{display:"grid", gridTemplateColumns:"repeat(4, 1fr)", gap:"var(--sp-4)", marginBottom:"var(--sp-5)"}}>
              <Stat label="parent" v="main"/>
              <Stat label="model" v="claude-haiku"/>
              <Stat label="iterations" v="8 / 20"/>
              <Stat label="tokens" v="3,241"/>
            </div>
            <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>task</div>
            <div style={{padding:"var(--sp-4)", background:"var(--bg-input)", borderRadius:"var(--r-2)", border:"1px solid var(--border-subtle)", fontSize:"var(--fs-13)", marginBottom:"var(--sp-5)"}}>
              Read <code className="inline">.planning/phases/22.RETROSPECTIVE.md</code>, identify action items not ported to 22.3, and produce a checklist.
            </div>
            <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>tool calls</div>
            <div className="stack-sm">
              <ToolCall name="read_file" status="ok" duration="12ms" args={<span><K c="path"/>: <S c="phases/22.RETROSPECTIVE.md"/></span>}/>
              <ToolCall name="search_files" status="ok" duration="84ms" args={<span><K c="pattern"/>: <S c="session:fork"/>, <K c="dir"/>: <S c="crates/"/></span>}/>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

const Tree = ({children}) => <div className="ih-tree">{children}</div>;
const TNode = ({icon, label, meta, status, depth=0, active, leaf}) => (
  <div className={"row" + (active ? " selected" : "")} style={{paddingLeft: 12 + depth*18, padding:"var(--sp-2) var(--sp-3)", paddingLeft: 12 + depth*18 + 8, borderRadius: "var(--r-2)", background: active ? "var(--bg-selected)" : undefined}}>
    <span className="fg-faint mono" style={{width: 14}}>{leaf ? "·" : "▸"}</span>
    <span style={{color: status==="running" || status==="run" ? "var(--accent)" : status==="ok" ? "var(--success)" : "var(--fg-muted)"}}>{icon}</span>
    <span className="mono" style={{fontSize:"var(--fs-12)", color:"var(--fg)"}}>{label}</span>
    <span className="mono fg-faint t-micro" style={{marginLeft:"var(--sp-3)"}}>{meta}</span>
    <span style={{marginLeft:"auto"}}>
      {status==="running" || status==="run" ? <span className="ih-status live"><i className="dot"/></span>
        : status==="ok" ? <I.Check size={12} style={{color:"var(--success)"}}/>
        : null}
    </span>
  </div>
);

const Stat = ({label, v}) => (
  <div>
    <div className="t-label" style={{marginBottom: 4}}>{label}</div>
    <div className="mono" style={{fontSize:"var(--fs-14)", color:"var(--fg)"}}>{v}</div>
  </div>
);

// ============================================================
// TOOL INSPECTOR
// ============================================================
function ToolInspectorScreen() {
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">Tool inspector</span>
        <div style={{flex:1}}/>
        <Segmented options={[{value:"all",label:"all"},{value:"ok",label:"ok"},{value:"err",label:"err"},{value:"pending",label:"pending"}]} value="all"/>
      </div>
      <div style={{display:"grid", gridTemplateColumns:"340px 1fr", gap:"var(--sp-6)"}}>
        <div className="ih-card" style={{padding: 0}}>
          <div className="ih-card-head" style={{padding:"var(--sp-3) var(--sp-4)"}}>
            <span className="mono" style={{fontSize:"var(--fs-12)"}}>recent calls · 6</span>
          </div>
          <div className="ih-tree" style={{padding:"var(--sp-2)"}}>
            <ToolRow name="terminal" arg="cargo test -p ironhermes-cli" status="run" time="12ms"/>
            <ToolRow name="read_file" arg=".planning/phases/22.RETROSPECTIVE.md" status="ok" time="4ms" active/>
            <ToolRow name="session_search" arg="query=session:fork" status="ok" time="38ms"/>
            <ToolRow name="memory.add" arg="hash=5fd2" status="ok" time="2ms"/>
            <ToolRow name="web_search" arg="rust acp protocol" status="ok" time="412ms"/>
            <ToolRow name="terminal" arg="cargo clippy" status="err" time="1.2s"/>
          </div>
        </div>
        <div className="ih-card">
          <div className="ih-card-head">
            <div className="row gap-3">
              <I.Tool size={14} style={{color:"var(--accent)"}}/>
              <span className="mono" style={{fontSize:"var(--fs-13)", fontWeight:600}}>read_file</span>
              <Badge tone="success" dot>ok</Badge>
              <span className="t-micro fg-faint mono-num">turn 2 · 14:32:17.890 · 4ms</span>
            </div>
            <div className="row gap-2">
              <Btn variant="ghost" size="sm" icon={<I.Copy size={12}/>}>copy</Btn>
              <Btn variant="ghost" size="sm" icon={<I.Refresh size={12}/>}>replay</Btn>
            </div>
          </div>
          <div className="ih-card-body" style={{padding: 0}}>
            <div className="ih-tabs" style={{padding:"0 var(--sp-5)"}}>
              <div className="ih-tab" aria-selected="true">arguments</div>
              <div className="ih-tab">result</div>
              <div className="ih-tab">stderr</div>
              <div className="ih-tab">trace</div>
            </div>
            <div style={{padding:"var(--sp-5) var(--sp-6)"}}>
              <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>arguments · json</div>
              <pre className="block"><span className="p">{"{"}</span>{"\n  "}<span className="k">"path"</span><span className="p">:</span> <span className="s">".planning/phases/22.RETROSPECTIVE.md"</span><span className="p">,</span>{"\n  "}<span className="k">"offset"</span><span className="p">:</span> <span className="n">0</span><span className="p">,</span>{"\n  "}<span className="k">"limit"</span><span className="p">:</span> <span className="n">2000</span>{"\n"}<span className="p">{"}"}</span></pre>
              <div style={{height:"var(--sp-5)"}}/>
              <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>result · 16.8kb · 412 lines</div>
              <pre className="block"><span className="c"># Phase 22 Retrospective</span>{"\n"}{"\n"}## What worked{"\n"}{"\n"}- Three-way split at D-01 (22/22.1/22.2) contained scope creep{"\n"}- HookRegistry parity between CLI and gateway{"\n"}{"\n"}## What didn't{"\n"}{"\n"}- ACP session lineage not lifted into GW-06{"\n"}- session:fork hook event missing from catalog{"\n"}- FTS5 snippet generation doesn't span forked sessions{"\n"}{"\n"}<span className="c"># ... 402 more lines</span></pre>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

const ToolRow = ({name, arg, status, time, active}) => (
  <div className={"row" + (active ? " selected" : "")} style={{padding:"var(--sp-3)", borderRadius:"var(--r-2)", background: active ? "var(--bg-selected)" : undefined, marginBottom: 2, gap:"var(--sp-2)"}}>
    <span style={{color: status==="run" ? "var(--accent)" : status==="ok" ? "var(--success)" : "var(--danger)"}}>
      {status==="run" ? <span className="ih-status live"><i className="dot"/></span> : status==="ok" ? <I.CheckFilled size={12}/> : <I.Warn size={12}/>}
    </span>
    <div className="col" style={{flex: 1, minWidth: 0}}>
      <span className="mono" style={{fontSize:"var(--fs-12)", color:"var(--fg)", fontWeight:500}}>{name}</span>
      <span className="mono truncate t-micro fg-faint">{arg}</span>
    </div>
    <span className="mono-num t-micro fg-faint">{time}</span>
  </div>
);

// ============================================================
// PROVIDER PICKER
// ============================================================
function ProviderScreen() {
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto", maxWidth: 900, margin:"0 auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">Provider & model</span>
      </div>
      <div className="ih-card">
        <div className="ih-card-body">
          <div className="t-label" style={{marginBottom:"var(--sp-4)"}}>primary</div>
          <ProviderRow active name="anthropic" model="claude-3.7-sonnet-20250219" mode="anthropic_messages" status="ok" cache/>
          <ProviderRow name="openrouter" model="anthropic/claude-3.7-sonnet" mode="chat_completions" status="ok" fallback/>
          <ProviderRow name="nous" model="hermes-3-llama-70b" mode="chat_completions" status="ok"/>
          <div style={{height:"var(--sp-5)"}}/>
          <div className="t-label" style={{marginBottom:"var(--sp-4)"}}>auxiliary</div>
          <ProviderRow name="openai" model="gpt-4o-mini" mode="chat_completions" status="warn" aux="vision"/>
          <ProviderRow name="anthropic" model="claude-haiku-4" mode="anthropic_messages" status="ok" aux="compression"/>
          <ProviderRow name="openrouter" model="perplexity/sonar-small" mode="chat_completions" status="ok" aux="session search"/>
        </div>
      </div>
      <div style={{height:"var(--sp-5)"}}/>
      <div className="ih-card">
        <div className="ih-card-head"><span className="mono" style={{fontSize:"var(--fs-12)"}}>iteration budget</span></div>
        <div className="ih-card-body">
          <div style={{display:"grid", gridTemplateColumns:"1fr 1fr 1fr 1fr", gap:"var(--sp-5)"}}>
            <Stat label="main" v="42 / 100"/>
            <Stat label="caution · 70%" v="yellow at 70"/>
            <Stat label="warning · 90%" v="respond now"/>
            <Stat label="stop · 100%" v="hard stop"/>
          </div>
          <div style={{height:"var(--sp-4)"}}/>
          <Progress value={42} label="main.iterations" suffix="42 / 100"/>
        </div>
      </div>
    </div>
  );
}

const ProviderRow = ({active, name, model, mode, status, cache, fallback, aux}) => (
  <div className="row gap-3" style={{padding:"var(--sp-4)", borderRadius:"var(--r-3)", border:"1px solid " + (active ? "var(--accent)" : "var(--border-subtle)"), background: active ? "var(--accent-soft)" : "transparent", marginBottom:"var(--sp-3)"}}>
    <span className="ih-radio" aria-checked={active}></span>
    <div className="col" style={{flex:1, minWidth: 0}}>
      <div className="row gap-3">
        <span className="mono" style={{fontWeight: 600}}>{name}</span>
        <span className="mono t-small fg-muted">{model}</span>
        {cache && <Badge tone="accent">cache_control</Badge>}
        {fallback && <Badge>fallback</Badge>}
        {aux && <Badge tone="info">{aux}</Badge>}
      </div>
      <span className="mono t-micro fg-faint" style={{marginTop: 2}}>{mode}</span>
    </div>
    <Badge tone={status==="ok"?"success":status==="warn"?"warning":"danger"} dot>
      {status==="ok" ? "healthy" : status==="warn" ? "quota 81%" : "error"}
    </Badge>
  </div>
);

// ============================================================
// SOUL.md editor
// ============================================================
function SoulScreen() {
  return (
    <div style={{padding:"var(--sp-8)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      <div className="row gap-3" style={{marginBottom:"var(--sp-6)"}}>
        <span className="t-h2">SOUL.md</span>
        <Badge tone="accent">frozen snapshot</Badge>
        <div style={{flex:1}}/>
        <Btn variant="ghost" size="sm" icon={<I.Refresh size={12}/>}>reload from disk</Btn>
        <Btn size="sm" variant="primary" icon={<I.Check size={12}/>}>save</Btn>
      </div>
      <div style={{display:"grid", gridTemplateColumns:"1fr 320px", gap:"var(--sp-6)"}}>
        <div className="ih-card">
          <div className="ih-card-head">
            <div className="row gap-3"><I.Soul size={14} style={{color:"var(--accent)"}}/><span className="mono" style={{fontSize:"var(--fs-12)"}}>~/.ironhermes/SOUL.md</span></div>
            <span className="t-micro fg-dim mono-num">2,134 / 20,000 chars</span>
          </div>
          <div className="ih-card-body" style={{padding: 0}}>
            <pre className="block" style={{border: 0, borderRadius: 0, background:"transparent", padding:"var(--sp-6) var(--sp-6)"}}>
{`<span class="c"># identity</span>

You are <span class="s">IronHermes</span> — a self-improving agent ported from
hermes-agent. Your interface is terse, technical, and opinionated.

<span class="c"># style</span>

- <span class="k">response_shape</span>: bullets over paragraphs; ≤4 bullets per list
- <span class="k">code_tone</span>: imperative, lowercase filenames, kebab-case
- <span class="k">verbosity</span>: minimal — say what happened, skip apologies
- <span class="k">emoji</span>: never

<span class="c"># operating principles</span>

1. When you edit your own context, announce the diff before writing.
2. If context pressure exceeds <span class="n">0.50</span>, compress before continuing.
3. Tool failures are reported verbatim; do not paraphrase stderr.
4. Memory writes carry a one-line justification.
`.split(/(<span class="[a-z]+">[^<]*<\/span>)/g).map((chunk,i) => {
  if (chunk.startsWith("<span")) {
    const cls = chunk.match(/class="([a-z]+)"/)[1];
    const txt = chunk.replace(/<[^>]+>/g,"");
    return <span key={i} className={cls}>{txt}</span>;
  }
  return chunk;
})}
            </pre>
          </div>
          <div className="ih-card-foot">
            <span className="t-micro fg-faint mono-num">last modified · 2d ago</span>
            <div style={{flex:1}}/>
            <Btn variant="ghost" size="sm" icon={<I.Copy size={12}/>}>copy</Btn>
          </div>
        </div>
        <div className="col gap-4">
          <div className="ih-card">
            <div className="ih-card-head"><span className="mono" style={{fontSize:"var(--fs-12)"}}>session overlay</span></div>
            <div className="ih-card-body stack-sm">
              <div className="t-small fg-muted">Apply a personality preset without modifying SOUL.md on disk.</div>
              <div className="col gap-2">
                {[
                  ["helpful","balanced, asks for context"],
                  ["concise","terse, minimal"],
                  ["technical","precise, no fluff"],
                  ["creative","divergent, exploratory"],
                  ["teacher","explains reasoning"],
                ].map(([n,d])=> (
                  <div key={n} className="row gap-3" style={{padding:"var(--sp-3)", borderRadius:"var(--r-2)", border:"1px solid var(--border-subtle)"}}>
                    <span className="ih-radio" aria-checked={n==="concise"}></span>
                    <div className="col" style={{flex:1}}>
                      <span className="mono" style={{fontSize:"var(--fs-12)", fontWeight:500}}>{n}</span>
                      <span className="t-micro fg-faint">{d}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
          <div className="ih-card">
            <div className="ih-card-head"><span className="mono" style={{fontSize:"var(--fs-12)"}}>security</span></div>
            <div className="ih-card-body stack-sm">
              <div className="row gap-2"><I.CheckFilled size={12} style={{color:"var(--success)"}}/><span className="t-small fg-muted">injection scan · passed</span></div>
              <div className="row gap-2"><I.CheckFilled size={12} style={{color:"var(--success)"}}/><span className="t-small fg-muted">20k char cap · under</span></div>
              <div className="row gap-2"><I.CheckFilled size={12} style={{color:"var(--success)"}}/><span className="t-small fg-muted">invisible unicode · none</span></div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// ============================================================
// SETTINGS WIZARD
// ============================================================
function SettingsScreen() {
  return (
    <div style={{display:"grid", gridTemplateColumns:"220px 1fr", height:"100%", background:"var(--bg-canvas)"}}>
      <aside style={{borderRight:"1px solid var(--border-subtle)", padding:"var(--sp-5) var(--sp-3)", background:"var(--bg-surface)"}}>
        <div className="t-label" style={{padding:"0 var(--sp-3)", marginBottom:"var(--sp-3)"}}>settings</div>
        {[
          ["General","gear",true],["Providers","provider",false],["Memory","memory",false],
          ["Skills","skill",false],["Hooks","hook",false],["Gateway","chat",false],
          ["ACP","code",false],["Security","lock",false],
        ].map(([l,ic,a])=> {
          const Ic = {gear:I.Gear, provider:I.Provider, memory:I.Memory, skill:I.Skill, hook:I.Hook, chat:I.Chat, code:I.Code, lock:I.Lock}[ic];
          return (
            <div key={l} className="row gap-3" style={{padding:"var(--sp-3)", borderRadius:"var(--r-2)", background: a ? "var(--bg-selected)" : "transparent", color: a ? "var(--fg)" : "var(--fg-muted)", fontSize:"var(--fs-13)", cursor:"default"}}>
              <Ic size={13}/><span>{l}</span>
            </div>
          );
        })}
      </aside>
      <main style={{padding:"var(--sp-8) var(--sp-9)", overflow:"auto", maxWidth: 680}} className="ih-scroll">
        <span className="t-h2" style={{display:"block", marginBottom:"var(--sp-7)"}}>General</span>
        <Setting label="Profile" desc="Each profile gets its own HERMES_HOME, config, memory, and sessions.">
          <Input value="default" prefix={<I.Folder size={12}/>} suffix="/home/brad/.ironhermes"/>
        </Setting>
        <Setting label="Default model" desc="Used when no session-level override is set.">
          <Input value="claude-3.7-sonnet-20250219" prefix="anthropic/" code/>
        </Setting>
        <Setting label="Startup skills" desc="Auto-load on CLI start. Skills with missing env vars are skipped.">
          <div className="row gap-2" style={{flexWrap:"wrap"}}>
            <Badge tone="accent" dot>phase-planner ×</Badge>
            <Badge tone="accent" dot>rust-workspace ×</Badge>
            <Badge tone="accent" dot>session-search ×</Badge>
            <Btn variant="ghost" size="sm" icon={<I.Plus size={11}/>}>add</Btn>
          </div>
        </Setting>
        <Setting label="Context compression" desc="Dual-threshold — agent compacts at 50%, gateway hygiene at 85%.">
          <div className="row gap-4">
            <div className="col" style={{flex:1}}>
              <span className="t-micro fg-dim">agent</span>
              <Input value="0.50" suffix="threshold"/>
            </div>
            <div className="col" style={{flex:1}}>
              <span className="t-micro fg-dim">gateway</span>
              <Input value="0.85" suffix="threshold"/>
            </div>
          </div>
        </Setting>
        <Setting label="Prompt caching" desc="Places cache_control breakpoints using system_and_3 strategy.">
          <div className="row gap-4">
            <Toggle checked={true}/>
            <Segmented options={[{value:"5m",label:"5 min"},{value:"1h",label:"1 hour"}]} value="5m"/>
          </div>
        </Setting>
        <Setting label="Telemetry" desc="Anonymous, aggregate usage. Never includes message content.">
          <Toggle checked={false}/>
        </Setting>
      </main>
    </div>
  );
}

const Setting = ({label, desc, children}) => (
  <div style={{display:"grid", gridTemplateColumns:"260px 1fr", gap:"var(--sp-6)", padding:"var(--sp-5) 0", borderBottom:"1px solid var(--border-subtle)"}}>
    <div>
      <div style={{fontWeight: 600, fontSize:"var(--fs-13)"}}>{label}</div>
      <div className="t-small fg-muted" style={{marginTop: 4}}>{desc}</div>
    </div>
    <div>{children}</div>
  </div>
);

Object.assign(window, {
  MemoryScreen, SkillsScreen, CronScreen, HooksScreen,
  SubagentScreen, ToolInspectorScreen, ProviderScreen,
  SoulScreen, SettingsScreen,
});
