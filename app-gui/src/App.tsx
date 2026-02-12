import { useEffect, useMemo, useState } from "react";
import "./App.css";

function App() {
  const [isMining, setIsMining] = useState(false);
  const [cpuSeconds, setCpuSeconds] = useState(0);
  const [logs, setLogs] = useState<string[]>([
    "[ready] Miner initialized.",
    "[ready] Waiting for command.",
  ]);

  useEffect(() => {
    if (!isMining) {
      return;
    }

    const tickInterval = setInterval(() => {
      setCpuSeconds((value) => value + 1);
    }, 1000);

    const logInterval = setInterval(() => {
      setLogs((prev) => {
        const timestamp = new Date().toLocaleTimeString();
        const next = [...prev, `[${timestamp}] mining copper shard...`];
        return next.slice(-10);
      });
    }, 2500);

    return () => {
      clearInterval(tickInterval);
      clearInterval(logInterval);
    };
  }, [isMining]);

  const buttonLabel = useMemo(
    () => (isMining ? "Mining Copper..." : "Mine Copper"),
    [isMining],
  );

  return (
    <div className="app-shell">
      <aside className="objects-pane">
        <h2 className="objects-title">
          <span className="icon-folder" aria-hidden="true">
            <svg viewBox="0 0 24 24" role="img" focusable="false">
              <path d="M3 6.75A1.75 1.75 0 0 1 4.75 5h4.1c.46 0 .9.19 1.23.52l1.4 1.4c.14.14.34.23.54.23h7.23A1.75 1.75 0 0 1 21 8.9v8.35A1.75 1.75 0 0 1 19.25 19H4.75A1.75 1.75 0 0 1 3 17.25z" />
            </svg>
          </span>
          Your Objects
        </h2>
      </aside>

      <main className="workspace">
        <section className="stage">
          <button
            type="button"
            className="mine-button"
            onClick={() => setIsMining((value) => !value)}
          >
            <span className="icon-play" aria-hidden="true">
              <svg viewBox="0 0 24 24" role="img" focusable="false">
                <circle cx="12" cy="12" r="10.5" />
                <path d="M10 8.5l6.25 3.5L10 15.5z" />
              </svg>
            </span>
            {buttonLabel}
          </button>
        </section>

        <div className="panel-divider" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>

        <section className="metrics-panel">
          <div className="console-card">
            <p className="cpu-label">CPU Time Spent: {cpuSeconds}s</p>
            <div className="console-box" aria-live="polite">
              {logs.map((entry, index) => (
                <p key={`${entry}-${index}`} className="console-line">
                  {entry}
                </p>
              ))}
            </div>
          </div>
        </section>
      </main>
    </div>
  );
}

export default App;
