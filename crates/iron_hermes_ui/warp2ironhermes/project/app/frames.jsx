// Variations — wraps WarpHermes in 4 different configurations × 3 device frames.

// ─── desktop (macOS) variant ──────────────────────────────────
function DesktopFrame({ children, width = 1180, height = 740 }) {
  return (
    <div style={{
      width, height,
      background: "#1c1f24",
      borderRadius: 12,
      boxShadow: "0 30px 80px rgba(0,0,0,.55), 0 0 0 1px rgba(255,255,255,.05)",
      overflow: "hidden",
      display: "flex", flexDirection: "column",
    }}>
      <div style={{
        height: 28, background: "#0a0e13",
        display: "flex", alignItems: "center", padding: "0 14px",
        borderBottom: "1px solid var(--w-border)",
      }}>
        <div style={{ display: "flex", gap: 8 }}>
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#ff5f57" }} />
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#febc2e" }} />
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#28c840" }} />
        </div>
        <span style={{ flex: 1, textAlign: "center", color: "var(--fg-dim)", fontSize: 12, fontFamily: "var(--font-mono)" }}>
          ironhermes — chat
        </span>
        <span style={{ width: 52 }} />
      </div>
      <div style={{ flex: 1, minHeight: 0 }}>{children}</div>
    </div>
  );
}

// ─── browser frame ────────────────────────────────────────────
function WebFrame({ children, width = 1180, height = 740 }) {
  return (
    <div style={{
      width, height,
      background: "#0a0e13",
      borderRadius: 12,
      boxShadow: "0 30px 80px rgba(0,0,0,.55), 0 0 0 1px rgba(255,255,255,.05)",
      overflow: "hidden",
      display: "flex", flexDirection: "column",
    }}>
      <div style={{
        height: 38, background: "#11161d",
        display: "flex", alignItems: "center", gap: 10, padding: "0 12px",
        borderBottom: "1px solid var(--w-border)",
      }}>
        <div style={{ display: "flex", gap: 8 }}>
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#ff5f57" }} />
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#febc2e" }} />
          <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#28c840" }} />
        </div>
        <div style={{ display: "flex", gap: 6, color: "var(--fg-dim)", fontSize: 12 }}>
          <span style={{ opacity: .5 }}>‹</span>
          <span style={{ opacity: .5 }}>›</span>
          <span style={{ opacity: .8 }}>↻</span>
        </div>
        <div style={{
          flex: 1, height: 22, background: "#0a0e13",
          border: "1px solid var(--w-border)",
          borderRadius: 6,
          display: "flex", alignItems: "center", padding: "0 10px",
          color: "var(--fg-dim)", fontSize: 11, fontFamily: "var(--font-mono)",
        }}>
          <span style={{ color: "var(--success)" }}>●</span>
          <span style={{ marginLeft: 8 }}>app.ironhermes.dev/chat</span>
          <span style={{ marginLeft: "auto", opacity: .5 }}>⋆</span>
        </div>
        <span style={{ color: "var(--fg-dim)", fontSize: 12 }}>⋯</span>
      </div>
      <div style={{ flex: 1, minHeight: 0 }}>{children}</div>
    </div>
  );
}

// ─── iOS frame (custom — terminal aesthetic, not the iOS kit) ──
function IOSPhoneFrame({ children, width = 380, height = 780 }) {
  return (
    <div style={{
      width: width + 16, height: height + 16,
      padding: 8,
      background: "#1a1d23",
      borderRadius: 56,
      boxShadow: "0 30px 80px rgba(0,0,0,.55), inset 0 0 0 1px rgba(255,255,255,.04)",
    }}>
      <div style={{
        width, height,
        background: "var(--w-bg-1)",
        borderRadius: 48,
        overflow: "hidden",
        position: "relative",
        display: "flex", flexDirection: "column",
      }}>
        {/* dynamic island */}
        <div style={{
          position: "absolute", top: 12, left: "50%", transform: "translateX(-50%)",
          width: 110, height: 32, background: "#000", borderRadius: 20, zIndex: 100,
        }} />
        {/* status bar */}
        <div style={{
          height: 50, flexShrink: 0, padding: "16px 32px 0",
          display: "flex", alignItems: "center", justifyContent: "space-between",
          color: "var(--fg-strong)", fontSize: 14, fontWeight: 600, fontFamily: "var(--font-mono)",
        }}>
          <span>9:41</span>
          <span style={{ width: 60 }} />
          <span style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <span style={{ fontSize: 11 }}>▮▮▮▯</span>
            <span style={{ fontSize: 11 }}>5G</span>
            <span style={{ fontSize: 11 }}>▰▰▰▱</span>
          </span>
        </div>
        <div style={{ flex: 1, minHeight: 0 }}>{children}</div>
        {/* home indicator */}
        <div style={{
          position: "absolute", bottom: 8, left: "50%", transform: "translateX(-50%)",
          width: 134, height: 5, background: "var(--fg-strong)", borderRadius: 3,
        }} />
      </div>
    </div>
  );
}

