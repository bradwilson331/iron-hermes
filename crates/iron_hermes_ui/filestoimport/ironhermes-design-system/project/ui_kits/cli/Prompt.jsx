/* Prompt.jsx — bottom input row with slash-command suggestions */

const SLASH = [
  { cmd: '/quit',        desc: 'Exit the program' },
  { cmd: '/clear',       desc: 'Clear conversation history' },
  { cmd: '/status',      desc: 'Show current status' },
  { cmd: '/doctor',      desc: 'Run diagnostic checks' },
  { cmd: '/model',       desc: 'Switch model' },
  { cmd: '/personality', desc: 'Set personality preset' },
  { cmd: '/help',        desc: 'Show this help' },
];

function Prompt({ value, onChange, onSubmit, onInterrupt, disabled }) {
  const inputRef = React.useRef(null);
  const [sel, setSel] = React.useState(0);

  React.useEffect(() => { inputRef.current && inputRef.current.focus(); }, []);

  const showSuggest = value.startsWith('/') && !disabled;
  const matches = showSuggest
    ? SLASH.filter(s => s.cmd.startsWith(value.split(' ')[0]))
    : [];

  React.useEffect(() => { setSel(0); }, [value]);

  function handleKey(e) {
    if (e.ctrlKey && e.key === 'c') {
      e.preventDefault();
      onInterrupt();
      return;
    }
    if (matches.length > 0) {
      if (e.key === 'ArrowDown') { e.preventDefault(); setSel(s => Math.min(s + 1, matches.length - 1)); return; }
      if (e.key === 'ArrowUp')   { e.preventDefault(); setSel(s => Math.max(s - 1, 0)); return; }
      if (e.key === 'Tab')       { e.preventDefault(); onChange(matches[sel].cmd + ' '); return; }
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (!disabled && value.trim()) onSubmit(value);
    }
  }

  return (
    <>
      {matches.length > 0 && (
        <div className="suggestions">
          {matches.map((m, i) => (
            <div key={m.cmd} className={'row' + (i === sel ? ' active' : '')}>
              <span className="cmd">{m.cmd}</span>
              <span className="desc">{m.desc}</span>
            </div>
          ))}
        </div>
      )}
      <div className="row-prompt">
        <span className="prefix">You:</span>
        <input
          ref={inputRef}
          value={value}
          placeholder={disabled ? 'waiting…' : 'type a message or /command'}
          onChange={e => onChange(e.target.value)}
          onKeyDown={handleKey}
          disabled={disabled}
          spellCheck={false}
          autoComplete="off"
        />
      </div>
    </>
  );
}

window.Prompt = Prompt;
window.SLASH_COMMANDS = SLASH;
