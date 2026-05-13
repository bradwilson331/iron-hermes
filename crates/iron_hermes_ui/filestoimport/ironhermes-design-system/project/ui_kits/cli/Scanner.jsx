/* Scanner.jsx — knight-rider scanner matching knight_rider.rs */

const TRACK_WIDTH = 10;

function ScannerCells({ tick }) {
  const period = (TRACK_WIDTH - 1) * 2;
  const phase = tick % period;
  const lit = phase < TRACK_WIDTH ? phase : period - phase;
  const cells = [];
  for (let i = 0; i < TRACK_WIDTH; i++) {
    const d = Math.abs(i - lit);
    if (d === 0) cells.push(<span key={i} className="s-lit">█</span>);
    else if (d === 1) cells.push(<span key={i} className="s-t1">▓</span>);
    else if (d === 2) cells.push(<span key={i} className="s-t2">▒</span>);
    else cells.push(<span key={i} className="s-bg">░</span>);
  }
  return <span className="cells">{cells}</span>;
}

function Scanner({ activity }) {
  const [tick, setTick] = React.useState(0);
  React.useEffect(() => {
    if (activity === 'idle') return;
    const id = setInterval(() => setTick(t => t + 1), 100);
    return () => clearInterval(id);
  }, [activity]);

  if (activity === 'idle') return <div className="row-scanner" />;
  if (activity === 'streaming') {
    return (
      <div className="row-scanner">
        <ScannerCells tick={tick} /> <span className="dim">Streaming</span>
      </div>
    );
  }
  // tool call: activity === { kind: 'tool', name: '...' }
  return (
    <div className="row-scanner">
      <ScannerCells tick={tick} /> <span className="dim">Running:</span> <span className="yellow">{activity.name}</span>
    </div>
  );
}

window.Scanner = Scanner;
