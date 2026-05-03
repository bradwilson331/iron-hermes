// Warp × IronHermes — full prototype app
// One <WarpHermes/> instance = one terminal. Drives:
//   - shell stream (blocks)
//   - agent panel
//   - input modes (Shell / Agent)
//   - command palette
//   - personality preset overlay
// Side-effect-free aside from setTimeout for fake streaming.

const PERSONALITIES = ["concise", "technical", "noir", "hype", "catgirl", "default"];

const PALETTE_ITEMS = [
  { section: "slash", cmd: "/help",        label: "Show available commands",  kbd: ["?"] },
  { section: "slash", cmd: "/status",      label: "IronHermes status",        kbd: ["⌘","I"] },
  { section: "slash", cmd: "/doctor",      label: "Run doctor checks",        kbd: [] },
  { section: "slash", cmd: "/personality", label: "Change personality preset", kbd: [] },
  { section: "slash", cmd: "/clear",       label: "Clear scrollback",          kbd: ["⌘","K"] },
  { section: "slash", cmd: "/quit",        label: "Exit chat",                  kbd: ["⌘","Q"] },
  { section: "workflow", cmd: "git status",      label: "Git: working tree status" },
  { section: "workflow", cmd: "cargo build",     label: "Cargo: build workspace" },
  { section: "workflow", cmd: "ironhermes chat", label: "Start chat session" },
  { section: "workflow", cmd: "ironhermes doctor", label: "Run config doctor" },
];

const STATUS_TEXT = `IronHermes Status
────────────────────────────────────────
  Home:     ~/.ironhermes/
  Model:    anthropic/claude-sonnet-4-20250514
  Provider: anthropic
  Terminal: bash
  Web:      firecrawl

API Keys
  OpenRouter:  configured
  Anthropic:   configured
  OpenAI:      not set`;

const DOCTOR_LINES = [
  { ok: true,  k: "Rust toolchain",  v: "1.81.0 stable" },
  { ok: true,  k: "Cargo workspace", v: "7 crates · 360k LOC" },
  { ok: true,  k: "config.yaml",     v: "~/.ironhermes/config.yaml" },
  { ok: true,  k: ".env",            v: "loaded · 14 vars" },
  { ok: true,  k: "Anthropic key",   v: "configured" },
  { ok: false, k: "OpenAI key",      v: "not set" },
  { ok: true,  k: "SOUL.md",         v: "found · 412 lines" },
  { ok: true,  k: "Memory store",    v: "sqlite · 1,284 entries" },
];

// Default seed transcript so the prototype lands looking real.
function seedBlocks() {
  return [
    {
      id: "b1", kind: "cmd",
      cmd: { parts: [
        { kind: "bin", t: "ironhermes" },
        { kind: "arg", t: "doctor" },
      ], time: "0.4s" },
    },
    {
      id: "b2", kind: "out", author: "doctor", time: "00:14:02",
      render: () => (
        <div>
          <div style={{ color: "var(--accent-primary)", fontWeight: 700, marginBottom: 6 }}>IronHermes Doctor</div>
          <div style={{ color: "var(--fg-dim)", marginBottom: 8 }}>{"────────────────────────────────────────"}</div>
          {DOCTOR_LINES.map((l, i) => (
            <div key={i} style={{ display: "flex", gap: 12, fontFamily: "var(--font-mono)" }}>
              <span style={{ color: l.ok ? "var(--success)" : "var(--warn)", fontWeight: 700, width: 90 }}>
                {l.ok ? "[OK]" : "[MISSING]"}
              </span>
              <span style={{ color: "var(--fg-strong)", width: 160 }}>{l.k}</span>
              <span style={{ color: l.ok ? "var(--fg)" : "var(--warn)" }}>{l.v}</span>
            </div>
          ))}
        </div>
      ),
    },
    {
      id: "b3", kind: "cmd",
      cmd: { parts: [
        { kind: "bin", t: "git" },
        { kind: "arg", t: "diff" },
        { kind: "flag", t: "--stat" },
      ], time: "0.1s" },
    },
    {
      id: "b4", kind: "ok", author: "git", time: "00:14:31",
      render: () => (
        <pre style={{ margin: 0, color: "var(--fg)" }}>
{` crates/ironhermes-cli/src/tui/render.rs       | 18 +++++++++++--
 crates/ironhermes-cli/src/tui/status_line.rs  |  6 +++
 crates/ironhermes-agent/src/personality.rs    | 24 ++++++++++++--
 3 files changed, 42 insertions(+), 6 deletions(-)`}
        </pre>
      ),
    },
    {
      id: "b5", kind: "ai", author: "Hermes", time: "00:14:48",
      render: () => (
        <div>
          The diff looks clean — the new <code>concise</code> personality slot is wired through{" "}
          <code>personality.rs</code> and the status line picks it up via the existing pill rotation.
          Want me to add a test that snapshots the rendered status line for each preset?
        </div>
      ),
    },
  ];
}

