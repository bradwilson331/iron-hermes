// Desktop.jsx — IronHermes Manager, a Warp-inspired desktop management app.
// Manages the bundled Dioxus web server, handles terminal commands, and runs
// agent sessions. All rendered in the IronHermes CLI aesthetic: monospace,
// ANSI color, box-drawing, orange brand accent.
//
// Layout (left → right):
//   sidebar (sessions + server status)  |  content (tabs + breadcrumb + session + composer)

const HR = (n, ch = "─") => ch.repeat(n);

// ─────────────────────────────────────────────────────────────
// Header icons — box-drawing substitutes for GUI icons
// ─────────────────────────────────────────────────────────────
const I = {
  sidebar: "▌▐",
  wrench: "⚙",
  grid: "⊞",
  search: "⌕",
  upload: "↥",
  inbox: "⌂",
  back: "‹",
  esc: "ESC",
  check: "✓",
  share: "↗",
  more: "⋯",
  attach: "@",
  mic: "▲",
  plus: "+",
  dash: "−",
  close: "×",
  bolt: "⏵",
  flame: "⏻",
  model: "◉",
  git: "⎇",
  clock: "◔",
  up: "▲",
  down: "▼",
  right: "›",
  play: "▶",
  stop: "■",
  restart: "↻",
};

