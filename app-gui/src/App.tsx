import { useEffect, useState } from "react";
import { ContextPanel } from "./features/context/ContextPanel";
import { InventoryPanel } from "./features/inventory/InventoryPanel";
import { ProofRunnerPanel } from "./features/proof-runner/ProofRunnerPanel";
import { RecipeGrid } from "./features/recipes/RecipeGrid";
import {
  getThingsDir,
  listenObjectsChanged,
  listenRunSdkActionProgress,
  openThingsDir,
  sampleAppCpu,
} from "./shared/api/tauriClient";
import { useUiStore } from "./shared/state/uiStore";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/layout.css";
import "./styles/shared.css";
import "./features/inventory/InventoryPanel.css";
import "./features/context/ContextPanel.css";
import "./features/proof-runner/ProofRunnerPanel.css";
import "./features/recipes/RecipeGrid.css";

function App() {
  const [thingsDirPath, setThingsDirPath] = useState("~/.objects");
  const items = useUiStore((state) => state.items);
  const recipes = useUiStore((state) => state.recipes);
  const activeItemId = useUiStore((state) => state.activeItemId);
  const activeRecipeId = useUiStore((state) => state.activeRecipeId);
  const contextSelection = useUiStore((state) => state.contextSelection);
  const showNullifiedItems = useUiStore((state) => state.showNullifiedItems);
  const hydrateData = useUiStore((state) => state.hydrateData);
  const selectItem = useUiStore((state) => state.selectItem);
  const selectRecipe = useUiStore((state) => state.selectRecipe);
  const clearSelection = useUiStore((state) => state.clearSelection);
  const toggleNullified = useUiStore((state) => state.toggleNullified);
  const recordCpuSample = useUiStore((state) => state.recordCpuSample);
  const applyRunSdkActionProgress = useUiStore(
    (state) => state.applyRunSdkActionProgress,
  );
  const runProof = useUiStore((state) => state.runProof);
  const proofStatus = useUiStore((state) => state.proof.status);
  const proofRunning = useUiStore(
    (state) =>
      state.proof.status === "generating" ||
      state.proof.status === "committing" ||
      state.proof.status === "summary",
  );
  const selectedItem =
    items.find((item) => item.id === activeItemId) ?? null;

  useEffect(() => {
    hydrateData().catch((error) => {
      console.error("Failed to load GUI bootstrap:", error);
    });
  }, [hydrateData]);

  useEffect(() => {
    let cancelled = false;
    getThingsDir()
      .then((path) => {
        if (!cancelled) setThingsDirPath(path);
      })
      .catch(() => {
        if (!cancelled) setThingsDirPath("~/.objects");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    listenRunSdkActionProgress((event) => {
      if (!cancelled) {
        applyRunSdkActionProgress(event);
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
        console.error("Failed to subscribe to run-sdk-action progress:", error);
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [applyRunSdkActionProgress]);

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
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        clearSelection();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [clearSelection]);

  const handleOpenThingsDir = async () => {
    try {
      const dir = await openThingsDir();
      setThingsDirPath(dir);
    } catch (error) {
      console.error("Failed to open things directory:", error);
    }
  };

  return (
    <main className="app-shell">
      <InventoryPanel
        items={items}
        thingsDirPath={thingsDirPath}
        activeItemId={activeItemId}
        showNullifiedItems={showNullifiedItems}
        onSelectItem={selectItem}
        onToggleNullified={toggleNullified}
        onOpenThingsDir={handleOpenThingsDir}
      />

      <div className="main-column">
        <ContextPanel
          selection={contextSelection}
          items={items}
          recipes={recipes}
          onRunProof={runProof}
          proofRunning={proofRunning}
          proofStatus={proofStatus}
          onClearSelection={clearSelection}
        />
        <ProofRunnerPanel />
      </div>

      <div className="right-column">
        <RecipeGrid
          recipes={recipes}
          activeRecipeId={activeRecipeId}
          selectedItem={selectedItem}
          onSelectRecipe={selectRecipe}
          onClearSelection={clearSelection}
        />
      </div>
    </main>
  );
}

export default App;
