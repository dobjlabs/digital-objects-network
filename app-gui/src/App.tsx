import { useEffect, useState } from "react";
import { ContextPanel } from "./features/context/ContextPanel";
import { ActionGrid } from "./features/actions/ActionGrid";
import { InventoryPanel } from "./features/inventory/InventoryPanel";
import { ProofRunnerPanel } from "./features/proof-runner/ProofRunnerPanel";
import { SettingsModal } from "./features/settings/SettingsModal";
import {
  getGlobalStateRoot,
  getObjectsDir,
  listenOpenSettings,
  listenObjectsChanged,
  listenRunActionProgress,
  listenMcpActionStarted,
  openObjectsDir,
  sampleAppCpu,
} from "./shared/api/tauriClient";
import { useStore } from "./shared/state/store";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/layout.css";
import "./styles/shared.css";
import "./features/inventory/InventoryPanel.css";
import "./features/context/ContextPanel.css";
import "./features/proof-runner/ProofRunnerPanel.css";
import "./features/actions/ActionGrid.css";
import "./features/settings/SettingsModal.css";

function App() {
  const [objectsDirPath, setObjectsDirPath] = useState("~/.dobj/objects");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [initialHydrationPending, setInitialHydrationPending] = useState(true);
  const inventory = useStore((state) => state.inventory);
  const actions = useStore((state) => state.actions);
  const activeObjectId = useStore((state) => state.activeObjectId);
  const activeActionId = useStore((state) => state.activeActionId);
  const contextSelection = useStore((state) => state.contextSelection);
  const showNullifiedItems = useStore((state) => state.showNullifiedItems);
  const hydrateData = useStore((state) => state.hydrateData);
  const selectObject = useStore((state) => state.selectObject);
  const selectAction = useStore((state) => state.selectAction);
  const clearSelection = useStore((state) => state.clearSelection);
  const toggleNullified = useStore((state) => state.toggleNullified);
  const recordCpuSample = useStore((state) => state.recordCpuSample);
  const setGlobalStateRoot = useStore((state) => state.setGlobalStateRoot);
  const applyRunActionProgress = useStore(
    (state) => state.applyRunActionProgress,
  );
  const initProofPanel = useStore((state) => state.initProofPanel);
  const runProof = useStore((state) => state.runProof);
  const proofStatus = useStore((state) => state.proof.status);
  const proofRunning = useStore(
    (state) =>
      state.proof.status === "generating" ||
      state.proof.status === "committing" ||
      state.proof.status === "summary",
  );
  const selectedObject =
    inventory.find((object) => object.id === activeObjectId) ?? null;

  useEffect(() => {
    let cancelled = false;
    hydrateData().catch((error) => {
      console.error("Failed to load GUI inventory:", error);
    }).finally(() => {
      if (!cancelled) {
        setInitialHydrationPending(false);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [hydrateData]);

  useEffect(() => {
    let cancelled = false;
    getObjectsDir()
      .then((path) => {
        if (!cancelled) setObjectsDirPath(path);
      })
      .catch(() => {
        if (!cancelled) setObjectsDirPath("~/.dobj/objects");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    listenRunActionProgress((event) => {
      if (!cancelled) {
        applyRunActionProgress(event);
      }
    })
      .then((dispose) => {
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((error) => {
        console.error("Failed to subscribe to run-action progress:", error);
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [applyRunActionProgress]);

  // Listen for MCP-initiated actions so the proof panel shows progress
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    listenMcpActionStarted((event) => {
      if (!cancelled) {
        initProofPanel({
          actionId: event.actionId,
          cpuCost: event.cpuCost,
          args: ["(via MCP)"],
        });
      }
    })
      .then((dispose) => {
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((error) => {
        console.error("Failed to subscribe to mcp-action-started:", error);
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [initProofPanel]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    let refreshTimer: number | null = null;

    const scheduleRefresh = () => {
      if (cancelled) return;
      if (refreshTimer !== null) {
        window.clearTimeout(refreshTimer);
      }
      refreshTimer = window.setTimeout(() => {
        refreshTimer = null;
        if (cancelled) return;
        hydrateData().catch((error) => {
          if (!cancelled) {
            console.error("Failed to refresh GUI after objects change:", error);
          }
        });
      }, 120);
    };

    listenObjectsChanged(() => {
      scheduleRefresh();
    })
      .then((dispose) => {
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((error) => {
        console.error("Failed to subscribe to objects-changed:", error);
      });

    return () => {
      cancelled = true;
      if (refreshTimer !== null) {
        window.clearTimeout(refreshTimer);
      }
      if (unlisten) unlisten();
    };
  }, [hydrateData]);

  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        const sample = await sampleAppCpu();
        if (!cancelled) {
          recordCpuSample(sample.usagePct, sample.totalCpuSecs);
        }
      } catch (error) {
        if (!cancelled) {
          console.error("Failed to sample CPU usage:", error);
        }
      }
    };

    void poll();
    const interval = window.setInterval(() => {
      void poll();
    }, 1000);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [recordCpuSample]);

  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        const root = await getGlobalStateRoot();
        if (!cancelled) {
          setGlobalStateRoot(root);
        }
      } catch (error) {
        if (!cancelled) {
          console.error("Failed to fetch global state root:", error);
        }
      }
    };

    void poll();
    const interval = window.setInterval(() => {
      void poll();
    }, 4000);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [setGlobalStateRoot]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (settingsOpen) return;
      if (event.key === "Escape") {
        clearSelection();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [clearSelection, settingsOpen]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    listenOpenSettings(() => {
      if (!cancelled) {
        setSettingsOpen(true);
      }
    })
      .then((dispose) => {
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((error) => {
        console.error("Failed to subscribe to open-settings:", error);
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const handleOpenObjectsDir = async () => {
    try {
      const dir = await openObjectsDir();
      setObjectsDirPath(dir);
    } catch (error) {
      console.error("Failed to open objects directory:", error);
    }
  };

  return (
    <>
      <div className="app-frame">
        <main className="app-shell" aria-busy={initialHydrationPending}>
          <InventoryPanel
            inventory={inventory}
            objectsDirPath={objectsDirPath}
            activeObjectId={activeObjectId}
            showNullifiedItems={showNullifiedItems}
            onSelectObject={selectObject}
            onToggleNullified={toggleNullified}
            onOpenObjectsDir={handleOpenObjectsDir}
          />

          <div className="main-column">
            <ContextPanel
              selection={contextSelection}
              inventory={inventory}
              objectsDirPath={objectsDirPath}
              actions={actions}
              onRunProof={runProof}
              proofRunning={proofRunning}
              proofStatus={proofStatus}
              onClearSelection={clearSelection}
            />
            <ProofRunnerPanel />
          </div>

          <div className="right-column">
            <ActionGrid
              actions={actions}
              activeActionId={activeActionId}
              selectedObject={selectedObject}
              onSelectAction={selectAction}
              onClearSelection={clearSelection}
            />
          </div>
        </main>

        {initialHydrationPending && (
          <div
            className="app-loading-overlay"
            role="status"
            aria-live="polite"
            aria-label="Loading objects and actions"
          >
            <div className="app-loading-card">
              <span className="app-loading-spinner" aria-hidden="true" />
              <span className="app-loading-label">
                Loading objects and actions...
              </span>
            </div>
          </div>
        )}
      </div>

      <SettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
      />
    </>
  );
}

export default App;