// ─────────────────────────────────────────────────────────────
// Traffic lights (macOS)
// ─────────────────────────────────────────────────────────────
function TrafficLights() {
  return (
    <div className="lights">
      <span className="l r"/>
      <span className="l y"/>
      <span className="l g"/>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Title bar
// ─────────────────────────────────────────────────────────────
function TitleBar({ sidebarOpen, setSidebarOpen }) {
  return (
    <div className="tbar">
      <TrafficLights/>
      <div className="tbar-brand" title="IronHermes Manager">
        <img src="../assets/ih-shield-caduceus-transparent-256.png" alt=""/>
        <span>IronHermes</span>
      </div>
      <div className="tools">
        <button className={"ticon" + (sidebarOpen ? " on" : "")}
          onClick={() => setSidebarOpen(v => !v)}
          title="Toggle sidebar">{I.sidebar}</button>
        <button className="ticon" title="Settings">{I.wrench}</button>
        <button className="ticon" title="Workflows">{I.grid}</button>
      </div>
      <div className="search">
        <span style={{color: "var(--fg-dim)"}}>{I.search}</span>
        <span>Search sessions, agents, files…</span>
        <span className="kbd">⌘K</span>
      </div>
      <div className="right">
        <button className="ticon" title="Share">{I.upload}</button>
        <button className="ticon" title="Inbox">{I.inbox}</button>
        <div className="avatar" title="@twilson">TW</div>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Sidebar
// ─────────────────────────────────────────────────────────────
function Sidebar({ sessions, activeId, onActivate, server }) {
  const running = sessions.filter(s => s.status === "running").length;
  const total = sessions.length;

  // live sparkline for server requests/min
  const [spark, setSpark] = React.useState(
    () => Array.from({length: 20}, () => 0.4 + Math.random() * 0.5)
  );
  React.useEffect(() => {
    if (!server.running) return;
    const id = setInterval(() => {
      setSpark(prev => {
        const next = prev.slice(1);
        next.push(0.3 + Math.random() * 0.7);
        return next;
      });
    }, 900);
    return () => clearInterval(id);
  }, [server.running]);

  return (
    <aside className="sidebar">
      <div className="sb-top">
        <div className="sb-search">
          <span>{I.search}</span>
          <span>Search tabs…</span>
        </div>
        <button className="sb-btn" title="Filter">≡</button>
        <button className="sb-btn" title="New session">{I.plus}</button>
      </div>

      <div className="sb-section">
        <span>Open Sessions</span>
        <span className="count">{running} running · {total} total</span>
      </div>

      <div className="sb-scroll">
        {sessions.map(s => (
          <div key={s.id}
            className={"sb-row" + (s.id === activeId ? " active" : "")}
            onClick={() => onActivate(s.id)}>
            <span className="glyph">
              {s.kind === "agent" ? "/" :
               s.kind === "server" ? "⏵" :
               s.kind === "md" ? "¶" : "$"}
            </span>
            <div className="meta">
              <div className="title">{s.title}</div>
              <div className="sub">
                {s.status === "running" && <span style={{color: "var(--warn)"}}>● </span>}
                {s.status === "ok" && <span style={{color: "var(--success)"}}>✓ </span>}
                {s.status === "err" && <span style={{color: "var(--danger)"}}>✕ </span>}
                {s.sub}
              </div>
            </div>
            <div className="when">{s.when}</div>
          </div>
        ))}
      </div>

      <div className="sb-foot">
        <div style={{
          display: "flex", alignItems: "center", justifyContent: "space-between",
          marginBottom: 4,
        }}>
          <span style={{color: "var(--brand)", fontWeight: 700, fontSize: 10, letterSpacing: "0.08em"}}>
            ── DIOXUS SERVER ──
          </span>
        </div>
        <div className="row">
          <span className="k">status</span>
          <span className={"v " + (server.running ? "ok" : "warn")}>
            {server.running ? "● running" : "○ stopped"}
          </span>
        </div>
        <div className="row">
          <span className="k">port</span>
          <span className="v">:{server.port}</span>
        </div>
        <div className="row">
          <span className="k">target</span>
          <span className="v">wasm32 · {server.build}</span>
        </div>
        <div className="row">
          <span className="k">req/min</span>
          <span className="v">{server.rpm}</span>
        </div>
        <div className="sparkline">
          {spark.map((v, i) => (
            <div key={i} className="bar" style={{
              height: `${Math.max(10, v * 100)}%`,
              opacity: server.running ? 0.5 + v * 0.5 : 0.2,
            }}/>
          ))}
        </div>
        <div style={{display: "flex", gap: 4, marginTop: 6}}>
          <button className="sb-btn" style={{flex: 1}} title="Restart">{I.restart} restart</button>
          <button className="sb-btn" style={{width: 28}} title="Stop">{I.stop}</button>
        </div>
      </div>
    </aside>
  );
}

// ─────────────────────────────────────────────────────────────
// Tab bar
// ─────────────────────────────────────────────────────────────
function TabBar({ tabs, activeId, onActivate, onClose }) {
  return (
    <div className="tabs">
      {tabs.map(t => (
        <div key={t.id}
          className={"tab" + (t.id === activeId ? " active" : "") +
            (t.state === "running" ? " running" : "") +
            (t.state === "ok" ? " ok" : "")}
          onClick={() => onActivate(t.id)}>
          <span className="tdot"/>
          <span className="tname">{t.label}</span>
          <button className="x" onClick={e => { e.stopPropagation(); onClose(t.id); }}>{I.close}</button>
        </div>
      ))}
      <button className="add" title="New tab">{I.plus}</button>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Breadcrumb
// ─────────────────────────────────────────────────────────────
function Breadcrumb({ title, taskState }) {
  return (
    <div className="breadcrumb">
      <button className="bc-back">
        <span className="bc-kbd">{I.esc}</span>
        <span>for terminal</span>
      </button>
      <div className={"bc-task" + (taskState === "running" ? " running" : "")}>
        <span className="ok">{taskState === "running" ? "●" : I.check}</span>
        <span>{title}</span>
      </div>
      <div className="bc-right">
        <button className="bc-icon" title="Share">{I.upload}</button>
        <button className="bc-icon" title="More">{I.more}</button>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Command block (shell invocation + output)
// ─────────────────────────────────────────────────────────────
function CmdBlock({ host, cwd, dur, exit, prompt, output }) {
  return (
    <div className="block cmd">
      <div className="meta-line">
        <span className="host">{host}</span>
        <span className="sep">·</span>
        <span className="dur">{dur}</span>
        <span className="sep">·</span>
        <span className={exit === 0 ? "exit-ok" : "exit-bad"}>
          {exit === 0 ? "exit 0" : `exit ${exit}`}
        </span>
      </div>
      <div className="card">
        <div className="prompt-line">
          <span className="p">{cwd} ›</span>{" "}
          <span className="u">{prompt}</span>
        </div>
        <div>{output}</div>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Agent block (like Warp's /agent card)
// ─────────────────────────────────────────────────────────────
function AgentBlock({ cmd, arg, thoughtSec, tool, children, running, tokens, credits }) {
  return (
    <div className="block agent">
      <div className="glyph">H</div>
      <div className="body">
        <div className="hdr">
          <span className="cmd">{cmd}</span>
          <span className="arg">{arg}</span>
          <span className="more">{I.more}</span>
        </div>
        {running ? (
          <Scanner toolName={tool?.name}/>
        ) : (
          thoughtSec && (
            <div className="thought">
              <span className="caret">▸</span>
              <span>Thought for {thoughtSec}s</span>
            </div>
          )
        )}
        {tool && !running && (
          <div className="tool-card">
            <span className="ok">{I.check}</span>
            <span className="tool-name">{tool.name}</span>
            <span className="args">{tool.args}</span>
            <span className="expand">{I.right}</span>
          </div>
        )}
        <div className="prose">{children}</div>
        {!running && (
          <div className="reactions">
            <button className="rx" title="Helpful">▲</button>
            <button className="rx" title="Not helpful">▼</button>
            <span className="tokens">{tokens}</span>
            <span className="sep" style={{color:"var(--fg-dim)"}}>·</span>
            <span className="credits">{credits}</span>
            <span className="arrow">{I.right}</span>
          </div>
        )}
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Composer (bottom input + hints + status pills)
// ─────────────────────────────────────────────────────────────
function Composer({ model, onSubmit }) {
  const [value, setValue] = React.useState("");
  return (
    <div className="composer">
      <div className="cp-hints">
        <span className="hint"><span className="k">?</span> for help</span>
        <span className="hint"><span className="k">/</span> for commands</span>
        <span className="hint"><span className="k">⌘Y</span> open conversation</span>
        <span className="hint"><span className="k">⇧⌘+</span> for code review</span>
      </div>
      <div className="cp-input">
        <button className="cp-attach" title="Attach">{I.attach}</button>
        <input
          value={value}
          onChange={e => setValue(e.target.value)}
          onKeyDown={e => { if (e.key === "Enter" && value.trim()) { onSubmit?.(value); setValue(""); } }}
          placeholder="Ask hermes anything — e.g. restart the dioxus server and run doctor"/>
        <button className="cp-model" title="Model">
          <span className="dot"/>
          <span>{model}</span>
          <span style={{color: "var(--fg-dim)"}}>▾</span>
        </button>
        <button className="cp-send" title="Send">↵</button>
      </div>
      <div className="cp-status">
        <span className="pill-0">chat</span>
        <span className="sep">·</span>
        <span className="pill-1">{model}</span>
        <span className="sep">·</span>
        <span className="pill-2">anthropic</span>
        <span className="sep">·</span>
        <span className="pill-3">14.2k / 200k</span>
        <span className="sep">·</span>
        <span className="pill-4">soul: concise</span>
        <span className="sep" style={{marginLeft: "auto"}}>·</span>
        <span className="pill-4">⎇ main</span>
        <span className="sep">·</span>
        <span className="pill-4">◔ 2m idle</span>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Session content (the three-block Warp screen)
// ─────────────────────────────────────────────────────────────
function ChatSession() {
  return (
    <div className="session">
      {/* Block 1: shell command that failed */}
      <CmdBlock
        host="twilson@Brads-MacBook-Pro"
        cwd="~/code/ironhermes/site"
        dur="0.029s"
        exit={1}
        prompt="ls -lat && cargo dx serve --platform web"
        output={
          <>
            {"total 0\n"}
            <span className="out-err">ls: .: Operation not permitted</span>{"\n"}
            {"twilson@Brads-MacBook-Pro site % cargo dx serve --platform web\n"}
            <span className="out-err">error: failed to read `./Cargo.toml`</span>{"\n"}
            <span className="out-dim">   Caused by: Permission denied (os error 13)</span>{"\n\n"}
            <span className="out-warn">zsh:</span>{" command not found: Documents\n"}
            <span className="out-warn">zsh:</span>{" command not found: total\n"}
          </>
        }/>

      {/* Block 2: agent response */}
      <AgentBlock
        cmd="/agent"
        arg="fix the permissions issue and start the dioxus site"
        thoughtSec={3}
        tool={{
          name: "list_dir",
          args: "~/code/ironhermes/site   # inspect tree before acting",
        }}
        tokens="14.2k / 200k"
        credits="5.9 credits">
        <p>
          The <span className="warn-ref">Operation not permitted</span> error is a macOS privacy
          restriction — the terminal app needs <strong>Full Disk Access</strong> granted in{" "}
          <strong>System Settings › Privacy &amp; Security › Full Disk Access</strong> to read{" "}
          <code>~/code</code> from certain contexts. Here's what's actually on disk:
        </p>

        {/* ascii table mocked after Warp's screenshot */}
        <div className="ascii-table">
<span className="at-r">┌──────┬───────────────────────────────┬──────────┐</span>{"\n"}
<span className="at-r">│ </span><span className="at-h">Type</span><span className="at-r"> │ </span><span className="at-h">Name</span><span className="at-r">                          │ </span><span className="at-h">Modified</span><span className="at-r"> │</span>{"\n"}
<span className="at-r">├──────┼───────────────────────────────┼──────────┤</span>{"\n"}
<span className="at-r">│ </span><span className="at-file">file</span><span className="at-r"> │ </span><span className="at-file">Cargo.toml</span><span className="at-r">                    │ Apr 18   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-dir">dir </span><span className="at-r"> │ </span><span className="at-dir">src/</span><span className="at-r">                          │ Apr 18   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-dir">dir </span><span className="at-r"> │ </span><span className="at-dir">assets/</span><span className="at-r">                       │ Apr 15   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-dir">dir </span><span className="at-r"> │ </span><span className="at-dir">dist/</span><span className="at-r">                         │ Mar 22   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-file">file</span><span className="at-r"> │ </span><span className="at-file">Dioxus.toml</span><span className="at-r">                   │ Apr 10   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-file">file</span><span className="at-r"> │ </span><span className="at-file">README.md</span><span className="at-r">                     │ Apr 08   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-img">img </span><span className="at-r"> │ </span><span className="at-img">assets/wordmark.svg</span><span className="at-r">           │ Mar 12   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-img">img </span><span className="at-r"> │ </span><span className="at-img">assets/scanner.svg</span><span className="at-r">            │ Mar 12   │</span>{"\n"}
<span className="at-r">│ </span><span className="at-file">file</span><span className="at-r"> │ </span><span className="at-file">assets/glyphs.txt</span><span className="at-r">             │ Mar 12   │</span>{"\n"}
<span className="at-r">└──────┴───────────────────────────────┴──────────┘</span>
        </div>

        <p>
          The project is intact. The restart will be clean. I'll grant full-disk access via{" "}
          <code>tccutil</code> first, then bring the dev server back up on port{" "}
          <span className="path">:8080</span>:
        </p>
      </AgentBlock>

      {/* Block 3: agent running a tool live */}
      <AgentBlock
        cmd="/agent"
        arg="restart dioxus dev-server and verify /docs route"
        running
        tool={{ name: "shell: cargo dx serve --platform web --port 8080" }}
        tokens="14.4k / 200k"
        credits="5.9 credits">
        <p style={{color: "var(--fg-dim)"}}>
          spinning up wasm target · hot-reload watcher armed · will tail stdout until ready-signal…
        </p>
      </AgentBlock>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────
// Root desktop app
// ─────────────────────────────────────────────────────────────
function DesktopApp() {
  const [sidebarOpen, setSidebarOpen] = React.useState(true);

  const sessions = [
    { id: "s1", kind: "agent",  title: "fix permissions · restart dioxus", sub: "running cargo dx serve", when: "now",  status: "running" },
    { id: "s2", kind: "md",     title: "ROADMAP.md",                       sub: "Q2: batch mode + web UI",   when: "2m",  status: "ok" },
    { id: "s3", kind: "md",     title: "PROJECT.md",                       sub: "ironhermes overview",       when: "14m", status: "ok" },
    { id: "s4", kind: "md",     title: "03-02-PLAN.md",                    sub: "skills protocol draft",     when: "37m", status: "ok" },
    { id: "s5", kind: "md",     title: "03-02-SUMMARY.md",                 sub: "weekly rollup",             when: "1h",  status: "ok" },
    { id: "s6", kind: "cmd",    title: "cargo test --workspace",           sub: "47 passed · 0 failed",      when: "2h",  status: "ok" },
    { id: "s7", kind: "agent",  title: "refactor tokio::select in gateway",sub: "6 turns · paused",          when: "3h",  status: "ok" },
    { id: "s8", kind: "server", title: "dioxus dev-server · :8080",        sub: "wasm32 · hot-reload on",    when: "—",   status: "running" },
    { id: "s9", kind: "cmd",    title: "ripgrep 'SOUL.md' crates/",        sub: "14 matches in 3 files",     when: "yesterday", status: "ok" },
    { id: "s10", kind: "agent", title: "draft README for skills protocol", sub: "concise preset · 4 turns",  when: "Mon", status: "ok" },
  ];
  const [activeId, setActiveId] = React.useState("s1");

  const tabs = [
    { id: "t1", label: "~/code/ironhermes/site", state: "running" },
    { id: "t2", label: "list documents directory", state: "ok" },
    { id: "t3", label: "doctor — env audit", state: "ok" },
  ];
  const [activeTab, setActiveTab] = React.useState("t1");

  const server = {
    running: true,
    port: 8080,
    build: "debug",
    rpm: 42,
  };

  return (
    <div className={"mac-win" + (window.__IHM_SCANLINES ? " scanlines" : "")}>
      <TitleBar sidebarOpen={sidebarOpen} setSidebarOpen={setSidebarOpen}/>
      <div className="mac-body">
        {sidebarOpen && (
          <Sidebar
            sessions={sessions}
            activeId={activeId}
            onActivate={setActiveId}
            server={server}/>
        )}
        <div className="content">
          <TabBar
            tabs={tabs}
            activeId={activeTab}
            onActivate={setActiveTab}
            onClose={(id) => console.log("close", id)}/>
          <Breadcrumb
            title="List Documents Directory Contents"
            taskState="running"/>
          <ChatSession/>
          <Composer model="auto (claude-sonnet-4)"/>
        </div>
      </div>
    </div>
  );
}

window.DesktopApp = DesktopApp;