function seedMessages() {
  return [
    { who: "user",   time: "00:14:42", body: "Pull request feedback on the personality refactor — did I miss anything?" },
    { who: "hermes", time: "00:14:43", tool: { name: "read_file", args: '{"path":"crates/ironhermes-agent/src/personality.rs"}', status: "done" } },
    { who: "hermes", time: "00:14:46", body: "I'll read the file first…\n\nThe new preset registry is clean. One nit: `personality.rs:84` builds the system-prompt prefix with `format!`, but the old code interned it via `PROMPT_CACHE`. Worth restoring to avoid an alloc per turn." },
    { who: "user",   time: "00:14:50", body: "Good catch. Patch it." },
    { who: "hermes", time: "00:14:51", tool: { name: "edit_file", args: '{"path":"...","find":"format!","replace":"PROMPT_CACHE.intern(format!"}', status: "running" } },
  ];
}

// ─── Main shell ───────────────────────────────────────────────
function WarpHermes({ variant = "classic", tweaks }) {
  const [blocks, setBlocks] = React.useState(seedBlocks);
  const [messages, setMessages] = React.useState(seedMessages);
  const [input, setInput] = React.useState("");
  const [mode, setMode] = React.useState("shell");
  const [focused, setFocused] = React.useState(false);
  const [palOpen, setPalOpen] = React.useState(false);
  const [palQuery, setPalQuery] = React.useState("");
  const [activeTab, setActiveTab] = React.useState(0);
  const [scannerOn, setScannerOn] = React.useState(true);

  // global key handler — ⌘K opens palette
  React.useEffect(() => {
    const fn = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault(); setPalOpen(o => !o);
      }
      if (e.key === "Escape") setPalOpen(false);
    };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, []);

  // turn the scanner on briefly when activity happens
  function pulseScanner(ms = 1400) {
    setScannerOn(true);
    clearTimeout(pulseScanner._t);
    pulseScanner._t = setTimeout(() => setScannerOn(false), ms);
  }

  // submit handler — routes to shell or agent
  function submit() {
    const text = input.trim();
    if (!text) return;
    setInput("");
    pulseScanner(2000);
    if (mode === "agent") {
      runAgent(text);
    } else {
      runShell(text);
    }
  }

  function runShell(text) {
    const id = "b" + Date.now();
    setBlocks(bs => [...bs, {
      id, kind: "cmd",
      cmd: { parts: [{ kind: "bin", t: text.split(" ")[0] }, ...text.split(" ").slice(1).map(t => ({ kind: t.startsWith("-") ? "flag" : "arg", t }))], time: "…" }
    }]);
    // fake an output
    setTimeout(() => {
      const out = fakeShellOut(text);
      setBlocks(bs => [...bs, { id: id + "o", ...out }]);
    }, 600);
  }

  function runAgent(text) {
    const t = nowTime();
    setMessages(ms => [...ms, { who: "user", time: t, body: text }]);
    setTimeout(() => {
      setMessages(ms => [...ms, { who: "hermes", time: nowTime(), tool: { name: "search", args: `{"q":${JSON.stringify(text.slice(0, 40))}}`, status: "running" } }]);
    }, 400);
    setTimeout(() => {
      setMessages(ms => [...ms, { who: "hermes", time: nowTime(), body: fakeAgentReply(text, tweaks?.personality || "default") }]);
    }, 1400);
  }

  function pick(item) {
    setPalOpen(false);
    setPalQuery("");
    if (item.section === "slash" && item.cmd === "/clear") { setBlocks([]); return; }
    if (item.section === "slash" && item.cmd === "/status") {
      setBlocks(bs => [...bs, { id: "s" + Date.now(), kind: "out", author: "ironhermes", time: nowTime(),
        render: () => <pre style={{ margin: 0 }}>{STATUS_TEXT}</pre> }]); return;
    }
    if (item.section === "slash" && item.cmd === "/help") {
      setBlocks(bs => [...bs, { id: "h" + Date.now(), kind: "out", author: "help", time: nowTime(),
        render: () => (
          <div>
            <div style={{ color: "var(--accent-primary)", fontWeight: 700, marginBottom: 4 }}>Available commands</div>
            {PALETTE_ITEMS.filter(p => p.section === "slash").map(p => (
              <div key={p.cmd} style={{ display: "flex", gap: 14 }}>
                <span style={{ color: "var(--success)", width: 130 }}>{p.cmd}</span>
                <span style={{ color: "var(--fg-dim)" }}>{p.label}</span>
              </div>
            ))}
          </div>
        ) }]); return;
    }
    setInput(item.cmd);
  }

  // build status bar
  const status = (
    <StatusBar
      mode={mode === "agent" ? "Agent" : "Chat"}
      model="claude-sonnet-4"
      provider="anthropic"
      tokens={{ used: 12300, max: 128000 }}
      scannerActive={scannerOn}
      hint={mode === "agent" ? "/personality · ⌃C cancel · ⌘K palette" : "/help · ⌃C cancel · ⌘K palette"}
    />
  );

  // tabs
  const tabs = [
    { label: "ironhermes chat",  live: true },
    { label: "cargo watch",      live: true },
    { label: "agent · scratch",  live: false },
  ];

  // wrapper data attrs from tweaks
  const wrap = {
    "data-theme":   tweaks?.theme   || "cyan",
    "data-density": tweaks?.density || "comfy",
    "data-block":   tweaks?.block   || "framed",
    "data-agent":   tweaks?.agent   || (variant === "bottom" ? "bottom" : variant === "inline" ? "hidden" : "right"),
  };

  // inline-sigil: agent replies appear as inline blocks instead of a side panel
  const showSidePanel = wrap["data-agent"] !== "hidden";

  return (
    <div className="wh-app" {...wrap}>
      <TitleBar tabs={tabs} activeTab={activeTab} onTab={setActiveTab} showTrafficLights={variant === "classic"} />
      <div className="wh-main">
        <div className="wh-stream">
          <div className="wh-stream-scroll">
            {blocks.map(b => <RenderBlock key={b.id} b={b} />)}
            {!showSidePanel && messages.filter(m => m.who === "hermes").map((m, i) => (
              <div key={"inl" + i} className="wh-block is-ai">
                <div className="wh-block-head" style={{ display: "flex", gap: 10, alignItems: "center" }}>
                  <Sigil size={20} />
                  <span className="wh-author">Hermes</span>
                  <span style={{ color: "var(--fg-dim)" }}>· {tweaks?.personality || "default"}</span>
                  <span style={{ marginLeft: "auto" }}>{m.time}</span>
                </div>
                <div className="wh-block-body">
                  {m.tool ? <ToolCall {...m.tool} /> : m.body}
                </div>
              </div>
            ))}
          </div>
          <InputBox mode={mode} value={input} onChange={setInput}
            onSubmit={submit} focused={focused}
            onFocus={() => setFocused(true)} onBlur={() => setFocused(false)} />
          {status}
        </div>
        {showSidePanel && (
          <AgentPanel messages={messages} personality={tweaks?.personality || "default"} onPalette={() => setPalOpen(true)} />
        )}
      </div>
      <CommandPalette open={palOpen} onClose={() => setPalOpen(false)} onPick={pick}
        items={PALETTE_ITEMS} query={palQuery} setQuery={setPalQuery} />
    </div>
  );
}

