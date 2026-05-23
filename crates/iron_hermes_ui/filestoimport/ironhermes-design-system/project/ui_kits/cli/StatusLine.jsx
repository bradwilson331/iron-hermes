/* StatusLine.jsx — positional pill bar with dot separators */

function formatTokens(n) {
  if (n >= 999500) return (n / 1_000_000).toFixed(1) + 'M';
  if (n >= 1000)   return (n / 1000).toFixed(1) + 'K';
  return String(n);
}

const PILL_CLASSES = ['cyan', 'magenta', 'green', 'yellow', 'dim'];

function StatusLine({ mode, model, provider, tokens, limit, hint }) {
  const pct = limit === 0 ? 0 : Math.round((tokens / limit) * 100);
  const cells = [
    mode,
    model,
    provider,
    `${formatTokens(tokens)}/${formatTokens(limit)} (${pct}%)`,
  ];
  return (
    <div className="row-status">
      {cells.map((c, i) => (
        <React.Fragment key={i}>
          {i > 0 && <span className="sep"> · </span>}
          <span className={PILL_CLASSES[i % PILL_CLASSES.length]}>{c}</span>
        </React.Fragment>
      ))}
      {hint && (
        <>
          <span className="sep"> · </span>
          <span className="dim">{hint}</span>
        </>
      )}
    </div>
  );
}

window.StatusLine = StatusLine;
