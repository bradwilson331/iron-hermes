// app.jsx — main shell

const { useState: useAS, useEffect: useAE } = React;

const THEMES = [
  { id: "slate-dark",      label: "Slate · Dark",      swatches: ["#1a1f2a","#2a323f","#58a6ff"] },
  { id: "slate-light",     label: "Slate · Light",     swatches: ["#f8f8f9","#e8ebef","#0969da"] },
  { id: "iron-dark",       label: "Iron · Dark",       swatches: ["#121010","#2a201a","#ff8844"] },
  { id: "terminal-dark",   label: "Terminal · Dark",   swatches: ["#000000","#0f1410","#4cff88"] },
  { id: "parchment-light", label: "Parchment · Light", swatches: ["#f5ecd8","#e9d9b8","#8a5e2a"] },
];

const NAV = [
  { group: "overview", items: [
    { id: "overview",   label: "Overview",        icon: "Mark" },
    { id: "brand",      label: "Brand & logo",    icon: "Sparkle" },
  ]},
  { group: "foundation", items: [
    { id: "type",       label: "Typography",      icon: "Code" },
    { id: "color",      label: "Color",           icon: "Soul" },
    { id: "spacing",    label: "Spacing & radii", icon: "Command" },
    { id: "icons",      label: "Iconography",     icon: "Skill" },
  ]},
  { group: "components", items: [
    { id: "buttons",    label: "Buttons",         icon: "Play" },
    { id: "forms",      label: "Forms & inputs",  icon: "Edit" },
    { id: "badges",     label: "Badges & status", icon: "Dot" },
    { id: "cards",      label: "Cards & layout",  icon: "File" },
    { id: "terminal",   label: "Terminal blocks", icon: "Terminal" },
    { id: "toolcalls",  label: "Tool calls",      icon: "Tool" },
    { id: "chat",       label: "Chat messages",   icon: "Chat" },
  ]},
  { group: "screens", items: [
    { id: "chat-s",     label: "Chat",            icon: "Chat" },
    { id: "terminal-s", label: "Terminal",        icon: "Terminal" },
    { id: "memory-s",   label: "Memory",          icon: "Memory" },
    { id: "skills-s",   label: "Skills",          icon: "Skill" },
    { id: "cron-s",     label: "Cron",            icon: "Cron" },
    { id: "hooks-s",    label: "Hooks",           icon: "Hook" },
    { id: "agents-s",   label: "Subagents",       icon: "Subagent" },
    { id: "tool-s",     label: "Tool inspector",  icon: "Tool" },
    { id: "provider-s", label: "Provider",        icon: "Provider" },
    { id: "soul-s",     label: "SOUL.md",         icon: "Soul" },
    { id: "settings-s", label: "Settings",        icon: "Gear" },
  ]},
  { group: "platform", items: [
    { id: "macos",      label: "macOS window",    icon: "Command" },
    { id: "ios",        label: "iOS mobile",      icon: "Chat" },
    { id: "web",        label: "Web admin",       icon: "Provider" },
    { id: "tui",        label: "Pure TUI",        icon: "Terminal" },
  ]},
];

function DSApp() {
  const saved = (typeof localStorage !== "undefined" && localStorage.getItem("ih-ds-view")) || "overview";
  const savedTheme = (typeof localStorage !== "undefined" && localStorage.getItem("ih-ds-theme")) || "slate-dark";
  const [view, setView] = useAS(saved);
  const [theme, setTheme] = useAS(savedTheme);

  useAE(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("ih-ds-theme", theme);
  }, [theme]);
  useAE(() => { localStorage.setItem("ih-ds-view", view); }, [view]);

  const current = NAV.flatMap(g => g.items).find(i => i.id === view);
  const group = NAV.find(g => g.items.some(i => i.id === view))?.group || "overview";

  return (
    <div className="ds-shell">
      <nav className="ds-nav ih-scroll">
        <div className="brand">
          <span className="glyph">IH</span>
          <span className="wordmark">Iron<span className="fe">Hermes</span></span>
        </div>
        {NAV.map(g => (
          <div className="group" key={g.group}>
            <div className="group-label">{g.group}</div>
            {g.items.map(it => {
              const Ic = I[it.icon] || I.Mark;
              return (
                <button key={it.id}
                  className={"item" + (view === it.id ? " active" : "")}
                  onClick={() => setView(it.id)}>
                  <span className="ic"><Ic size={13}/></span>
                  <span>{it.label}</span>
                </button>
              );
            })}
          </div>
        ))}
      </nav>

      <div className="ds-main">
        <div className="ds-topbar">
          <div className="crumbs">
            <span>design-system</span>
            <span className="sep">/</span>
            <span>{group}</span>
            <span className="sep">/</span>
            <span className="here">{current?.label || view}</span>
          </div>
          <div style={{flex:1}}/>
          <Badge>v0.1.0</Badge>
          <Badge tone="accent" dot>tokens &amp; components</Badge>
        </div>

        <div className="ds-viewport ih-scroll">
          <Router view={view}/>
        </div>
      </div>

      <ThemeSwitch theme={theme} onChange={setTheme}/>
    </div>
  );
}

function ThemeSwitch({theme, onChange}) {
  return (
    <div className="ds-theme-switch">
      <h4>theme</h4>
      {THEMES.map(t => (
        <div key={t.id}
          className={"row" + (theme === t.id ? " active" : "")}
          onClick={() => onChange(t.id)}>
          <span className="swatches">
            {t.swatches.map((c,i) => <i key={i} style={{background: c}}/>)}
          </span>
          <span className="label">{t.label}</span>
          <span className="check"><I.Check size={12}/></span>
        </div>
      ))}
    </div>
  );
}

function Router({view}) {
  switch(view) {
    case "overview":   return <OverviewView/>;
    case "brand":      return <BrandView/>;
    case "type":       return <TypeView/>;
    case "color":      return <ColorView/>;
    case "spacing":    return <SpacingView/>;
    case "icons":      return <IconsView/>;
    case "buttons":    return <ButtonsView/>;
    case "forms":      return <FormsView/>;
    case "badges":     return <BadgesView/>;
    case "cards":      return <CardsView/>;
    case "terminal":   return <TerminalBlocksView/>;
    case "toolcalls":  return <ToolCallsView/>;
    case "chat":       return <ChatMsgView/>;
    case "chat-s":     return <FrameView><ChatScreen/></FrameView>;
    case "terminal-s": return <FrameView><TerminalScreen/></FrameView>;
    case "memory-s":   return <FrameView><MemoryScreen/></FrameView>;
    case "skills-s":   return <FrameView><SkillsScreen/></FrameView>;
    case "cron-s":     return <FrameView><CronScreen/></FrameView>;
    case "hooks-s":    return <FrameView><HooksScreen/></FrameView>;
    case "agents-s":   return <FrameView><SubagentScreen/></FrameView>;
    case "tool-s":     return <FrameView><ToolInspectorScreen/></FrameView>;
    case "provider-s": return <FrameView><ProviderScreen/></FrameView>;
    case "soul-s":     return <FrameView><SoulScreen/></FrameView>;
    case "settings-s": return <FrameView><SettingsScreen/></FrameView>;
    case "macos":      return <MacOSStage/>;
    case "ios":        return <IOSStage/>;
    case "web":        return <WebStage/>;
    case "tui":        return <TuiStage/>;
    default: return <OverviewView/>;
  }
}

function FrameView({children}) {
  return (
    <div style={{padding: "var(--sp-6)"}}>
      <div className="ds-frame" style={{height: "calc(100vh - 120px)", minHeight: 720}}>
        {children}
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(<DSApp/>);
