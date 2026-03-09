import { useMemo, useState } from "react";
import type { Recipe } from "../../shared/types/domain";

interface RecipeGridProps {
  recipes: Recipe[];
  activeRecipeId: string | null;
  onSelectRecipe: (recipeId: string) => void;
}

function actionHash(seed: string) {
  const bytes = new Uint8Array(8);
  for (let i = 0; i < seed.length; i += 1) {
    bytes[i % bytes.length] = (bytes[i % bytes.length] + seed.charCodeAt(i)) & 0xff;
  }
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
  return `0x${hex.slice(0, 4)}...${hex.slice(-4)}`;
}

export function RecipeGrid({
  recipes,
  activeRecipeId,
  onSelectRecipe,
}: RecipeGridProps) {
  const [search, setSearch] = useState("");
  const unlocked = recipes.filter((recipe) => recipe.unlocked);
  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return unlocked;
    return unlocked.filter((recipe) => {
      return (
        recipe.name.toLowerCase().includes(q) ||
        recipe.className.toLowerCase().includes(q) ||
        recipe.verb.toLowerCase().includes(q)
      );
    });
  }, [search, unlocked]);

  return (
    <section className="recipes-panel">
      <div className="action-tab-row">
        <button type="button" className="action-tab-btn active">
          Actions
        </button>
      </div>
      <div className="action-toolbar">
        <input
          className="action-search"
          placeholder="search actions..."
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
      </div>
      <div className="action-list">
        {filtered.map((recipe) => (
          <button
            key={recipe.id}
            type="button"
            className={`action-row ${activeRecipeId === recipe.id ? "active" : ""}`}
            onClick={() => onSelectRecipe(recipe.id)}
            title={`${recipe.name} (${recipe.verb})`}
          >
            <span className="action-row-emoji">{recipe.emoji}</span>
            <span className="action-row-name">{recipe.className}</span>
            <span className="action-row-hash">{actionHash(recipe.id)}</span>
          </button>
        ))}
        {filtered.length === 0 && (
          <div className="action-empty">No actions match.</div>
        )}
      </div>
    </section>
  );
}
