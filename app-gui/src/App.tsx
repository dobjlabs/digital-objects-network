import { useEffect, useState } from "react";
import { ContextPanel } from "./features/context/ContextPanel";
import { InventoryPanel } from "./features/inventory/InventoryPanel";
import { ProofRunnerPanel } from "./features/proof-runner/ProofRunnerPanel";
import { RecipeGrid } from "./features/recipes/RecipeGrid";
import {
  getThingsDir,
  listenCreateDobjProgress,
  openThingsDir,
  sampleAppCpu,
} from "./shared/api/tauriClient";
import { mockItems, mockRecipes } from "./shared/data/mockData";
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
  const activeItemId = useUiStore((state) => state.activeItemId);
  const activeRecipeId = useUiStore((state) => state.activeRecipeId);
  const contextSelection = useUiStore((state) => state.contextSelection);
  const showNullifiedItems = useUiStore((state) => state.showNullifiedItems);
  const selectItem = useUiStore((state) => state.selectItem);
  const selectRecipe = useUiStore((state) => state.selectRecipe);
  const toggleNullified = useUiStore((state) => state.toggleNullified);
  const recordCpuSample = useUiStore((state) => state.recordCpuSample);
  const applyCreateDobjProgress = useUiStore(
    (state) => state.applyCreateDobjProgress,
  );
  const runProof = useUiStore((state) => state.runProof);
  const proofRunning = useUiStore(
    (state) =>
      state.proof.status === "generating" ||
      state.proof.status === "committing" ||
      state.proof.status === "summary",
  );
  const selectedItem =
    mockItems.find((item) => item.id === activeItemId) ?? null;

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
    listenCreateDobjProgress((event) => {
      if (!cancelled) {
        applyCreateDobjProgress(event);
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
        console.error("Failed to subscribe to create_dobj progress:", error);
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [applyCreateDobjProgress]);

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
        items={mockItems}
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
          items={mockItems}
          recipes={mockRecipes}
          onRunProof={runProof}
          proofRunning={proofRunning}
        />
        <ProofRunnerPanel />
      </div>

      <div className="right-column">
        <RecipeGrid
          recipes={mockRecipes}
          activeRecipeId={activeRecipeId}
          selectedItem={selectedItem}
          onSelectRecipe={selectRecipe}
        />
      </div>
    </main>
  );
}

export default App;
