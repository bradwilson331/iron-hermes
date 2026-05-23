// components-view.jsx — showcases of every core component

function _SectionHeader({eyebrow, title, subtitle}) {
  return (
    <div style={{marginBottom:"var(--sp-7)"}}>
      <span className="t-label">{eyebrow}</span>
      <h1 className="t-h1" style={{margin:"var(--sp-3) 0 var(--sp-3)"}}>{title}</h1>
      {subtitle && <p className="t-body fg-muted" style={{maxWidth:"62ch", margin: 0}}>{subtitle}</p>}
    </div>
  );
}

function _PageWrap({children}) {
  return <div style={{padding:"var(--sp-10) var(--sp-12)", maxWidth: 1100}}>{children}</div>;
}

function _Group({label, children}) {
  return (
    <div style={{marginTop:"var(--sp-7)"}}>
      <span className="t-label" style={{display:"block", marginBottom:"var(--sp-3)"}}>{label}</span>
      <div className="ih-card" style={{padding:"var(--sp-6)"}}>
        {children}
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// BUTTONS
// ─────────────────────────────────────────────────────────────
function ButtonsView() {
  return (
    <_PageWrap>
      <_SectionHeader eyebrow="components / buttons" title="Buttons" subtitle="Used for actions. Primary triggers commit; ghost is default for toolbar chrome; danger is destructive only."/>

      <_Group label="variants">
        <div className="row gap-3" style={{flexWrap:"wrap"}}>
          <Btn variant="primary">Run agent</Btn>
          <Btn>Open session</Btn>
          <Btn variant="ghost">Cancel</Btn>
          <Btn variant="danger">Delete</Btn>
          <Btn icon={<I.Play size={12}/>} variant="primary">Start</Btn>
          <Btn icon={<I.Plus size={12}/>}>New session</Btn>
          <Btn icon={<I.Copy size={12}/>} variant="ghost">Copy</Btn>
        </div>
      </_Group>

      <_Group label="sizes">
        <div className="row gap-3" style={{alignItems:"center"}}>
          <Btn size="sm" variant="primary">Small</Btn>
          <Btn variant="primary">Default</Btn>
          <Btn size="lg" variant="primary">Large</Btn>
        </div>
      </_Group>

      <_Group label="icon-only (square)">
        <div className="row gap-3">
          <Btn square variant="ghost"><I.Plus size={14}/></Btn>
          <Btn square variant="ghost"><I.Search size={14}/></Btn>
          <Btn square variant="ghost"><I.More size={14}/></Btn>
          <Btn square variant="ghost"><I.Refresh size={14}/></Btn>
          <Btn square><I.Gear size={14}/></Btn>
          <Btn square variant="primary"><I.Send size={14}/></Btn>
        </div>
      </_Group>

      <_Group label="with keyboard shortcut">
        <div className="row gap-3">
          <Btn>Accept <Kbd>⏎</Kbd></Btn>
          <Btn variant="ghost">Dismiss <Kbd>esc</Kbd></Btn>
          <Btn variant="primary">Send <Kbd>⌘</Kbd><Kbd>⏎</Kbd></Btn>
        </div>
      </_Group>
    </_PageWrap>
  );
}

// ─────────────────────────────────────────────────────────────
// FORMS & INPUTS
// ─────────────────────────────────────────────────────────────
function FormsView() {
  return (
    <_PageWrap>
      <_SectionHeader eyebrow="components / forms" title="Forms & inputs" subtitle="Inputs, toggles, segmented pickers. All support the design-token borders and focus rings."/>

      <_Group label="text input">
        <div className="stack-sm" style={{maxWidth: 520}}>
          <Input placeholder="Search sessions…" prefix={<I.Search size={12}/>}/>
          <Input placeholder="anthropic-api-key-..." code prefix={<I.Lock size={12}/>}/>
          <Input placeholder="Add a new memory note" suffix={<Kbd>⏎</Kbd>}/>
          <Input value="~/.ironhermes/config.toml" code prefix={<I.File size={12}/>}/>
        </div>
      </_Group>

      <_Group label="segmented">
        <div className="stack-sm">
          <Segmented value="sonnet" options={[
            {value:"haiku",  label:"Haiku 4.5"},
            {value:"sonnet", label:"Sonnet 4.5"},
            {value:"opus",   label:"Opus 4.5"},
          ]}/>
          <Segmented value="dark" options={[
            {value:"light", label:"Light"},
            {value:"dark",  label:"Dark"},
            {value:"auto",  label:"Auto"},
          ]}/>
        </div>
      </_Group>

      <_Group label="toggles & selects">
        <div className="stack-sm">
          <div className="row gap-4" style={{alignItems:"center"}}>
            <Toggle checked={true}/>
            <span className="t-body">Prompt caching</span>
            <span className="t-small fg-muted" style={{marginLeft:"auto"}}>5-minute TTL</span>
          </div>
          <div className="row gap-4" style={{alignItems:"center"}}>
            <Toggle checked={false}/>
            <span className="t-body">Run pre-commit hook</span>
            <span className="t-small fg-muted" style={{marginLeft:"auto"}}>hooks.on_tool</span>
          </div>
          <div className="row gap-4" style={{alignItems:"center"}}>
            <Toggle checked={true}/>
            <span className="t-body">Share anonymous telemetry</span>
          </div>
        </div>
      </_Group>

      <_Group label="progress & capacity">
        <div className="stack-sm">
          <Progress value={0.67} label="MEMORY.md" suffix="67%"/>
          <Progress value={0.23} label="USER.md" suffix="23%" tone="success"/>
          <Progress value={0.92} label="context window" suffix="184k / 200k" tone="warning"/>
          <Progress value={0.03} label="daily spend" suffix="$0.41 / $15.00"/>
        </div>
      </_Group>
    </_PageWrap>
  );
}

// ─────────────────────────────────────────────────────────────
// BADGES & STATUS
// ─────────────────────────────────────────────────────────────
function BadgesView() {
  return (
    <_PageWrap>
      <_SectionHeader eyebrow="components / badges" title="Badges & status" subtitle="Dense informational chips. Dot variant signals live state; square variant pairs with mono labels."/>

      <_Group label="tones">
        <div className="row gap-3" style={{flexWrap:"wrap"}}>
          <Badge>default</Badge>
          <Badge tone="accent">accent</Badge>
          <Badge tone="success">success</Badge>
          <Badge tone="warning">warning</Badge>
          <Badge tone="danger">danger</Badge>
          <Badge tone="info">info</Badge>
        </div>
      </_Group>

      <_Group label="with dot (live state)">
        <div className="row gap-3" style={{flexWrap:"wrap"}}>
          <Badge tone="success" dot>active</Badge>
          <Badge tone="warning" dot>running</Badge>
          <Badge tone="danger"  dot>failed</Badge>
          <Badge tone="accent"  dot>streaming</Badge>
          <Badge dot>idle</Badge>
        </div>
      </_Group>

      <_Group label="square (metadata)">
        <div className="row gap-3" style={{flexWrap:"wrap"}}>
          <Badge square>v0.1.0</Badge>
          <Badge square tone="accent">sonnet-4.5</Badge>
          <Badge square tone="success">ok</Badge>
          <Badge square>kebab-case</Badge>
          <Badge square tone="info">rust 1.82</Badge>
        </div>
      </_Group>

      <_Group label="live status indicators">
        <div className="stack-sm">
          <Status kind="live">Agent running · turn 3/20</Status>
          <Status kind="idle">Idle · waiting for input</Status>
          <Status kind="done">Completed in 4.2s · 12.3k tokens</Status>
          <Status kind="error">Tool failed · terminal exit 127</Status>
        </div>
      </_Group>
    </_PageWrap>
  );
}

// ─────────────────────────────────────────────────────────────
// CARDS & LAYOUT
// ─────────────────────────────────────────────────────────────
function CardsView() {
  return (
    <_PageWrap>
      <_SectionHeader eyebrow="components / cards" title="Cards & layout" subtitle="Low-chrome containers. Keep borders subtle; rely on spacing and type weight for hierarchy."/>

      <_Group label="basic card">
        <div style={{display:"grid", gridTemplateColumns:"1fr 1fr", gap:"var(--sp-4)"}}>
          <div className="ih-card" style={{padding:"var(--sp-5)"}}>
            <div className="row" style={{alignItems:"center"}}>
              <I.Memory size={14} style={{color:"var(--accent)"}}/>
              <span className="t-label" style={{marginLeft: 8}}>MEMORY.md</span>
            </div>
            <div style={{fontSize:28, fontFamily:"var(--font-mono-cond)", fontWeight:700, marginTop:12}}>67%</div>
            <div className="t-small fg-muted">13.4k of 20k tokens</div>
          </div>
          <div className="ih-card" style={{padding:"var(--sp-5)"}}>
            <div className="row" style={{alignItems:"center"}}>
              <I.Cron size={14} style={{color:"var(--accent)"}}/>
              <span className="t-label" style={{marginLeft: 8}}>scheduled</span>
            </div>
            <div style={{fontSize:28, fontFamily:"var(--font-mono-cond)", fontWeight:700, marginTop:12}}>4 active</div>
            <div className="t-small fg-muted">next run in 00:14:32</div>
          </div>
        </div>
      </_Group>

      <_Group label="key-value list">
        <div className="ih-kv">
          <div><span className="k">session-id</span><span className="v mono">01HT8Z4A9QK</span></div>
          <div><span className="k">provider</span><span className="v">anthropic</span></div>
          <div><span className="k">model</span><span className="v mono">claude-sonnet-4.5</span></div>
          <div><span className="k">context</span><span className="v">183,421 / 200,000</span></div>
          <div><span className="k">cost</span><span className="v mono">$0.413</span></div>
          <div><span className="k">cwd</span><span className="v mono">~/code/ironhermes</span></div>
        </div>
      </_Group>

      <_Group label="list item rows">
        <div className="ih-listing">
          {[
            {name:"refactor session storage", t:"3h", state:"success", meta:"12.3k tokens"},
            {name:"write migration for fts5", t:"6h", state:"warning", meta:"paused"},
            {name:"benchmark tokio runtime",  t:"1d", state:"success", meta:"4.1k tokens"},
            {name:"debug hook timeout",       t:"2d", state:"danger",  meta:"3 retries"},
          ].map((r,i) => (
            <div key={i} className="row gap-4" style={{padding:"var(--sp-3) var(--sp-4)", borderBottom: i === 3 ? 0 : "1px solid var(--border-subtle)"}}>
              <I.Session size={13} style={{color:"var(--fg-faint)"}}/>
              <span style={{flex:1}}>{r.name}</span>
              <span className="mono t-small fg-faint">{r.meta}</span>
              <Badge tone={r.state} dot square>{r.state}</Badge>
              <span className="mono t-small fg-faint" style={{width: 32, textAlign:"right"}}>{r.t}</span>
            </div>
          ))}
        </div>
      </_Group>
    </_PageWrap>
  );
}

// ─────────────────────────────────────────────────────────────
// TERMINAL BLOCKS
// ─────────────────────────────────────────────────────────────
function TerminalBlocksView() {
  return (
    <_PageWrap>
      <_SectionHeader eyebrow="components / terminal" title="Terminal blocks" subtitle="Discrete input→output cards. Each command is self-contained, collapsible, and carries a state rail showing exit code and duration."/>

      <_Group label="basic command">
        <CmdBlock cwd="~/code/ironhermes" cmd="cargo build --release" state="ok" duration="24.8s">
{`   Compiling ironhermes v0.1.0 (/Users/bw/code/ironhermes)
    Finished \`release\` profile [optimized] target(s) in 24.78s`}
        </CmdBlock>
      </_Group>

      <_Group label="states">
        <div className="stack-sm">
          <CmdBlock cwd="~" cmd="git status" state="ok" duration="0.03s">
{`On branch main
nothing to commit, working tree clean`}
          </CmdBlock>
          <CmdBlock cwd="~" cmd="cargo test --package agent" state="running" duration="…">
{`running 42 tests
test agent::memory::read ... ok
test agent::memory::write ... ok
test agent::session::fts   ... `}
          </CmdBlock>
          <CmdBlock cwd="~" cmd="deploy --env prod" state="err" duration="3.12s">
{`Error: missing environment variable ANTHROPIC_API_KEY
       at src/provider/anthropic.rs:42`}
          </CmdBlock>
        </div>
      </_Group>

      <_Group label="with inline pills">
        <CmdBlock cwd="~/code/ironhermes" cmd="ih session list --tag refactor" state="ok" duration="0.14s">
          <div className="stack-xs" style={{fontFamily:"var(--font-mono)", fontSize: 12}}>
            <div>01HT8Z4A9QK  <Badge square tone="success">done</Badge>   refactor session storage</div>
            <div>01HT8Z4B2ZV  <Badge square tone="warning">paused</Badge> migrate fts5 schema</div>
            <div>01HT8Z4C4JP  <Badge square tone="success">done</Badge>   benchmark tokio</div>
          </div>
        </CmdBlock>
      </_Group>
    </_PageWrap>
  );
}

// ─────────────────────────────────────────────────────────────
// TOOL CALLS
// ─────────────────────────────────────────────────────────────
function ToolCallsView() {
  return (
    <_PageWrap>
      <_SectionHeader eyebrow="components / tool calls" title="Tool calls" subtitle="Appear inline inside agent messages. Show name, args summary, status, duration — collapse by default when long."/>

      <_Group label="states">
        <div className="stack-sm">
          <ToolCall name="read_file" status="ok" duration="0.04s"
            args={<><K c="path"/>=<S c="src/agent/memory.rs"/></>}>
{`pub struct MemoryStore {
    base: PathBuf,
    fts:  SqlitePool,
}`}
          </ToolCall>
          <ToolCall name="terminal" status="ok" duration="1.84s"
            args={<><K c="cmd"/>=<S c="cargo check"/></>}>
{`    Checking ironhermes v0.1.0
     Finished \`dev\` profile in 1.84s`}
          </ToolCall>
          <ToolCall name="patch" status="running" duration="…"
            args={<><K c="path"/>=<S c="src/agent/loop.rs"/> <K c="hunks"/>=<N c="3"/></>}/>
          <ToolCall name="fetch_url" status="err" duration="5.00s"
            args={<><K c="url"/>=<S c="https://api.internal/models"/></>}>
{`Error: connection refused (os error 61)`}
          </ToolCall>
        </div>
      </_Group>
    </_PageWrap>
  );
}

// ─────────────────────────────────────────────────────────────
// CHAT MESSAGES
// ─────────────────────────────────────────────────────────────
function ChatMsgView() {
  return (
    <_PageWrap>
      <_SectionHeader eyebrow="components / chat" title="Chat messages" subtitle="Speaker-labeled blocks. Agent messages can contain tool calls and thought blocks; user messages are plain prose or attachments."/>

      <_Group label="conversation">
        <div className="stack-md">
          <Msg who="user" author="bw" time="14:03">
            Pick up where we left off — the session storage refactor. The fts5 migration was blocking.
          </Msg>
          <Msg who="agent" author="ironhermes" time="14:03">
            <p>Reading the last session memory and the migration file to understand the blocker.</p>
            <ToolCall name="read_file" status="ok" duration="0.03s"
              args={<><K c="path"/>=<S c="migrations/003_fts.sql"/></>}/>
            <ToolCall name="read_file" status="ok" duration="0.04s"
              args={<><K c="path"/>=<S c=".ironhermes/MEMORY.md"/></>}/>
            <p>The <code className="inline">tokenize = 'porter'</code> option is unavailable on the bundled sqlite build. Switching to <code className="inline">unicode61 remove_diacritics 2</code>.</p>
          </Msg>
          <Msg who="user" author="bw" time="14:05">
            Go ahead. Commit after the tests pass.
          </Msg>
          <Msg who="agent" author="ironhermes" time="14:05">
            <ToolCall name="patch" status="ok" duration="0.11s"
              args={<><K c="path"/>=<S c="migrations/003_fts.sql"/> <K c="hunks"/>=<N c="1"/></>}/>
            <ToolCall name="terminal" status="running" duration="…"
              args={<><K c="cmd"/>=<S c="cargo test --package agent::session"/></>}/>
          </Msg>
        </div>
      </_Group>

      <_Group label="thought block">
        <div style={{padding:"var(--sp-4) var(--sp-5)", borderLeft:"2px solid var(--border)", background: "color-mix(in oklch, var(--bg-raised) 50%, transparent)", borderRadius:"var(--r-2)"}}>
          <div className="row" style={{alignItems:"center", marginBottom: 6}}>
            <I.Thought size={12} style={{color:"var(--fg-faint)"}}/>
            <span className="t-label" style={{marginLeft: 6}}>thinking · 1.2s</span>
          </div>
          <p className="t-small fg-muted" style={{margin: 0, fontStyle:"italic"}}>
            The user wants me to resume. I should re-read MEMORY.md first to reload context, then check the migration. The fts5 issue was that the tokenize directive used an unavailable option…
          </p>
        </div>
      </_Group>
    </_PageWrap>
  );
}

window.ButtonsView = ButtonsView;
window.FormsView = FormsView;
window.BadgesView = BadgesView;
window.CardsView = CardsView;
window.TerminalBlocksView = TerminalBlocksView;
window.ToolCallsView = ToolCallsView;
window.ChatMsgView = ChatMsgView;
