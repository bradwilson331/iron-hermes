// IOSApp.jsx — iOS-form-factor companion to IronHermes CLI.
// CRITICAL: the real IronHermes README says mobile is explicitly unsupported.
// This mockup embraces that: the iPhone frame renders the same monospace,
// ANSI-colored, box-drawing TUI as the desktop. No colorful icons, no emoji,
// no iOS glass pills, no rounded buttons. The "iOS-ness" is ONLY the shell.

const HR = (n, ch="─") => ch.repeat(n);

// ── Shared bits ─────────────────────────────────────────────
function IosStatusBar({ time = "9:41", right = "LTE · 82%" }) {
  return (
    <div className="ios-status">
      <span className="t">{time}</span>
      <span className="right">{right}</span>
    </div>
  );
}

function PromptRow({ value = "", ph = "type a message…" }) {
  return (
    <div className="ios-prompt">
      <span className="prefix">you ›</span>
      <input defaultValue={value} placeholder={ph} spellCheck={false}/>
    </div>
  );
}

function StatusLine({ mode = "chat", model = "sonnet-4", provider = "anthropic", tokens = "14.2k / 200k" }) {
  return (
    <div className="ios-statusline">
      <span className="pill-0">[{mode}]</span>{" "}
      <span className="pill-1">[{model}]</span>{" "}
      <span className="pill-2">[{provider}]</span>{" "}
      <span className="pill-3">[{tokens}]</span>
    </div>
  );
}

// ── Screen A: Sessions list ─────────────────────────────────
function SessionsScreen() {
  const sessions = [
    { t: "refactor tokio::select in gateway", p: "~/code/ironhermes", when: "2m",  active: true },
    { t: "draft README for skills protocol",  p: "~/code/ironhermes", when: "37m" },
    { t: "why does SOUL.md drift on reload",  p: "~/code/ironhermes", when: "2h" },
    { t: "port knight-rider to web canvas",   p: "~/code/experiments", when: "yesterday" },
    { t: "debug: cron overlapping runs",      p: "~/.ironhermes",     when: "Mon" },
    { t: "telegram gateway keepalive",        p: "~/.ironhermes",     when: "Sun" },
    { t: "batch mode: CSV enrichment run",    p: "~/code/hermes-jobs", when: "Oct 18" },
  ];
  return (
    <>
      <div className="ios-head">
        <div className="title"><span className="mark"></span>Sessions</div>
        <div className="sub">$ ironhermes sessions — sorted by last-touched</div>
        <div className="stats">
          <span><span className="k">total</span> <span className="v">23</span></span>
          <span><span className="k">active</span> <span className="v">1</span></span>
          <span><span className="k">resumable</span> <span className="v">19</span></span>
        </div>
      </div>
      <div className="ios-body">
        {sessions.map((s, i) => (
          <div key={i} className={"sess-row" + (s.active ? " active" : "")}>
            <div className="t">{s.active ? "› " : "  "}{s.t}</div>
            <div className="when">{s.when}</div>
            <div className="p">  {s.p}</div>
          </div>
        ))}
        <div style={{color:"var(--fg-dim)", fontSize:11, padding:"10px 4px 0"}}>
          ── {sessions.length} of 23 ──  tap to resume · swipe to archive
        </div>
      </div>
      <div className="ios-foot">
        <PromptRow ph="search sessions or start a new one…"/>
      </div>
      <StatusLine mode="sessions" tokens="23 total"/>
    </>
  );
}

// ── Screen B: Active chat (with live scanner) ──────────────
function ChatScreen() {
  return (
    <>
      <div className="ios-head">
        <div className="title"><span className="mark"></span>refactor tokio::select</div>
        <div className="sub">~/code/ironhermes · soul: concise · 6 turns</div>
      </div>
      <div className="ios-body">
        <div className="msg"><span className="you">you ›</span> the gateway.rs select! macro is getting<br/>unreadable. refactor without changing semantics.</div>

        <div className="msg"><span className="hermes">hermes ›</span> reading the file first.</div>

        <div className="msg tool">
          <span className="tn">• read_file</span> <span style={{color:"var(--fg)"}}>src/gateway.rs</span>{"  "}
          <span style={{color:"var(--success)"}}>(ok, 412 lines)</span>
        </div>

        <div className="msg"><span className="hermes">hermes ›</span> three receivers, two timers, one<br/>shutdown signal. splitting into an enum + a<br/>single loop. writing the change.</div>

        <div className="msg tool">
          <span className="tn">• edit_file</span> <span style={{color:"var(--fg)"}}>src/gateway.rs</span>{"\n"}
          <span style={{color:"var(--fg-dim)"}}>   −  tokio::select! {"{"} biased; ...48 lines... {"}"}</span>{"\n"}
          <span style={{color:"var(--fg-dim)"}}>   +  while let Some(ev) = events.recv().await {"{"}</span>
        </div>

        <div className="msg"><span className="hermes">hermes ›</span> running tests.</div>
        <Scanner toolName="cargo test --package gateway"/>
      </div>
      <div className="ios-foot">
        <PromptRow ph="reply to hermes…"/>
      </div>
      <StatusLine/>
    </>
  );
}

