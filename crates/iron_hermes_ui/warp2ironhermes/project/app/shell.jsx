// Warp × IronHermes — shell primitives
// Pure presentational components; state lives in app.jsx

// ─── Scanner (knight-rider, 10 cells, 100ms tick) ─────────────
function Scanner({ active = true }) {
  const [tick, setTick] = React.useState(0);
  React.useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setTick(t => (t + 1) % 18), 100);
    return () => clearInterval(id);
  }, [active]);
  // triangle wave: 0..9..0
  const lit = tick < 10 ? tick : 18 - tick;
  const cells = Array.from({ length: 10 }, (_, i) => {
    const d = Math.abs(i - lit);
    if (!active) return { ch: "░", cls: "" };
    if (d === 0) return { ch: "█", cls: "lit" };
    if (d === 1) return { ch: "▓", cls: "t1" };
    if (d === 2) return { ch: "▒", cls: "t2" };
    return { ch: "░", cls: "" };
  });
  return (
    <span className="wh-scanner" aria-hidden="true">
      {cells.map((c, i) => <span key={i} className={c.cls}>{c.ch}</span>)}
    </span>
  );
}

// ─── Status bar (dot-pill bar) ────────────────────────────────
function StatusBar({ mode, model, provider, tokens, scannerActive, hint }) {
  const used = Math.round((tokens.used / tokens.max) * 100);
  return (
    <div className="wh-status">
      <span className="wh-pill" style={{ color: "var(--pill-0)" }}>{mode}</span>
      <span className="wh-sep">·</span>
      <span className="wh-pill" style={{ color: "var(--pill-1)" }}>{model}</span>
      <span className="wh-sep">·</span>
      <span className="wh-pill" style={{ color: "var(--pill-2)" }}>{provider}</span>
      <span className="wh-sep">·</span>
      <span className="wh-pill" style={{ color: "var(--pill-3)" }}>
        {(tokens.used/1000).toFixed(1)}K/{(tokens.max/1000).toFixed(0)}K ({used}%)
      </span>
      {scannerActive && (<>
        <span className="wh-sep">·</span>
        <Scanner active={scannerActive} />
      </>)}
      <span className="wh-hint">{hint}</span>
    </div>
  );
}

// ─── IronHermes shield (small text-stamp) ─────────────────────
function Sigil({ size = 26 }) {
  return (
    <span className="wh-sigil" style={{ width: size, height: size, fontSize: size * 0.46 }}>
      IH
    </span>
  );
}

// ─── Title bar with tabs ──────────────────────────────────────
function TitleBar({ tabs, activeTab, onTab, showTrafficLights = false }) {
  return (
    <div className="wh-titlebar">
      {showTrafficLights && (
        <div style={{ display: "flex", gap: 8, alignItems: "center", paddingRight: 8 }}>
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#ff5f57" }} />
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#febc2e" }} />
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#28c840" }} />
        </div>
      )}
      <div style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--accent-primary)", fontWeight: 700, fontSize: 12, paddingRight: 12, borderRight: "1px solid var(--w-border)", height: "100%" }}>
        <Sigil size={18} />
        <span>IronHermes</span>
      </div>
      <div className="wh-tabs">
        {tabs.map((t, i) => (
          <div key={i} className={"wh-tab" + (i === activeTab ? " is-active" : "")} onClick={() => onTab && onTab(i)}>
            <span className="wh-tab-dot" style={{ background: t.live ? "var(--success)" : "var(--fg-dim)" }} />
            <span>{t.label}</span>
            <span style={{ color: "var(--fg-disabled)", marginLeft: 4, fontSize: 11 }}>×</span>
          </div>
        ))}
        <div style={{ display: "flex", alignItems: "center", padding: "0 10px", color: "var(--fg-dim)", fontSize: 14, fontWeight: 700 }}>+</div>
      </div>
      <div className="wh-titlebar-actions">
        <span style={{ fontSize: 11 }}>⌘K</span>
      </div>
    </div>
  );
}