// ─── iOS-adapted shell (simpler chrome, sheet-based agent) ────
function WarpHermesMobile({ tweaks }) {
  const [tab, setTab] = React.useState("shell");
  const [blocks, setBlocks] = React.useState([
    { id: "m1", kind: "cmd", cmd: { parts: [{ kind: "bin", t: "ironhermes" }, { kind: "arg", t: "status" }], time: "0.2s" } },
    { id: "m2", kind: "out", author: "status", time: "9:41",
      render: () => (
        <pre style={{ margin: 0, fontSize: 12 }}>{`Home:     ~/.ironhermes/
Model:    claude-sonnet-4
Provider: anthropic
Tokens:   12.3K/128K`}</pre>
      ),
    },
    { id: "m3", kind: "ai", author: "Hermes", time: "9:42",
      render: () => <span>Tap the chat tab to talk to me.</span> },
  ]);
  const [messages] = React.useState([
    { who: "user",   time: "9:42", body: "What's the next thing on the roadmap?" },
    { who: "hermes", time: "9:42", body: "Per `.planning/PROJECT.md`, next up is the Telegram gateway shipping with image-attachment support, then the cron scheduler." },
  ]);
  const [input, setInput] = React.useState("");
  const [palOpen, setPalOpen] = React.useState(false);

  const wrap = {
    "data-theme":   tweaks?.theme   || "cyan",
    "data-density": "compact",
    "data-block":   tweaks?.block   || "framed",
    "data-agent":   "hidden",
  };

  return (
    <div className="wh-app" style={{ borderRadius: 0 }} {...wrap}>
      {/* compact title */}
      <div className="wh-titlebar" style={{ height: 38, padding: "0 16px" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--accent-primary)", fontWeight: 700, fontSize: 13 }}>
          <Sigil size={18} />
          IronHermes
        </div>
        <span style={{ marginLeft: "auto", color: "var(--fg-dim)", fontSize: 11 }}>{tab === "shell" ? "shell" : tab === "chat" ? "agent" : "files"}</span>
      </div>
      <div className="wh-main" style={{ flexDirection: "column" }}>
        <div className="wh-stream">
          {tab === "shell" && (
            <div className="wh-stream-scroll" style={{ padding: 12, gap: 8 }}>
              {blocks.map(b => <RenderBlock key={b.id} b={b} />)}
            </div>
          )}
          {tab === "chat" && (
            <div className="wh-stream-scroll" style={{ padding: 12, gap: 12 }}>
              {messages.map((m, i) => (
                <div key={i} className={"wh-msg is-" + m.who}>
                  <div className="wh-msg-meta">
                    <b>{m.who === "user" ? "You" : "Hermes"}</b>
                    <span>{m.time}</span>
                  </div>
                  <div className="wh-msg-body">{m.body}</div>
                </div>
              ))}
            </div>
          )}
          {tab === "files" && (
            <div className="wh-stream-scroll" style={{ padding: 16, gap: 4 }}>
              <div style={{ color: "var(--fg-dim)", fontSize: 11, textTransform: "uppercase", letterSpacing: ".06em", marginBottom: 8 }}>~/.ironhermes/</div>
              {["config.yaml", ".env", "SOUL.md", "AGENTS.md", "memory.db", "skills/", "logs/"].map(f => (
                <div key={f} style={{ padding: "10px 4px", borderBottom: "1px solid var(--w-border)", display: "flex", gap: 10 }}>
                  <span style={{ color: f.endsWith("/") ? "var(--accent-primary)" : "var(--fg-dim)" }}>{f.endsWith("/") ? "▸" : "·"}</span>
                  <span>{f}</span>
                </div>
              ))}
            </div>
          )}
          <InputBox mode={tab === "chat" ? "agent" : "shell"} value={input} onChange={setInput}
            onSubmit={() => setInput("")} focused={false} onFocus={() => {}} onBlur={() => {}} />
        </div>
      </div>
      <div className="wh-mobile-tabs">
        <button className={tab === "shell" ? "is-active" : ""} onClick={() => setTab("shell")}>
          <span className="wh-mt-glyph">❯</span><span>shell</span>
        </button>
        <button className={tab === "chat" ? "is-active" : ""} onClick={() => setTab("chat")}>
          <span className="wh-mt-glyph">✦</span><span>hermes</span>
        </button>
        <button className={tab === "files" ? "is-active" : ""} onClick={() => setTab("files")}>
          <span className="wh-mt-glyph">▤</span><span>files</span>
        </button>
      </div>
    </div>
  );
}

Object.assign(window, { DesktopFrame, WebFrame, IOSPhoneFrame, WarpHermesMobile });
