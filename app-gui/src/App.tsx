import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";
import "./App.css";

const MAX_CPU_POINTS = 120;

function App() {
  const [isMining, setIsMining] = useState(false);
  const [isGraphCollapsed, setIsGraphCollapsed] = useState(false);
  const [cpuUsage, setCpuUsage] = useState(0);
  const [cpuHistory, setCpuHistory] = useState<number[]>([]);
  const [objects, setObjects] = useState<string[]>([]);
  const [mineStatus, setMineStatus] = useState("");
  const [openStatus, setOpenStatus] = useState("");

  const loadObjects = useCallback(async () => {
    try {
      const items = await invoke<string[]>("list_objects");
      setObjects(items);
    } catch {
      setObjects([]);
    }
  }, []);

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
  }, [loadObjects]);

  useEffect(() => {
    let isMounted = true;
    let unlisten: (() => void) | undefined;

    const subscribe = async () => {
      const stop = await listen("objects-changed", async () => {
        if (!isMounted) {
          return;
        }
        await loadObjects();
      });
      unlisten = stop;
    };

    void subscribe();

    return () => {
      isMounted = false;
      if (unlisten) {
        unlisten();
      }
    };
  }, [loadObjects]);

  useEffect(() => {
    let isMounted = true;
    let unlisten: (() => void) | undefined;

    const subscribe = async () => {
      const stop = await listen<string>("mining-log", (event) => {
        if (!isMounted) {
          return;
        }
        setMineStatus(event.payload);
      });
      unlisten = stop;
    };

    void subscribe();

    return () => {
      isMounted = false;
      if (unlisten) {
        unlisten();
      }
    };
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
    setMineStatus("Mining started...");
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

  const handleOpenObjectsFolder = async () => {
    try {
      await invoke("open_objects_folder");
      setOpenStatus("");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setOpenStatus(`Failed to open folder: ${message}`);
    }
  };

  return (
    <div className="app-shell">
      <aside className="objects-pane">
        <h2 className="objects-title">Your Objects</h2>
        <div className="objects-list" aria-live="polite">
          <button
            type="button"
            className="tree-row root-row"
            onClick={handleOpenObjectsFolder}
          >
            <span className="tree-icon" aria-hidden="true">
              <svg viewBox="0 0 24 24" role="img" focusable="false">
                <path d="M3 6.75A1.75 1.75 0 0 1 4.75 5h4.1c.46 0 .9.19 1.23.52l1.4 1.4c.14.14.34.23.54.23h7.23A1.75 1.75 0 0 1 21 8.9v8.35A1.75 1.75 0 0 1 19.25 19H4.75A1.75 1.75 0 0 1 3 17.25z" />
              </svg>
            </span>
            <span className="tree-label">objects/</span>
          </button>

          {objects.length === 0 ? (
            <p className="objects-empty">No objects yet.</p>
          ) : (
            <ul className="tree-list">
              {objects.map((objectName) => (
                <li key={objectName}>
                  <button
                    type="button"
                    className="tree-row file-row"
                    onClick={handleOpenObjectsFolder}
                  >
                    <span className="tree-icon file-icon" aria-hidden="true">
                      <svg viewBox="0 0 24 24" role="img" focusable="false">
                        <path d="M7 3.5A1.5 1.5 0 0 0 5.5 5v14A1.5 1.5 0 0 0 7 20.5h10A1.5 1.5 0 0 0 18.5 19V8.7a1.5 1.5 0 0 0-.44-1.06l-3.2-3.2A1.5 1.5 0 0 0 13.8 4H7z" />
                      </svg>
                    </span>
                    <span className="tree-label">{objectName}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {openStatus ? <p className="objects-status">{openStatus}</p> : null}
        </div>
      </aside>

      <main className={`workspace ${isGraphCollapsed ? "graph-collapsed" : ""}`}>
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

        <button
          type="button"
          className="panel-divider"
          aria-label={isGraphCollapsed ? "Show CPU graph" : "Hide CPU graph"}
          onClick={() => setIsGraphCollapsed((value) => !value)}
        >
          <span />
          <span />
          <span />
        </button>

        {!isGraphCollapsed ? (
          <section className="metrics-panel">
            <div className="console-card">
              <p className="cpu-label">App CPU Usage: {cpuUsage.toFixed(1)}%</p>
              <div className="console-box" aria-live="polite">
                <div className="cpu-chart-layout">
                  <div className="cpu-axis-y" aria-hidden="true">
                    <span>100%</span>
                    <span>0%</span>
                  </div>
                  <svg
                    className="cpu-chart"
                    viewBox="0 0 100 100"
                    preserveAspectRatio="none"
                  >
                    <line className="grid-line" x1="0" y1="25" x2="100" y2="25" />
                    <line className="grid-line" x1="0" y1="50" x2="100" y2="50" />
                    <line className="grid-line" x1="0" y1="75" x2="100" y2="75" />
                    <polyline className="cpu-line" points={chartPoints} />
                  </svg>
                </div>
              </div>
            </div>
          </section>
        ) : null}
      </main>
    </div>
  );
}

export default App;
