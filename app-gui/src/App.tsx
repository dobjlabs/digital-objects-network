import { useEffect, useState } from "react";
import { ContextPanel } from "./features/context/ContextPanel";
import { FeedPanel } from "./features/feed/FeedPanel";
import { InventoryPanel } from "./features/inventory/InventoryPanel";
import { ProofRunnerPanel } from "./features/proof-runner/ProofRunnerPanel";
import { RecipeGrid } from "./features/recipes/RecipeGrid";
import { getThingsDir, openThingsDir } from "./shared/api/tauriClient";
import { mockFeed, mockItems, mockRecipes } from "./shared/data/mockData";
import { useUiStore } from "./shared/state/uiStore";
import "./App.css";

function App() {
  const [thingsDirPath, setThingsDirPath] = useState("loading...");
  const activeItemId = useUiStore((state) => state.activeItemId);
  const activeRecipeId = useUiStore((state) => state.activeRecipeId);
  const contextSelection = useUiStore((state) => state.contextSelection);
  const showNullifiedItems = useUiStore((state) => state.showNullifiedItems);
  const selectItem = useUiStore((state) => state.selectItem);
  const selectRecipe = useUiStore((state) => state.selectRecipe);
  const toggleNullified = useUiStore((state) => state.toggleNullified);
  const runProof = useUiStore((state) => state.runProof);
  const proofRunning = useUiStore(
    (state) =>
      state.proof.status === "generating" ||
      state.proof.status === "committing",
  );

  useEffect(() => {
    let cancelled = false;
    getThingsDir()
      .then((path) => {
        if (!cancelled) setThingsDirPath(path);
      })
      .catch(() => {
        if (!cancelled)
          setThingsDirPath("(failed to resolve things directory)");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleOpenThingsDir = async () => {
    if (!thingsDirPath || thingsDirPath.startsWith("(")) return;
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
          thingsDirPath={thingsDirPath}
          onRunProof={runProof}
          proofRunning={proofRunning}
        />
        <ProofRunnerPanel />
      </div>

      <div className="right-column">
        <RecipeGrid
          recipes={mockRecipes}
          activeRecipeId={activeRecipeId}
          onSelectRecipe={selectRecipe}
        />
        <FeedPanel posts={mockFeed} />
      </div>
    </main>
  );
}

export default App;