// ─── Block components ─────────────────────────────────────────
function Block({ kind = "out", author, time, children, exitCode }) {
  return (
    <div className={"wh-block is-" + kind}>
      {(author || time) && (
        <div className="wh-block-head">
          {author && <span className="wh-author">{author}</span>}
          {kind === "ok"  && <span style={{ color: "var(--success)" }}>[OK]</span>}
          {kind === "err" && <span style={{ color: "var(--danger)" }}>exit {exitCode ?? 1}</span>}
          {time  && <span style={{ marginLeft: "auto" }}>{time}</span>}
        </div>
      )}
      <div className="wh-block-actions">
        <button className="wh-icon-btn" title="copy">⎘</button>
        <button className="wh-icon-btn" title="rerun">↻</button>
        <button className="wh-icon-btn" title="share">↗</button>
      </div>
      <div className="wh-block-body">{children}</div>
    </div>
  );
}

function CommandLine({ cwd = "~/projects/ironhermes", glyph = "❯", parts, time }) {
  return (
    <div className="wh-cmdline">
      <span style={{ color: "var(--fg-dim)", fontSize: 11 }}>{cwd}</span>
      <span className="wh-prompt-glyph">{glyph}</span>
      <span style={{ flex: 1 }}>
        {parts.map((p, i) => (
          <span key={i} className={"wh-cmd-" + (p.kind || "arg") + (p.kind === "bin" ? " wh-cmd" : "")} style={p.kind === "bin" ? { color: "var(--fg-strong)", fontWeight: 700 } : undefined}>
            {i > 0 ? " " : ""}{p.t}
          </span>
        ))}
      </span>
      {time && <span className="wh-cmd-time">{time}</span>}
    </div>
  );
}

function ToolCall({ name, args, status = "running" }) {
  return (
    <div className="wh-toolcall">
      <div style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
        <span style={{ color: "var(--fg-dim)" }}>Tool:</span>
        <b>{name}</b>
        <span style={{ marginLeft: "auto", fontSize: 10, color: status === "done" ? "var(--success)" : "var(--warn)" }}>
          {status === "done" ? "[OK]" : "running…"}
        </span>
      </div>
      {args && (
        <pre style={{ margin: "4px 0 0", color: "var(--fg-dim)", fontSize: 11, whiteSpace: "pre-wrap" }}>{args}</pre>
      )}
    </div>
  );
}

// ─── Input box ────────────────────────────────────────────────
function InputBox({ mode, value, onChange, onSubmit, focused, onFocus, onBlur }) {
  return (
    <div className={"wh-input-wrap" + (focused ? " is-focus" : "")}>
      <div className="wh-input-mode">
        <span className={"wh-mode-pill" + (mode === "agent" ? " is-agent" : "")}>{mode === "agent" ? "Agent" : "Shell"}</span>
        <span>⌥+M to switch</span>
        <span style={{ marginLeft: "auto" }}>↵ run · ⇧↵ newline · ⌃C cancel</span>
      </div>
      <div className="wh-input-row">
        <span className="wh-prompt-glyph">{mode === "agent" ? "✦" : "❯"}</span>
        <textarea
          className="wh-textarea"
          rows={1}
          placeholder={mode === "agent" ? "Ask IronHermes anything…" : "Type a command, or `/` for commands"}
          value={value}
          onChange={e => onChange(e.target.value)}
          onKeyDown={e => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              onSubmit();
            }
          }}
          onFocus={onFocus}
          onBlur={onBlur}
        />
        <div className="wh-input-actions">
          <button className="wh-icon-btn" title="attach">@</button>
          <button className="wh-icon-btn" title="voice">●</button>
          <button className="wh-icon-btn" title="run" style={{ color: "var(--accent-primary)" }}>↵</button>
        </div>
      </div>
    </div>
  );
}

