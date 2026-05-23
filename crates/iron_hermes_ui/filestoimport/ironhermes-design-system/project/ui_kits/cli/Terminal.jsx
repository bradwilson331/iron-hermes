/* Terminal.jsx — composed CLI app: titlebar + transcript + bottom bar */

function Titlebar({ cwd }) {
  return (
    <div className="titlebar">
      <div className="dots"><div className="dot"/><div className="dot"/><div className="dot"/></div>
      <span className="dim">{cwd}</span>
      <div className="spacer" />
      <span className="cyan" style={{ fontWeight: 700 }}>IronHermes</span>
    </div>
  );
}

function Terminal({ state, setState, shelf }) {
  const [input, setInput] = React.useState('');
  const {
    lines, activity, tokens, limit, model, provider, mode, interrupts,
  } = state;

  function push(...toAdd) {
    setState(s => ({ ...s, lines: [...s.lines, ...toAdd] }));
  }

  function simulateTurn(userText) {
    push({ kind: 'you', text: userText });
    setState(s => ({ ...s, activity: 'streaming', tokens: s.tokens + 40 }));
    // stream tokens into a hermes line
    const reply = pickReply(userText);
    let i = 0;
    const placeholder = { kind: 'hermes', text: '', pending: true };
    setState(s => ({ ...s, lines: [...s.lines, placeholder] }));
    const id = setInterval(() => {
      i += 1;
      setState(s => {
        const next = [...s.lines];
        const last = { ...next[next.length - 1] };
        last.text = reply.slice(0, i * 2);
        last.pending = i * 2 < reply.length;
        next[next.length - 1] = last;
        return { ...s, lines: next, tokens: s.tokens + 3 };
      });
      if (i * 2 >= reply.length) {
        clearInterval(id);
        setState(s => ({ ...s, activity: 'idle' }));
      }
    }, 28);
  }

  function runSlash(cmd) {
    const [head] = cmd.split(' ');
    if (head === '/quit') {
      push({ kind: 'dim', text: 'Goodbye!' });
      setState(s => ({ ...s, activity: 'idle' }));
      return;
    }
    if (head === '/clear') {
      setState(s => ({ ...s, lines: [{ kind: 'dim', text: 'Conversation cleared.' }] }));
      return;
    }
    if (head === '/status') {
      push(
        { kind: 'cmd-head', text: 'IronHermes Status' },
        { kind: 'cmd-kv', key: 'Home:',     value: '~/.ironhermes/' },
        { kind: 'cmd-kv', key: 'Model:',    value: state.model },
        { kind: 'cmd-kv', key: 'Provider:', value: state.provider },
        { kind: 'cmd-kv', key: 'Tokens:',   value: `${state.tokens} / ${state.limit}` },
        { kind: 'spacer' },
      );
      return;
    }
    if (head === '/doctor') {
      push(
        { kind: 'cmd-head', text: 'Doctor' },
        { kind: 'doctor', ok: true,  label: 'Home directory' },
        { kind: 'doctor', ok: true,  label: 'Config file' },
        { kind: 'doctor', ok: false, label: '.env file' },
        { kind: 'doctor', ok: true,  label: 'OpenRouter API key' },
        { kind: 'doctor', ok: false, label: 'State database' },
        { kind: 'spacer' },
      );
      return;
    }
    if (head === '/help') {
      push({ kind: 'cmd-head', text: 'Available commands' });
      SLASH_COMMANDS.forEach(s => push({ kind: 'cmd-kv', key: s.cmd, value: s.desc }));
      push({ kind: 'spacer' });
      return;
    }
    if (head === '/model') {
      const m = cmd.split(' ')[1] || 'claude-sonnet-4-20250514';
      setState(s => ({ ...s, model: m, lines: [...s.lines, { kind: 'dim', text: `Model set to ${m}` }] }));
      return;
    }
    if (head === '/personality') {
      const p = cmd.split(' ')[1] || 'default';
      push({ kind: 'dim', text: `Personality: ${p}` });
      return;
    }
    push({ kind: 'red', text: `Unknown command: ${head}` });
  }

  function handleSubmit(text) {
    setInput('');
    setState(s => ({ ...s, interrupts: 0 }));
    if (text.startsWith('/')) runSlash(text.trim());
    else simulateTurn(text);
  }

  function handleInterrupt() {
    setState(s => {
      const n = s.interrupts + 1;
      const lines = [...s.lines];
      if (s.activity !== 'idle') {
        lines.push({ kind: 'dim', text: '^C — turn cancelled' });
      } else if (n >= 3) {
        lines.push({ kind: 'red', text: '^C×3 — emergency exit' });
      } else {
        lines.push({ kind: 'dim', text: '^C — type /quit to exit' });
      }
      return { ...s, lines, interrupts: n, activity: 'idle' };
    });
  }

  // hint like render.rs
  const hint = state.activity === 'idle'
    ? 'ctrl+c quit · /help commands'
    : 'ctrl+c cancel';

  return (
    <div className="terminal">
      <Titlebar cwd="~/projects/ironhermes" />
      {shelf}
      <Transcript lines={lines} />
      <div className="bottom">
        <Prompt
          value={input}
          onChange={setInput}
          onSubmit={handleSubmit}
          onInterrupt={handleInterrupt}
          disabled={false}
        />
        <Scanner activity={
          state.activity === 'idle' ? 'idle'
          : state.activity === 'streaming' ? 'streaming'
          : { kind: 'tool', name: state.activity.name }
        } />
        <StatusLine
          mode={mode}
          model={model}
          provider={provider}
          tokens={tokens}
          limit={limit}
          hint={hint}
        />
      </div>
    </div>
  );
}

function pickReply(input) {
  const l = input.toLowerCase();
  if (l.includes('refactor')) return "I'll read the file first, then propose a patch that splits the loop into a pure helper.";
  if (l.includes('test'))     return "Let me scan the existing test layout before adding one — consistency matters more than coverage here.";
  if (l.includes('hello'))    return "Ready. What do you want to build?";
  if (l.includes('cat') || l.includes('nya')) return "Nya~ let's take a look at the code together, meow =^.^=";
  return "Got it. I'll inspect the repo structure and come back with a concrete plan.";
}

window.Terminal = Terminal;
