// icons.jsx — Simple line icons for IronHermes design system.
// All icons: 14px default, 1.5 stroke, currentColor.

const Icon = ({ d, size = 14, fill = false, viewBox = "0 0 16 16", style, children }) => (
  <svg width={size} height={size} viewBox={viewBox}
    style={{ flexShrink: 0, display: "inline-block", verticalAlign: "middle", ...style }}
    fill={fill ? "currentColor" : "none"}
    stroke={fill ? "none" : "currentColor"}
    strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round">
    {d ? <path d={d}/> : children}
  </svg>
);

const I = {
  // nav + system
  Chat:     (p) => <Icon {...p}><path d="M2.5 4.5a2 2 0 012-2h7a2 2 0 012 2v5a2 2 0 01-2 2H7l-3 2.5V11.5H4.5a2 2 0 01-2-2v-5z"/></Icon>,
  Session:  (p) => <Icon {...p}><path d="M2.5 3.5h11M2.5 8h11M2.5 12.5h7"/></Icon>,
  Search:   (p) => <Icon {...p}><circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5l3 3"/></Icon>,
  Memory:   (p) => <Icon {...p}><rect x="3" y="3" width="10" height="10" rx="1.5"/><path d="M3 6.5h10M6.5 3v10"/></Icon>,
  Skill:    (p) => <Icon {...p}><path d="M8 2l1.8 3.8L14 6.5l-3 2.9.8 4.1L8 11.6 4.2 13.5 5 9.4l-3-2.9 4.2-.7L8 2z"/></Icon>,
  Cron:     (p) => <Icon {...p}><circle cx="8" cy="8.5" r="5"/><path d="M8 6v2.5l1.8 1"/><path d="M6 2h4"/></Icon>,
  Hook:     (p) => <Icon {...p}><path d="M5 3v5.5a3 3 0 006 0V6.5"/><circle cx="11" cy="4.5" r="1.5"/></Icon>,
  Subagent: (p) => <Icon {...p}><circle cx="8" cy="4" r="2"/><circle cx="3.5" cy="12" r="2"/><circle cx="12.5" cy="12" r="2"/><path d="M8 6v2.5M8 8.5L4.5 10.5M8 8.5l3.5 2"/></Icon>,
  Tool:     (p) => <Icon {...p}><path d="M10.5 2.5L13.5 5.5 10 9l-3-3 3.5-3.5zM7 6L3 10v3h3l4-4"/></Icon>,
  Terminal: (p) => <Icon {...p}><rect x="2" y="3" width="12" height="10" rx="1.5"/><path d="M5 7l2 1.5-2 1.5M8.5 10.5h3"/></Icon>,
  Provider: (p) => <Icon {...p}><path d="M3 8a5 5 0 0110 0v3H3V8z"/><path d="M5.5 11v2M10.5 11v2"/></Icon>,
  Soul:     (p) => <Icon {...p}><path d="M8 13.5c-3-2-5-4-5-7a3 3 0 015-2.2 3 3 0 015 2.2c0 3-2 5-5 7z"/></Icon>,
  Gear:     (p) => <Icon {...p}><circle cx="8" cy="8" r="2"/><path d="M8 1.5v2M8 12.5v2M14.5 8h-2M3.5 8h-2M12.6 3.4l-1.4 1.4M4.8 11.2L3.4 12.6M12.6 12.6l-1.4-1.4M4.8 4.8L3.4 3.4"/></Icon>,

  // action
  Plus:     (p) => <Icon {...p}><path d="M8 3v10M3 8h10"/></Icon>,
  Close:    (p) => <Icon {...p}><path d="M4 4l8 8M12 4l-8 8"/></Icon>,
  Check:    (p) => <Icon {...p}><path d="M3 8.5L6.5 12 13 4.5"/></Icon>,
  CheckFilled: (p) => <Icon {...p} fill><path d="M8 1a7 7 0 100 14A7 7 0 008 1zm3.5 5.5l-4.5 4.5-2.5-2.5"/></Icon>,
  Chevron:  (p) => <Icon {...p}><path d="M5.5 3.5L10 8l-4.5 4.5"/></Icon>,
  ChevronDown: (p) => <Icon {...p}><path d="M3.5 5.5L8 10l4.5-4.5"/></Icon>,
  More:     (p) => <Icon {...p} fill><circle cx="3.5" cy="8" r="1.3"/><circle cx="8" cy="8" r="1.3"/><circle cx="12.5" cy="8" r="1.3"/></Icon>,
  Copy:     (p) => <Icon {...p}><rect x="5" y="5" width="8" height="8" rx="1.5"/><path d="M3 10V4a1 1 0 011-1h6"/></Icon>,
  Send:     (p) => <Icon {...p}><path d="M2.5 8l11-5-4 11-2-4.5-5-1.5z"/></Icon>,
  Play:     (p) => <Icon {...p} fill><path d="M4 3v10l9-5z"/></Icon>,
  Pause:    (p) => <Icon {...p} fill><rect x="4" y="3" width="3" height="10" rx="0.5"/><rect x="9" y="3" width="3" height="10" rx="0.5"/></Icon>,
  Stop:     (p) => <Icon {...p} fill><rect x="4" y="4" width="8" height="8" rx="1"/></Icon>,
  Refresh:  (p) => <Icon {...p}><path d="M13 8a5 5 0 11-1.5-3.5L13 6M13 3v3h-3"/></Icon>,
  Edit:     (p) => <Icon {...p}><path d="M11 2.5l2.5 2.5L6 12.5 3 13l.5-3L11 2.5z"/></Icon>,
  Trash:    (p) => <Icon {...p}><path d="M3 4.5h10M5.5 4.5V3a1 1 0 011-1h3a1 1 0 011 1v1.5M6 7.5v4M10 7.5v4M4 4.5l.5 8a1 1 0 001 1h5a1 1 0 001-1l.5-8"/></Icon>,

  // status
  Dot:      (p) => <Icon {...p} fill><circle cx="8" cy="8" r="3"/></Icon>,
  Warn:     (p) => <Icon {...p}><path d="M8 2.5l6 10.5H2l6-10.5z"/><path d="M8 7v3M8 11.5v.5"/></Icon>,
  Info:     (p) => <Icon {...p}><circle cx="8" cy="8" r="5.5"/><path d="M8 7v3.5M8 5.5v.2"/></Icon>,
  Lock:     (p) => <Icon {...p}><rect x="3" y="7" width="10" height="6.5" rx="1"/><path d="M5 7V5a3 3 0 016 0v2"/></Icon>,
  Folder:   (p) => <Icon {...p}><path d="M2 4.5a1 1 0 011-1h3l1.5 1.5H13a1 1 0 011 1v6a1 1 0 01-1 1H3a1 1 0 01-1-1v-7.5z"/></Icon>,
  File:     (p) => <Icon {...p}><path d="M3.5 2.5h6L12.5 5.5v7.5a1 1 0 01-1 1h-8a1 1 0 01-1-1v-9.5a1 1 0 011-1zM9 2.5v3h3"/></Icon>,
  Code:     (p) => <Icon {...p}><path d="M6 4.5L2.5 8 6 11.5M10 4.5L13.5 8 10 11.5"/></Icon>,
  Command:  (p) => <Icon {...p}><rect x="3" y="3" width="4" height="4" rx="1.5"/><rect x="9" y="3" width="4" height="4" rx="1.5"/><rect x="3" y="9" width="4" height="4" rx="1.5"/><rect x="9" y="9" width="4" height="4" rx="1.5"/></Icon>,
  Sparkle:  (p) => <Icon {...p}><path d="M8 2.5L9 6.5 13 7.5 9 8.5 8 12.5 7 8.5 3 7.5 7 6.5 8 2.5z"/></Icon>,
  Thought:  (p) => <Icon {...p}><path d="M3 7a4 4 0 118 0 3 3 0 01-1 2.2V11l-2-1H7a4 4 0 01-4-3z"/><circle cx="5.5" cy="13" r="0.8"/></Icon>,
  Mic:      (p) => <Icon {...p}><rect x="6" y="2.5" width="4" height="7" rx="2"/><path d="M3.5 7.5a4.5 4.5 0 009 0M8 12v2"/></Icon>,
  Attach:   (p) => <Icon {...p}><path d="M12 7l-4.5 4.5a2.5 2.5 0 01-3.5-3.5L8.5 3.5a1.8 1.8 0 012.5 2.5L6.5 10.5a1 1 0 01-1.5-1.5L9 5"/></Icon>,
  Branch:   (p) => <Icon {...p}><circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="4.5" r="1.5"/><path d="M4.5 5v6M4.5 7.5a4 4 0 007-2.5"/></Icon>,

  // brand — minimal original mark: abstracted "IH" glyph; winged-square
  Mark: ({ size = 20, style }) => (
    <svg width={size} height={size} viewBox="0 0 20 20" style={style}>
      <rect x="3" y="3" width="14" height="14" rx="2.5" fill="none" stroke="currentColor" strokeWidth="1.4"/>
      <path d="M10 5.5v9M7 7.5v5M13 7.5v5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round"/>
      <path d="M2 10h2M16 10h2" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round"/>
    </svg>
  ),
  MarkSolid: ({ size = 20, style }) => (
    <svg width={size} height={size} viewBox="0 0 20 20" style={style}>
      <rect x="2" y="2" width="16" height="16" rx="3" fill="currentColor"/>
      <path d="M10 5.5v9M7 7.5v5M13 7.5v5" stroke="var(--accent-fg)" strokeWidth="1.6" strokeLinecap="round"/>
    </svg>
  ),
};

window.I = I;
window.Icon = Icon;