// ─── Agent side panel ─────────────────────────────────────────
function AgentPanel({ messages, personality, onPalette }) {
  const ref = React.useRef(null);
  React.useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [messages.length]);
  return (
    <aside className="wh-side">
      <div className="wh-side-head">
        <Sigil size={20} />
        <span className="wh-side-title">HERMES</span>
        <span className="wh-personality" onClick={onPalette} style={{ cursor: "default" }}>/{personality}</span>
      </div>
      <div className="wh-side-scroll" ref={ref}>
        {messages.map((m, i) => (
          <div key={i} className={"wh-msg is-" + m.who}>
            <div className="wh-msg-meta">
              <b>{m.who === "user" ? "You" : "Hermes"}</b>
              <span>{m.time}</span>
            </div>
            {m.tool ? <ToolCall {...m.tool} /> : <div className="wh-msg-body">{m.body}</div>}
          </div>
        ))}
      </div>
    </aside>
  );
}

// ─── Command Palette ──────────────────────────────────────────
function CommandPalette({ open, onClose, onPick, items, query, setQuery }) {
  if (!open) return null;
  const filtered = items.filter(it =>
    !query || it.label.toLowerCase().includes(query.toLowerCase()) || (it.cmd || "").toLowerCase().includes(query.toLowerCase())
  );
  const [active, setActive] = React.useState(0);
  React.useEffect(() => { setActive(0); }, [query]);
  return (
    <div className="wh-pal-overlay" onClick={onClose}>
      <div className="wh-pal" onClick={e => e.stopPropagation()}>
        <div className="wh-pal-search">
          <span style={{ color: "var(--accent-primary)", fontWeight: 700 }}>⌘K</span>
          <input autoFocus placeholder="Search commands, files, recent…"
            value={query} onChange={e => setQuery(e.target.value)}
            onKeyDown={e => {
              if (e.key === "ArrowDown") { e.preventDefault(); setActive(a => Math.min(a + 1, filtered.length - 1)); }
              if (e.key === "ArrowUp")   { e.preventDefault(); setActive(a => Math.max(a - 1, 0)); }
              if (e.key === "Enter")     { e.preventDefault(); filtered[active] && onPick(filtered[active]); }
              if (e.key === "Escape")    { onClose(); }
            }} />
          <span className="wh-kbd">esc</span>
        </div>
        <div className="wh-pal-list">
          <div className="wh-pal-section">Slash commands</div>
          {filtered.filter(f => f.section === "slash").map((it, i) => {
            const idx = filtered.indexOf(it);
            return (
              <div key={it.cmd} className={"wh-pal-row" + (idx === active ? " is-active" : "")}
                   onMouseEnter={() => setActive(idx)} onClick={() => onPick(it)}>
                <span className="wh-pal-glyph">/</span>
                <span style={{ color: "var(--fg-strong)" }}>{it.cmd}</span>
                <span style={{ color: "var(--fg-dim)" }}>— {it.label}</span>
                <span className="wh-pal-hint">
                  <span className="wh-pal-kbd">{it.kbd && it.kbd.map((k, j) => <span key={j} className="wh-kbd">{k}</span>)}</span>
                </span>
              </div>
            );
          })}
          <div className="wh-pal-section">Workflows</div>
          {filtered.filter(f => f.section === "workflow").map((it) => {
            const idx = filtered.indexOf(it);
            return (
              <div key={it.cmd} className={"wh-pal-row" + (idx === active ? " is-active" : "")}
                   onMouseEnter={() => setActive(idx)} onClick={() => onPick(it)}>
                <span className="wh-pal-glyph">▸</span>
                <span style={{ color: "var(--fg-strong)" }}>{it.label}</span>
                <span style={{ color: "var(--fg-dim)" }}>{it.cmd}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

Object.assign(window, {
  Scanner, StatusBar, Sigil, TitleBar, Block, CommandLine, ToolCall,
  InputBox, AgentPanel, CommandPalette,
});
