import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import "./App.css";

const MAX_CPU_POINTS = 120;

function App() {
  const [isMining, setIsMining] = useState(false);
  const [cpuUsage, setCpuUsage] = useState(0);
  const [cpuHistory, setCpuHistory] = useState<number[]>([]);
  const [objects, setObjects] = useState<string[]>([]);
  const [mineStatus, setMineStatus] = useState("Ready.");

  const loadObjects = async () => {
    try {
      const items = await invoke<string[]>("list_objects");
      setObjects(items);
    } catch {
      setObjects([]);
    }
  };

  useEffect(() => {
    let isMounted = true;

    const sampleCpu = async () => {
      try {
        const sample = await invoke<number>("sample_app_cpu");
        if (!isMounted) {
          return;
        }

        const bounded = Math.max(0, Math.min(sample, 100));
        setCpuUsage(bounded);
        setCpuHistory((prev) => [...prev, bounded].slice(-MAX_CPU_POINTS));
      } catch {
        if (!isMounted) {
          return;
        }
        setCpuUsage(0);
      }
    };

    sampleCpu();
    const intervalId = setInterval(sampleCpu, 500);

    return () => {
      isMounted = false;
      clearInterval(intervalId);
    };
  }, []);

  useEffect(() => {
    void loadObjects();
  }, []);

  const buttonLabel = useMemo(
    () => (isMining ? "Mining Copper..." : "Mine Copper"),
    [isMining],
  );
  const chartPoints = useMemo(() => {
    if (cpuHistory.length < 2) {
      return "";
    }

    return cpuHistory
      .map((value, index) => {
        const x = (index / (MAX_CPU_POINTS - 1)) * 100;
        const y = 100 - value;
        return `${x},${y}`;
      })
      .join(" ");
  }, [cpuHistory]);

  const handleMine = async () => {
    if (isMining) {
      return;
    }

    setIsMining(true);
    setMineStatus("Mining started. Running hash workload for 10 seconds...");
    try {
      const objectName = await invoke<string>("mine_copper");
      await loadObjects();
      setMineStatus(`Mining complete. Created ${objectName}.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setMineStatus(`Mining failed: ${message}`);
    } finally {
      setIsMining(false);
    }
  };

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
        <div className="objects-list" aria-live="polite">
          {objects.length === 0 ? (
            <p className="objects-empty">No objects yet.</p>
          ) : (
            <ul>
              {objects.map((objectName) => (
                <li key={objectName}>{objectName}</li>
              ))}
            </ul>
          )}
        </div>
      </aside>

      <main className="workspace">
        <section className="stage">
          <div className="stage-content">
            <button
              type="button"
              className="mine-button"
              onClick={handleMine}
              disabled={isMining}
            >
              <span className="icon-play" aria-hidden="true">
                <svg viewBox="0 0 24 24" role="img" focusable="false">
                  <circle cx="12" cy="12" r="10.5" />
                  <path d="M10 8.5l6.25 3.5L10 15.5z" />
                </svg>
              </span>
              {buttonLabel}
            </button>
            <p className="mine-status">{mineStatus}</p>
          </div>
        </section>

        <div className="panel-divider" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>

        <section className="metrics-panel">
          <div className="console-card">
            <p className="cpu-label">App CPU Usage: {cpuUsage.toFixed(1)}%</p>
            <div className="console-box" aria-live="polite">
              <svg className="cpu-chart" viewBox="0 0 100 100" preserveAspectRatio="none">
                <line className="grid-line" x1="0" y1="25" x2="100" y2="25" />
                <line className="grid-line" x1="0" y1="50" x2="100" y2="50" />
                <line className="grid-line" x1="0" y1="75" x2="100" y2="75" />
                <polyline className="cpu-line" points={chartPoints} />
              </svg>
              <div className="cpu-axis-labels">
                <span>100%</span>
                <span>0%</span>
              </div>
            </div>
          </div>
        </section>
      </main>
    </div>
  );
}

export default App;
