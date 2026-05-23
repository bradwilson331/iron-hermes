// Site.jsx — Dioxus-style dev-tool landing page, rendered in strict IronHermes CLI aesthetic.
// All imagery is text (box-drawing, ASCII, ANSI color). No emoji. No rounded corners. No icons.

const HR40 = "─".repeat(40);
const HR20 = "─".repeat(20);

// ── Section: masthead ──
function Masthead() {
  return (
    <div className="masthead">
      <div className="mast-row">
        <div className="mast-copy">
          <h1 className="wordmark"><span>Iron</span><span className="hermes">Hermes</span></h1>
          <p className="tagline">
            A self-improving AI agent, rewritten in Rust.{" "}
            <em>One binary.</em> A CLI, a Telegram bot, a cron scheduler, and a batch processor —
            sharing the same 10-layer prompt, memory store, and skills hub.
          </p>
        </div>
        <div className="mast-shield" aria-label="IronHermes shield — caduceus">
          <img src="../assets/ih-shield-caduceus-transparent-600.png" alt=""/>
          <div className="mast-shield-chrome">
            <span className="chrome-top">╭─ crest ─╮</span>
            <span className="chrome-bottom">╰─ MMXXVI · iron &amp; hermes ─╯</span>
          </div>
        </div>
      </div>

      <div className="chips">
        <span className="chip hot"><span className="k">v</span><span className="v">2.0.0</span></span>
        <span className="chip ok">[OK]&nbsp;all 7 crates compile</span>
        <span className="chip"><span className="k">rust</span><span className="v">2024 edition</span></span>
        <span className="chip"><span className="k">stars</span><span className="v">2.1k</span></span>
        <span className="chip"><span className="k">license</span><span className="v">MIT · OFL</span></span>
      </div>

      <div className="cta-row">
        <a className="cta" href="#install">
          <span>$ cargo install ironhermes</span><span className="arrow">→</span>
        </a>
        <a className="cta ghost" href="#docs">
          <span>Read the docs</span>
        </a>
        <span className="hint"># supported: linux · macos · bsd — mobile is out-of-scope</span>
      </div>

      <div className="dio-ribbon">
        <span className="label">// dioxus</span>
        <span>
          This page is the canonical Dioxus-rendered mock of IronHermes' (non-existent)
          marketing surface. Every pixel is monospace, ANSI-colored, and drawable in the
          actual CLI — render it with{" "}
          <code>cargo run --bin dioxus_site</code>{" "}
          or pipe it through <code>less -R</code>.
        </span>
      </div>
    </div>
  );
}

