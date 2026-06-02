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
  listenRunActionProgress,
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
  const activeObjectContentHash = useStore(
    (state) => state.activeObjectContentHash,
  );
  const activeAction = useStore((state) => state.activeAction);
  const contextSelection = useStore((state) => state.contextSelection);
  const showNullifiedItems = useStore((state) => state.showNullifiedItems);
  const hydrateData = useStore((state) => state.hydrateData);
  const importObject = useStore((state) => state.importObject);
  const selectObject = useStore((state) => state.selectObject);
  const selectAction = useStore((state) => state.selectAction);
  const clearSelection = useStore((state) => state.clearSelection);
  const toggleNullified = useStore((state) => state.toggleNullified);
  const recordCpuSample = useStore((state) => state.recordCpuSample);
  const setGlobalStateRoot = useStore((state) => state.setGlobalStateRoot);
  const applyRunActionProgress = useStore(
    (state) => state.applyRunActionProgress,
  );
  const resetProofPanel = useStore((state) => state.resetProofPanel);
  const runProof = useStore((state) => state.runProof);
  const proofStatus = useStore((state) => state.proof.status);
  const proofRunning = useStore(
    (state) =>
      state.proof.status === "generating" ||
      state.proof.status === "committing" ||
      state.proof.status === "summary",
  );
  const selectedObject =
    inventory.find(
      (object) => object.contentHash === activeObjectContentHash,
    ) ?? null;

  useEffect(() => {
    let cancelled = false;
    hydrateData()
      .catch((error) => {
        console.error("Failed to load GUI inventory:", error);
      })
      .finally(() => {
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
    const resetTimers = new Set<number>();
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
            console.error("Failed to refresh GUI after action progress:", error);
          }
        });
      }, 120);
    };

    listenRunActionProgress((event) => {
      if (!cancelled) {
        applyRunActionProgress(event);
        if (
          (event.outputFiles?.length ?? 0) > 0 ||
          (event.phase === "commit" && event.status === "done")
        ) {
          scheduleRefresh();
        }
        if (event.phase === "commit" && event.status === "done") {
          const timer = window.setTimeout(() => {
            resetTimers.delete(timer);
            if (!cancelled) resetProofPanel(event.runId);
          }, 2800);
          resetTimers.add(timer);
        }
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
      if (refreshTimer !== null) window.clearTimeout(refreshTimer);
      for (const timer of resetTimers) window.clearTimeout(timer);
      if (unlisten) unlisten();
    };
  }, [applyRunActionProgress, hydrateData, resetProofPanel]);

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
            activeObjectContentHash={activeObjectContentHash}
            showNullifiedItems={showNullifiedItems}
            onSelectObject={selectObject}
            onToggleNullified={toggleNullified}
            onOpenObjectsDir={handleOpenObjectsDir}
            onImportObject={importObject}
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
              activeAction={activeAction}
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