// ── Screen C: Doctor ───────────────────────────────────────
function DoctorScreen() {
  return (
    <>
      <div className="ios-head">
        <div className="title"><span className="mark"></span>Doctor</div>
        <div className="sub">$ ironhermes doctor — environment audit</div>
      </div>
      <div className="ios-body">
        <div className="sect">API Keys</div>
        <div className="sect-rule">{HR(28)}</div>
        <div className="kv">
          <div><span className="k">anthropic</span>  <span className="v ok">[OK] configured</span></div>
          <div><span className="k">openrouter</span> <span className="v ok">[OK] configured</span></div>
          <div><span className="k">openai</span>     <span className="v miss">[MISSING]</span></div>
          <div><span className="k">gemini</span>     <span className="v miss">[MISSING]</span></div>
        </div>

        <div className="sect">Binaries</div>
        <div className="sect-rule">{HR(28)}</div>
        <div className="kv">
          <div><span className="k">cargo</span>    <span className="v ok">[OK]</span>  <span style={{color:"var(--fg-dim)"}}>1.85.0</span></div>
          <div><span className="k">rustc</span>    <span className="v ok">[OK]</span>  <span style={{color:"var(--fg-dim)"}}>1.85.0</span></div>
          <div><span className="k">ripgrep</span>  <span className="v ok">[OK]</span>  <span style={{color:"var(--fg-dim)"}}>14.1.0</span></div>
          <div><span className="k">fd</span>       <span className="v ok">[OK]</span>  <span style={{color:"var(--fg-dim)"}}>10.2.0</span></div>
          <div><span className="k">git</span>      <span className="v ok">[OK]</span>  <span style={{color:"var(--fg-dim)"}}>2.48.1</span></div>
        </div>

        <div className="sect">Gateway</div>
        <div className="sect-rule">{HR(28)}</div>
        <div className="kv">
          <div><span className="k">telegram</span>  <span className="v ok">[OK]</span></div>
          <div><span className="k">bot</span>       <span className="v b">@ironhermesbot</span></div>
          <div><span className="k">port</span>      <span className="v b">8787</span></div>
          <div><span className="k">uptime</span>    <span className="v">4d 7h</span></div>
        </div>

        <div className="sect">Agent</div>
        <div className="sect-rule">{HR(28)}</div>
        <div className="kv">
          <div><span className="k">soul</span>     <span className="v b">concise</span></div>
          <div><span className="k">memory</span>   <span className="v">1,284 entries</span></div>
          <div><span className="k">skills</span>   <span className="v">12 installed</span></div>
          <div><span className="k">sessions</span> <span className="v">23 stored</span></div>
        </div>

        <div style={{color:"var(--fg-dim)", fontSize: 11, marginTop: 16, lineHeight: 1.55}}>
          {HR(28)}{"\n"}
          2 warnings · 0 errors{"\n"}
          run <span style={{color:"var(--accent-primary)"}}>`ironhermes doctor --fix`</span> to{"\n"}
          populate missing keys interactively.
        </div>
      </div>
      <StatusLine mode="doctor" tokens="—"/>
    </>
  );
}

// ── The iPhone shell ───────────────────────────────────────
function IPhone({ screen }) {
  return (
    <div className="iphone">
      <div className="screen-in">
        <div className="island"/>
        <IosStatusBar/>
        {screen === "sessions" && <SessionsScreen/>}
        {screen === "chat"     && <ChatScreen/>}
        {screen === "doctor"   && <DoctorScreen/>}
      </div>
      <div className="home"/>
    </div>
  );
}

// ── The iOS stage: one phone + explanatory caption ─────────
function IOSScreen({ screen, onScreen }) {
  const screens = [
    { id: "sessions", label: "Sessions" },
    { id: "chat",     label: "Chat" },
    { id: "doctor",   label: "Doctor" },
  ];
  return (
    <div className="ios-stage">
      <IPhone screen={screen}/>
      <div className="ios-caption">
        <span className="pill">// out-of-scope</span>
        <h3>iOS companion</h3>
        <p>
          The real IronHermes README is explicit: mobile is out-of-scope. This mockup
          honors that constraint by refusing to translate the CLI into iOS idioms. No
          bottom tab bar. No large titles. No liquid-glass pills. No colorful app icons.
        </p>
        <p>
          Every surface is monospace, ANSI-colored, and rendered with box-drawing
          characters — the same rules as the terminal. The phone is just a viewport.
        </p>

        <div className="rule">{HR(28)}</div>
        <dl>
          <dt>Type</dt>
          <dd>JetBrains Mono — one family, always.</dd>
          <dt>Color</dt>
          <dd>8-color ANSI palette + one brand accent. No gradients.</dd>
          <dt>Iconography</dt>
          <dd>Drawn in text. <code style={{color:"var(--accent-primary)"}}>[+]</code>{" "}
              <code style={{color:"var(--warn)"}}>[MISSING]</code>{" "}
              <code style={{color:"var(--success)"}}>[OK]</code>.</dd>
          <dt>Scanner</dt>
          <dd>The 10-cell knight-rider sweeps at 100ms, identical to the desktop TUI.</dd>
        </dl>

        <div className="rule" style={{marginTop: 16}}>{HR(28)}</div>
        <div style={{color:"var(--fg-dim)", marginTop: 6}}>switch screen:</div>
        <div className="switch" style={{marginTop: 8}}>
          {screens.map(s => (
            <button key={s.id} aria-pressed={screen === s.id} onClick={() => onScreen(s.id)}>
              {s.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

window.IOSScreen = IOSScreen;
