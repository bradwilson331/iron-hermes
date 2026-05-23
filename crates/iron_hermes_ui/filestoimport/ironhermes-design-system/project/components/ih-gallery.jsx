// ih-gallery.jsx — Token gallery, component showcase, platform frames
// Needs: icons.jsx, ih-components.jsx, ih-screens.jsx, ih-screens-2.jsx
// MacWindow, IOSDevice, ChromeWindow from starter components.

// ============================================================
// DS GALLERY — tokens + components reference
// ============================================================
function TokenGallery() {
  return (
    <div style={{padding:"var(--sp-9)", background:"var(--bg-canvas)", height:"100%", overflow:"auto"}} className="ih-scroll">
      {/* Type scale */}
      <Section title="Type" meta="Iosekey Mono + Iosekey Mono Condensed + Inter">
        <div className="stack-lg">
          <div>
            <span className="t-display">IronHermes</span>
            <div className="t-micro fg-faint" style={{marginTop:4}}>t-display · Iosekey Mono Condensed 700 / 48</div>
          </div>
          <div>
            <span className="t-h1">The self-improving agent</span>
            <div className="t-micro fg-faint" style={{marginTop:4}}>t-h1 · Condensed 700 / 36</div>
          </div>
          <div><span className="t-h2">Session storage, memory, skills</span><div className="t-micro fg-faint">t-h2 · Inter 600 / 22</div></div>
          <div><span className="t-h3">Active phase · 22.2</span><div className="t-micro fg-faint">t-h3 · Inter 600 / 16</div></div>
          <div><span className="t-body">Body copy. The quick brown fox jumps over the lazy dog. 1234567890</span><div className="t-micro fg-faint">t-body · Inter 400 / 14</div></div>
          <div><span className="t-small">Small — metadata, captions, hints in dense UI.</span><div className="t-micro fg-faint">t-small · Inter 400 / 12</div></div>
          <div><span className="t-micro">t-micro — ~/.ironhermes/MEMORY.md · 67%</span><div className="t-micro fg-faint">Iosekey Mono 400 / 11</div></div>
          <div><span className="t-label">label · section header</span><div className="t-micro fg-faint">Iosekey Mono 600 / 11 uppercase 0.08em</div></div>
          <div className="mono" style={{fontSize:"var(--fs-13)"}}>code · `cargo build --release` → target/release/ironhermes</div>
        </div>
      </Section>

      {/* Color */}
      <Section title="Color" meta="All oklch — consistent chroma across themes">
        <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>surfaces</div>
        <div style={{display:"grid", gridTemplateColumns:"repeat(5, 1fr)", gap:"var(--sp-3)", marginBottom:"var(--sp-6)"}}>
          {["bg-canvas","bg-surface","bg-elevated","bg-raised","bg-input"].map(n => <Swatch key={n} name={n}/>)}
        </div>
        <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>text</div>
        <div style={{display:"grid", gridTemplateColumns:"repeat(5, 1fr)", gap:"var(--sp-3)", marginBottom:"var(--sp-6)"}}>
          {["fg","fg-muted","fg-dim","fg-faint","fg-on-accent"].map(n => <Swatch key={n} name={n} text/>)}
        </div>
        <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>accent + semantic</div>
        <div style={{display:"grid", gridTemplateColumns:"repeat(5, 1fr)", gap:"var(--sp-3)", marginBottom:"var(--sp-6)"}}>
          {["accent","success","warning","danger","info"].map(n => <Swatch key={n} name={n}/>)}
        </div>
        <div className="t-label" style={{marginBottom:"var(--sp-3)"}}>borders</div>
        <div style={{display:"grid", gridTemplateColumns:"repeat(3, 1fr)", gap:"var(--sp-3)"}}>
          {["border-subtle","border","border-strong"].map(n => <Swatch key={n} name={n} border/>)}
        </div>
      </Section>

      {/* Buttons */}
      <Section title="Buttons">
        <div className="row gap-3" style={{flexWrap:"wrap", marginBottom:"var(--sp-4)"}}>
          <Btn variant="primary">Send message</Btn>
          <Btn>Cancel</Btn>
          <Btn variant="ghost">Ghost</Btn>
          <Btn variant="danger" icon={<I.Trash size={12}/>}>Delete</Btn>
          <Btn variant="primary" icon={<I.Sparkle size={12}/>} kbd="⌘⏎">Run agent</Btn>
          <Btn variant="ghost" square icon={<I.More size={14}/>}/>
        </div>
        <div className="row gap-3" style={{flexWrap:"wrap", marginBottom:"var(--sp-4)"}}>
          <Btn size="sm">Small</Btn>
          <Btn size="sm" variant="primary">Small primary</Btn>
          <Btn size="lg">Large</Btn>
          <Btn size="lg" variant="primary" icon={<I.Play size={14}/>}>Deploy</Btn>
        </div>
        <div className="ih-btn-group">
          <Btn size="sm">all</Btn>
          <Btn size="sm">ok</Btn>
          <Btn size="sm">err</Btn>
          <Btn size="sm">pending</Btn>
        </div>
      </Section>

      {/* Inputs */}
      <Section title="Inputs">
        <div className="col gap-4" style={{maxWidth: 420}}>
          <Input placeholder="Session title"/>
          <Input prefix={<I.Search size={12}/>} placeholder="Search sessions…" suffix={<Kbd>⌘K</Kbd>}/>
          <Input prefix="$" placeholder="cargo build" code/>
          <div className="row gap-3">
            <Segmented options={[{value:"a",label:"dark"},{value:"b",label:"light"},{value:"c",label:"auto"}]} value="a"/>
            <Toggle checked={true}/>
            <Toggle checked={false}/>
          </div>
          <div className="row gap-4">
            <span className="ih-check" aria-checked="true"/><span className="t-small">Enable prompt cache</span>
            <span className="ih-radio" aria-checked="true"/><span className="t-small">cache_control</span>
            <span className="ih-radio" aria-checked="false"/><span className="t-small">none</span>
          </div>
        </div>
      </Section>

      {/* Badges */}
      <Section title="Badges & status">
        <div className="row gap-3" style={{flexWrap:"wrap", marginBottom:"var(--sp-4)"}}>
          <Badge>default</Badge>
          <Badge tone="accent">accent</Badge>
          <Badge tone="success" dot>healthy</Badge>
          <Badge tone="warning" dot>quota 81%</Badge>
          <Badge tone="danger" dot>error</Badge>
          <Badge tone="info">info</Badge>
          <Badge tone="solid">solid</Badge>
          <Badge square>square · kebab-case</Badge>
        </div>
        <div className="row gap-5">
          <Status kind="live">agent running</Status>
          <Status kind="warn">context 72%</Status>
          <Status kind="err">gateway offline</Status>
          <Status>idle</Status>
        </div>
      </Section>

      {/* Progress */}
      <Section title="Progress + capacity">
        <div style={{maxWidth: 420}} className="stack">
          <Progress value={67} tone="warning" label="MEMORY.md" suffix="1,474 / 2,200"/>
          <Progress value={42} label="USER.md" suffix="580 / 1,375"/>
          <Progress value={91} tone="danger" label="context" suffix="91%"/>
        </div>
      </Section>

      {/* Command block + tool call */}
      <Section title="Command blocks">
        <div className="stack">
          <CmdBlock cwd="~/ironhermes" cmd="cargo test --workspace" state="ok" duration="4.82s">
            <div className="fg-muted">running 412 tests</div>
            <div className="fg-success">test result: ok. 412 passed; 0 failed; 0 ignored</div>
          </CmdBlock>
          <CmdBlock cwd="~" cmd="rm -rf /" state="err" duration="0.01s">
            <div className="fg-danger">rm: cannot remove '/': Permission denied</div>
          </CmdBlock>
          <CmdBlock cwd="~/ironhermes" cmd="cargo watch -x check" state="run">
            <div className="fg-muted">watching ./src for changes…<span className="ih-caret"/></div>
          </CmdBlock>
        </div>
      </Section>

      <Section title="Tool calls">
        <div className="stack" style={{maxWidth: 560}}>
          <ToolCall name="read_file" status="ok" duration="4ms"
            args={<span><K c="path"/>: <S c=".planning/PROJECT.md"/>, <K c="limit"/>: <N c="500"/></span>}/>
          <ToolCall name="terminal" status="run"
            args={<span><K c="cmd"/>: <S c="cargo clippy --workspace"/></span>}>
            <pre style={{margin:0, fontSize:"var(--fs-11)", color:"var(--fg-muted)"}}>    Checking ironhermes-core<span className="ih-caret"/></pre>
          </ToolCall>
          <ToolCall name="memory.add" status="err" duration="12ms"
            args={<span><K c="text"/>: <S c="…"/>, <K c="reason"/>: <S c="capacity exceeded"/></span>}>
            <div style={{color:"var(--danger)", fontSize:"var(--fs-12)"}}>would exceed 2,200 char cap — entry rejected</div>
          </ToolCall>
        </div>
      </Section>

      {/* Chat messages */}
      <Section title="Chat messages">
        <div style={{maxWidth: 640}}>
          <Msg who="user"><p>what did phase 22.2 miss?</p></Msg>
          <Msg who="agent">
            <p>Session lineage on the ACP side. Three items worth lifting:</p>
            <ul style={{margin:0, paddingLeft:"var(--sp-6)"}}>
              <li>session:fork hook emission</li>
              <li>FTS5 snippets across forked sessions</li>
              <li>TUI fork-parent indicator</li>
            </ul>
          </Msg>
        </div>
      </Section>

      {/* Menu */}
      <Section title="Menu / popover">
        <div className="ih-menu" style={{maxWidth: 260}}>
          <div className="group-label">session</div>
          <div className="item"><I.Branch size={12} style={{color:"var(--fg-dim)"}}/>Fork session<span className="kbd">⌘B</span></div>
          <div className="item"><I.Copy size={12} style={{color:"var(--fg-dim)"}}/>Export transcript<span className="kbd">⌘E</span></div>
          <div className="item"><I.Search size={12} style={{color:"var(--fg-dim)"}}/>Search all sessions<span className="kbd">⌘K</span></div>
          <div className="sep"/>
          <div className="group-label">danger</div>
          <div className="item" style={{color:"var(--danger)"}}><I.Trash size={12}/>Delete session</div>
        </div>
      </Section>

      {/* Keyboard */}
      <Section title="Keyboard">
        <div className="row gap-3">
          <span className="row gap-1"><Kbd>⌘</Kbd><Kbd>K</Kbd><span className="t-small fg-muted">command palette</span></span>
          <span className="row gap-1"><Kbd>⌘</Kbd><Kbd>⏎</Kbd><span className="t-small fg-muted">send</span></span>
          <span className="row gap-1"><Kbd>⇧</Kbd><Kbd>⏎</Kbd><span className="t-small fg-muted">newline</span></span>
          <span className="row gap-1"><Kbd>⌃</Kbd><Kbd>C</Kbd><Kbd>⌃</Kbd><Kbd>C</Kbd><span className="t-small fg-muted">graceful stop</span></span>
        </div>
      </Section>
    </div>
  );
}

const Section = ({title, meta, children}) => (
  <section style={{marginBottom:"var(--sp-12)"}}>
    <div className="row gap-3" style={{marginBottom:"var(--sp-5)", paddingBottom:"var(--sp-3)", borderBottom:"1px solid var(--border-subtle)"}}>
      <span className="t-h3">{title}</span>
      {meta && <span className="t-micro fg-faint">{meta}</span>}
    </div>
    {children}
  </section>
);

const Swatch = ({name, text, border}) => {
  const color = `var(--${name})`;
  return (
    <div>
      <div style={{
        height: 52, borderRadius: "var(--r-3)",
        background: text ? "var(--bg-elevated)" : border ? "var(--bg-surface)" : color,
        color: text ? color : "transparent",
        border: border ? `2px solid ${color}` : "1px solid var(--border-subtle)",
        display:"flex", alignItems:"center", justifyContent:"center",
        fontFamily:"var(--font-mono-cond)", fontWeight: 700, fontSize:"var(--fs-18)",
      }}>{text ? "Aa" : border ? "" : ""}</div>
      <div className="mono t-micro fg-dim" style={{marginTop: 6}}>--{name}</div>
    </div>
  );
};

window.TokenGallery = TokenGallery;
window.Section = Section;
