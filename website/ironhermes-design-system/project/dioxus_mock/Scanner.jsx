// Scanner — matches knight_rider.rs: 10 cells, 100ms ticks, triangle-wave sweep.
// Trail: d=0 █ bright cyan, d=1 ▓ cyan, d=2 ▒ dim cyan, d>=3 ░ dim.
// Used on the iOS chat screen and (static snapshot) on the landing page.

const TRACK = 10;

function Scanner({ running = true, label = "Streaming", toolName }) {
  const [tick, setTick] = React.useState(5);
  React.useEffect(() => {
    if (!running) return;
    const id = setInterval(() => setTick(t => t + 1), 100);
    return () => clearInterval(id);
  }, [running]);

  const period = (TRACK - 1) * 2;
  const phase = tick % period;
  const lit = phase < TRACK ? phase : period - phase;
  const cells = [];
  for (let i = 0; i < TRACK; i++) {
    const d = Math.abs(i - lit);
    const ch = d === 0 ? "█" : d === 1 ? "▓" : d === 2 ? "▒" : "░";
    const cls = d === 0 ? "s-lit" : d === 1 ? "s-t1" : d === 2 ? "s-t2" : "s-bg";
    cells.push(<span key={i} className={cls}>{ch}</span>);
  }
  return (
    <div className="scanner-line">
      <span style={{letterSpacing: 0}}>{cells}</span>{" "}
      {toolName ? (
        <>
          <span className="s-bg">Running:</span>{" "}
          <span style={{color: "var(--warn)"}}>{toolName}</span>
        </>
      ) : (
        <span className="s-bg">{label}</span>
      )}
    </div>
  );
}

// Static snapshot for the landing page (no timer)
function StaticScanner({ lit = 5 }) {
  const cells = [];
  for (let i = 0; i < TRACK; i++) {
    const d = Math.abs(i - lit);
    const ch = d === 0 ? "█" : d === 1 ? "▓" : d === 2 ? "▒" : "░";
    const cls = d === 0 ? "s-lit" : d === 1 ? "s-t1" : d === 2 ? "s-t2" : "s-bg";
    cells.push(<span key={i} className={cls}>{ch}</span>);
  }
  return <span style={{letterSpacing: 0}}>{cells}</span>;
}

window.Scanner = Scanner;
window.StaticScanner = StaticScanner;
