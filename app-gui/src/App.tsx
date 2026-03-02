import { useMemo, useState } from "react";
import { ContextPanel } from "./features/context/ContextPanel";
import { InventoryPanel } from "./features/inventory/InventoryPanel";
import { RecipeGrid } from "./features/recipes/RecipeGrid";
import { mockFeed, mockItems, mockRecipes } from "./shared/data/mockData";
import { initialUiState } from "./shared/state/initialState";
import type { AppUiState } from "./shared/types/domain";
import "./App.css";

function App() {
  const [uiState, setUiState] = useState<AppUiState>(initialUiState);

  const activePostCount = useMemo(() => mockFeed.length, []);

  const handleSelectItem = (itemId: string) => {
    setUiState((prev) => ({
      ...prev,
      activeItemId: itemId,
      activeRecipeId: null,
      contextSelection: { kind: "item", itemId },
    }));
  };

  const handleSelectRecipe = (recipeId: string) => {
    setUiState((prev) => ({
      ...prev,
      activeItemId: null,
      activeRecipeId: recipeId,
      contextSelection: { kind: "recipe", recipeId },
    }));
  };

  const handleToggleNullified = () => {
    setUiState((prev) => ({
      ...prev,
      showNullifiedItems: !prev.showNullifiedItems,
    }));
  };

  return (
    <main className="app-shell">
      <InventoryPanel
        items={mockItems}
        activeItemId={uiState.activeItemId}
        showNullifiedItems={uiState.showNullifiedItems}
        onSelectItem={handleSelectItem}
        onToggleNullified={handleToggleNullified}
      />

      <div className="main-column">
        <ContextPanel selection={uiState.contextSelection} items={mockItems} recipes={mockRecipes} />
        <section className="cpu-panel">CPU / proof runner panel (next step)</section>
      </div>

      <div className="right-column">
        <RecipeGrid
          recipes={mockRecipes}
          activeRecipeId={uiState.activeRecipeId}
          onSelectRecipe={handleSelectRecipe}
        />
        <section className="feed-panel">Feed panel scaffold ({activePostCount} mock posts)</section>
      </div>
    </main>
  );
}

export default App;
