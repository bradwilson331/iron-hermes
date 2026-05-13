/* Transcript.jsx — chat transcript with You/Hermes/Tool prompts */

function Transcript({ lines }) {
  const ref = React.useRef(null);
  React.useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [lines]);
  return (
    <div className="transcript" ref={ref}>
      {lines.map((l, i) => {
        if (l.kind === 'spacer') return <div key={i} className="line spacer" />;
        if (l.kind === 'you') {
          return <p key={i} className="line"><span className="you">You:</span> {l.text}</p>;
        }
        if (l.kind === 'hermes') {
          return <p key={i} className="line"><span className="hermes">Hermes:</span> {l.text}{l.pending && <span className="blink">▋</span>}</p>;
        }
        if (l.kind === 'tool') {
          return (
            <p key={i} className="line tool">
              Tool: <span className="tn">{l.name}</span> {l.args ? <span className="dim">{l.args}</span> : null}
            </p>
          );
        }
        if (l.kind === 'cmd-head') {
          return (
            <React.Fragment key={i}>
              <p className="line cmd-head">{l.text}</p>
              <p className="line rule">{'─'.repeat(40)}</p>
            </React.Fragment>
          );
        }
        if (l.kind === 'cmd-kv') {
          return (
            <p key={i} className="line cmd-out">{'  '}{l.key.padEnd(10)}{' '}<span>{l.value}</span></p>
          );
        }
        if (l.kind === 'doctor') {
          const badge = l.ok
            ? <span className="ok">OK</span>
            : l.fail
            ? <span className="red">FAIL</span>
            : <span className="missing">MISSING</span>;
          return <p key={i} className="line cmd-out">{'  '}[{badge}] {l.label}</p>;
        }
        if (l.kind === 'dim') return <p key={i} className="line dim">{l.text}</p>;
        if (l.kind === 'red') return <p key={i} className="line red">{l.text}</p>;
        return <p key={i} className="line">{l.text}</p>;
      })}
    </div>
  );
}

window.Transcript = Transcript;