function RenderBlock({ b }) {
  if (b.kind === "cmd") {
    return (
      <div className="wh-block is-cmd">
        <div className="wh-block-actions">
          <button className="wh-icon-btn" title="copy">⎘</button>
          <button className="wh-icon-btn" title="rerun">↻</button>
        </div>
        <CommandLine parts={b.cmd.parts} time={b.cmd.time} />
      </div>
    );
  }
  return (
    <div className={"wh-block is-" + b.kind}>
      <div className="wh-block-head">
        {b.author && <span className="wh-author">{b.author}</span>}
        {b.time && <span style={{ marginLeft: "auto" }}>{b.time}</span>}
      </div>
      <div className="wh-block-actions">
        <button className="wh-icon-btn" title="copy">⎘</button>
      </div>
      <div className="wh-block-body">{b.render ? b.render() : b.body}</div>
    </div>
  );
}

function nowTime() {
  const d = new Date();
  return String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0") + ":" + String(d.getSeconds()).padStart(2, "0");
}

function fakeShellOut(text) {
  if (text.startsWith("git status")) {
    return { author: "git", time: nowTime(), kind: "ok", render: () => (
      <pre style={{ margin: 0 }}>{`On branch main
Your branch is up to date with 'origin/main'.

Changes not staged for commit:
  modified:   crates/ironhermes-cli/src/tui/render.rs
  modified:   crates/ironhermes-agent/src/personality.rs

no changes added to commit (use "git add" and/or "git commit -a")`}</pre>
    )};
  }
  if (text.startsWith("cargo")) {
    return { author: "cargo", time: nowTime(), kind: "ok", render: () => (
      <pre style={{ margin: 0 }}>{`   Compiling ironhermes-cli v0.4.1
   Compiling ironhermes-agent v0.4.1
    Finished \`dev\` profile [unoptimized + debuginfo] in 4.82s`}</pre>
    )};
  }
  if (text.startsWith("ls")) {
    return { author: "ls", time: nowTime(), kind: "out", render: () => (
      <pre style={{ margin: 0 }}>{`Cargo.toml   README.md   crates/   target/   .ironhermes/`}</pre>
    )};
  }
  return { author: "sh", time: nowTime(), kind: "ok", render: () => (
    <span style={{ color: "var(--fg-dim)" }}>{`(simulated) ran: ${text}`}</span>
  ) };
}

function fakeAgentReply(prompt, personality) {
  const base = `On it. I'll inspect the relevant files and propose a patch — give me a moment to read through what you have.`;
  switch (personality) {
    case "concise":   return "Will do. Reading now.";
    case "technical": return "Acknowledged. Inspecting `crates/ironhermes-cli/src/tui/render.rs` and adjacent modules; diff incoming.";
    case "noir":      return "Another case. The file's hiding something — they always are. I'll crack it open.";
    case "hype":      return "OH HECK YES, ON IT! READING THE CODE NOW! THIS IS GOING TO BE INCREDIBLE! ⚡";
    case "catgirl":   return "nya~ ok! reading the file rn (=^.^=) gimme a sec~";
    default:          return base;
  }
}

window.WarpHermes = WarpHermes;
window.PERSONALITIES = PERSONALITIES;
