// ih-components.jsx — IronHermes-specific UI primitives
// Needs: icons.jsx (provides window.I), components.css, tokens.css

const { useState, useEffect, useRef, useMemo, Fragment } = React;

// ============================================================
// Basic primitives wrapping the CSS classes
// ============================================================
const Btn = ({ variant, size, square, kbd, children, icon, onClick, style, ...rest }) => {
  const cls = ["ih-btn", variant, size, square && "square"].filter(Boolean).join(" ");
  return (
    <button className={cls} onClick={onClick} style={style} {...rest}>
      {icon}
      {children && <span>{children}</span>}
      {kbd && <span className="kbd">{kbd}</span>}
    </button>
  );
};

const Badge = ({ tone, square, dot, children }) => (
  <span className={["ih-badge", tone, square && "square"].filter(Boolean).join(" ")}>
    {dot && <i className="dot" />}
    {children}
  </span>
);

const Kbd = ({ children }) => <span className="kbd">{children}</span>;

const Input = ({ prefix, suffix, code, placeholder, value, onChange, style }) => (
  <div className={"ih-input" + (code ? " code" : "")} style={style}>
    {prefix && <span className="prefix">{prefix}</span>}
    <input placeholder={placeholder} value={value} onChange={onChange}/>
    {suffix && <span className="suffix">{suffix}</span>}
  </div>
);

const Segmented = ({ options, value, onChange }) => (
  <div className="ih-segmented">
    {options.map((o) => (
      <button key={o.value} aria-pressed={o.value === value} onClick={() => onChange?.(o.value)}>{o.label}</button>
    ))}
  </div>
);

const Toggle = ({ checked, onChange }) => (
  <button className="ih-toggle" aria-checked={checked} onClick={() => onChange?.(!checked)} />
);

const Status = ({ kind = "live", children }) => (
  <span className={`ih-status ${kind}`}><i className="dot"/>{children}</span>
);

const Progress = ({ value = 0, tone, label, suffix }) => (
  <div>
    {label && (
      <div className="row" style={{justifyContent:"space-between", marginBottom: 6}}>
        <span className="t-micro fg-muted">{label}</span>
        <span className="t-micro fg-muted mono-num">{suffix}</span>
      </div>
    )}
    <div className={"ih-progress " + (tone || "")}><i style={{width: `${value}%`}}/></div>
  </div>
);

// ============================================================
// Logo + brand
// ============================================================
const Logo = ({ compact = false }) => (
  <span className="ih-logo">
    <span className="fg-accent"><I.Mark size={18}/></span>
    {!compact && <span className="name">Iron<span className="fe">Hermes</span></span>}
  </span>
);

// ============================================================
// Command block — a single input→output cell
// ============================================================
const CmdBlock = ({ cwd = "~", cmd, state = "ok", duration = "0.03s", children }) => (
  <div className="ih-cmd" data-state={state}>
    <div className="rail"/>
    <div className="content">
      <div className="prompt-row">
        <span className="glyph">›</span>
        <span className="fg-dim">{cwd}</span>
        <span className="cmd">{cmd}</span>
        <span className="meta">{state === "run" ? "running…" : duration}</span>
      </div>
      {children && <div className="stdout">{children}</div>}
    </div>
  </div>
);

// ============================================================
// Tool call block — appears inline in agent messages
// ============================================================
const ToolCall = ({ name, status = "ok", children, args, duration }) => {
  const iconMap = { ok: <I.CheckFilled style={{color:"var(--success)"}}/>,
                    err: <I.Warn style={{color:"var(--danger)"}}/>,
                    run: <span className="ih-status live"><i className="dot"/></span> };
  return (
    <div className="ih-tool">
      <div className="ih-tool-head">
        <I.Tool style={{color:"var(--fg-dim)"}}/>
        <span className="name">{name}</span>
        {duration && <span className="fg-faint" style={{fontSize:"var(--fs-11)"}}>{duration}</span>}
        <span className="status-ic">{iconMap[status]}</span>
      </div>
      {args && <div className="ih-tool-body"><div className="args">{args}</div></div>}
      {children && <div className="ih-tool-body" style={{borderTop:"1px solid var(--border-subtle)"}}>{children}</div>}
    </div>
  );
};

// ============================================================
// Chat message
// ============================================================
const Msg = ({ who = "agent", author, time, children }) => {
  const avatar = who === "user"
    ? <span className="ih-avatar user">BW</span>
    : <span className="ih-avatar agent"/>;
  return (
    <div className="ih-msg">
      {avatar}
      <div className="body">
        <div className="hdr">
          <span className="author">{author || (who === "user" ? "Brad" : "IronHermes")}</span>
          {who === "agent" && <Badge tone="accent">claude-3.7-sonnet</Badge>}
          <span className="t" style={{marginLeft:"auto"}}>{time || "just now"}</span>
        </div>
        {children}
      </div>
    </div>
  );
};

// ============================================================
// Syntax colored args (simple helper)
// ============================================================
const Args = ({ children }) => <span>{children}</span>;
const K = ({c}) => <span className="k">{c}</span>;
const S = ({c}) => <span className="s">"{c}"</span>;
const N = ({c}) => <span className="n">{c}</span>;

Object.assign(window, {
  Btn, Badge, Kbd, Input, Segmented, Toggle, Status, Progress,
  Logo, CmdBlock, ToolCall, Msg, K, S, N,
});