// ── Section: install terminal ──
function InstallBlock() {
  return (
    <div className="row two">
      <div className="install" id="install">
        <div className="titlebar">
          <div className="dot"/><div className="dot"/><div className="dot"/>
          <span className="title">~ — zsh — 80×24</span>
        </div>
        <pre>
{`  `}<span className="prompt">›</span> <span className="cmd">cargo install ironhermes</span>{`
`}<span className="out">{`    Installing ironhermes v2.0.0 (registry \`crates-io\`)
       Compiling ironhermes-agent v2.0.0
       Compiling ironhermes-cli v2.0.0
        Finished \`release\` profile in 38.4s
       Installed package \`ironhermes v2.0.0\`
`}</span>
{`  `}<span className="prompt">›</span> <span className="cmd">ironhermes doctor</span>{`
`}<span className="out">
<span className="br">IronHermes Doctor</span>{"\n"}
<span className="cy">{HR40}</span>{"\n"}
{"  API Keys\n"}
{"    Anthropic:   "}<span className="ok">configured</span>{"\n"}
{"    OpenRouter:  "}<span className="ok">configured</span>{"\n"}
{"    OpenAI:      "}<span className="missing">[MISSING]</span>{"  not set\n"}
{"\n"}
{"  Binaries\n"}
{"    cargo:   "}<span className="ok">[OK]</span>{"   1.85.0\n"}
{"    rustc:   "}<span className="ok">[OK]</span>{"   1.85.0\n"}
{"    ripgrep: "}<span className="ok">[OK]</span>{"   14.1.0\n"}
{"\n"}
{"  Gateway\n"}
{"    Telegram: "}<span className="ok">[OK]</span>{"   bot: @ironhermesbot\n"}
{"    Port:     "}<span className="cy">8787</span>{" (listening)\n"}
</span>
{`  `}<span className="prompt">›</span> <span className="cmd">ironhermes chat</span><span className="blink" style={{color:"var(--accent-primary)"}}>▎</span>
</pre>
        <button className="copy" onClick={e => navigator.clipboard?.writeText("cargo install ironhermes").catch(()=>{})}>copy</button>
      </div>

      <div className="feat" style={{display:"flex", flexDirection:"column"}}>
        <span className="idx">[01]</span>
        <h3>What you actually get</h3>
        <p>
          A single Rust binary that ships four subcommands —{" "}
          <span style={{color:"var(--accent-primary)"}}>chat</span> ·{" "}
          <span style={{color:"var(--accent-primary)"}}>gateway</span> ·{" "}
          <span style={{color:"var(--accent-primary)"}}>cron</span> ·{" "}
          <span style={{color:"var(--accent-primary)"}}>batch</span>{" "}
          — and a config directory at <code style={{color:"var(--fg)"}}>~/.ironhermes/</code>.
          The agent edits its own <code style={{color:"var(--fg)"}}>SOUL.md</code> between turns.
        </p>
        <div className="keys" style={{marginTop:"auto"}}>
          <div>{"  "}<span className="k">home</span>{"    ~/.ironhermes/"}</div>
          <div>{"  "}<span className="k">config</span>{"  config.yaml · .env · MEMORY.md"}</div>
          <div>{"  "}<span className="k">soul</span>{"    SOUL.md · 14 presets · writable"}</div>
          <div>{"  "}<span className="k">skills</span>{"  ~/.ironhermes/skills/*.md"}</div>
        </div>
      </div>
    </div>
  );
}

// ── Section: three feature cards ──
function Features() {
  const items = [
    {
      n: "02",
      title: "10-layer prompt assembly",
      body: "Identity, memory, skills, context files, session overlays — stitched in deterministic order on every turn. Prompt cache breakpoints are automatic.",
      keys: [
        ["01", "identity"],
        ["02", "memory"],
        ["03", "user profile"],
        ["04", "project ctx"],
        ["05", "skills"],
        ["06", "soul overlay"],
      ],
    },
    {
      n: "03",
      title: "Self-improving SOUL.md",
      body: "The agent rewrites its own personality file between turns. 14 built-in presets — from concise to noir to catgirl — plus any you add. Snapshot is frozen for prompt-cache stability.",
      keys: [
        ["", "concise · technical · noir"],
        ["", "pirate · catgirl · hype"],
        ["", "teacher · socratic · …"],
      ],
    },
    {
      n: "04",
      title: "Knight-rider scanner",
      body: "A 10-cell scanner sweeps the TUI while tools run. 100ms ticks, triangle-wave, linear easing. Because mechanical confidence beats a spinner.",
      scanner: true,
    },
  ];
  return (
    <div className="row three">
      {items.map((it, i) => (
        <div className="feat" key={i}>
          <span className="idx">[{it.n}]</span>
          <h3>{it.title}</h3>
          <p>{it.body}</p>
          {it.keys && (
            <div className="keys">
              {it.keys.map(([k, v], j) => (
                <div key={j}>{"  "}<span className="k">{k ? k + " " : "· "}</span>{v}</div>
              ))}
            </div>
          )}
          {it.scanner && (
            <div className="keys" style={{marginTop: 14, fontSize: 14}}>
              <div style={{letterSpacing: 0}}>{"  "}<StaticScanner lit={3}/> <span style={{color:"var(--fg-dim)"}}>Streaming</span></div>
              <div style={{letterSpacing: 0}}>{"  "}<StaticScanner lit={6}/> <span style={{color:"var(--fg-dim)"}}>Running:</span> <span style={{color:"var(--warn)"}}>read_file</span></div>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

// ── Section: docs preview ──
function DocsPreview() {
  return (
    <div className="docs" id="docs">
      <div className="docs-split">
        <aside className="docs-nav">
          <div className="hdr">getting started</div>
          <a href="#" className="cur">overview</a>
          <a href="#">install</a>
          <a href="#">first chat</a>
          <a href="#">configure providers</a>

          <div className="hdr" style={{marginTop: 14}}>reference</div>
          <a href="#">subcommands</a>
          <a href="#">slash commands</a>
          <a href="#">config.yaml</a>
          <a href="#">SOUL.md schema</a>
          <a href="#">skills protocol</a>
          <a href="#">hooks catalog</a>

          <div className="hdr" style={{marginTop: 14}}>guides</div>
          <a href="#">writing a skill</a>
          <a href="#">context compression</a>
          <a href="#">telegram gateway</a>
          <a href="#">batch mode</a>
        </aside>
        <div className="docs-body">
          <h1>Overview</h1>
          <p className="lead">
            IronHermes is a single Rust binary. Installing it gives you one executable —{" "}
            <code>ironhermes</code> — with four long-running modes and a handful of one-shot
            utilities. There is no daemon, no web UI, no cloud account.
          </p>

          <h2>Minimal chat</h2>
          <pre>
<span className="com">// SOUL.md — concise preset, active</span>{"\n"}
<span className="kw">use</span> ironhermes::{"{Agent, Provider, Soul}"};{"\n"}{"\n"}
<span className="kw">fn</span> <span className="fn">main</span>() -&gt; ironhermes::Result&lt;()&gt; {"{"}{"\n"}
{"    "}<span className="kw">let</span> soul = Soul::preset(<span className="str">"concise"</span>);{"\n"}
{"    "}<span className="kw">let</span> provider = Provider::anthropic(<span className="str">"claude-sonnet-4-20250514"</span>);{"\n"}
{"    "}<span className="kw">let</span> <span className="kw">mut</span> agent = Agent::builder(){"\n"}
{"        "}.with_soul(soul){"\n"}
{"        "}.with_provider(provider){"\n"}
{"        "}.with_context_pressure(<span className="num">0.50</span>){"\n"}
{"        "}.build()?;{"\n"}{"\n"}
{"    "}agent.chat(<span className="str">"refactor this without loops"</span>)?;{"\n"}
{"    "}<span className="kw">Ok</span>(()){"\n"}
{"}"}
          </pre>

          <h2>Slash commands</h2>
          <p>
            Inside <code>ironhermes chat</code>, slash commands dispatch inline. They never
            open a modal. Output echoes into the transcript and the scanner appears
            if a tool fires.
          </p>
          <pre>
<span className="key">/quit</span>{"        "}<span className="com">// exit the session; SOUL snapshot is written</span>{"\n"}
<span className="key">/clear</span>{"       "}<span className="com">// flush transcript; keep memory + soul</span>{"\n"}
<span className="key">/status</span>{"      "}<span className="com">// pills inline: mode · model · provider · tokens</span>{"\n"}
<span className="key">/personality</span>{" "}<span className="str">"noir"</span>{"  "}<span className="com">// swap preset for this session</span>{"\n"}
<span className="key">/help</span>{"        "}<span className="com">// list all slash commands</span>
          </pre>

          <h2>What it is not</h2>
          <p style={{color:"var(--fg-dim)"}}>
            No web dashboard. No mobile app. No multi-user auth. No Discord/Slack.
            No plugin loader beyond the skills protocol. No settings GUI. If a feature
            would require any of those, it is explicitly out of scope — the surface is
            the terminal, and the terminal is the surface.
          </p>
        </div>
      </div>
    </div>
  );
}

// ── Footer ──
function SiteFooter() {
  return (
    <div className="sitefoot">
      <div className="left">
        <span style={{color:"var(--brand)", fontWeight:700}}>IronHermes</span>{" "}
        · v2.0.0 · built with <span style={{color:"var(--brand)"}}>dioxus</span>
        · <span style={{color:"var(--fg-dim)"}}>rendered offline</span>
      </div>
      <div>
        <a href="#">github</a>
        <a href="#">crates.io</a>
        <a href="#">docs.rs</a>
        <a href="#">nousresearch</a>
        <a href="#">rss</a>
      </div>
    </div>
  );
}

function LandingScreen() {
  return (
    <div className="site">
      <Masthead/>
      <InstallBlock/>
      <Features/>
      <div style={{marginTop: 32}}><DocsPreview/></div>
      <SiteFooter/>
    </div>
  );
}

window.LandingScreen = LandingScreen;
