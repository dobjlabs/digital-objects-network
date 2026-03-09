import { useMemo, useState } from "react";
import type { InventoryItem, Recipe } from "../../shared/types/domain";

interface RecipeGridProps {
  recipes: Recipe[];
  activeRecipeId: string | null;
  selectedItem: InventoryItem | null;
  onSelectRecipe: (recipeId: string) => void;
}

export function RecipeGrid({
  recipes,
  activeRecipeId,
  selectedItem,
  onSelectRecipe,
}: RecipeGridProps) {
  const [search, setSearch] = useState("");

  const unlocked = useMemo(
    () => recipes.filter((recipe) => recipe.unlocked),
    [recipes],
  );

  const compatibilityFiltered = useMemo(() => {
    if (!selectedItem) return unlocked;
    return unlocked.filter((recipe) =>
      recipe.args.some((arg) => arg.classHash === selectedItem.classMeta.hash),
    );
  }, [selectedItem, unlocked]);

  const visibleActions = useMemo(() => {
    if (selectedItem) return compatibilityFiltered;
    const q = search.trim().toLowerCase();
    if (!q) return compatibilityFiltered;
    return compatibilityFiltered.filter((recipe) => {
      return (
        recipe.name.toLowerCase().includes(q) ||
        recipe.group.toLowerCase().includes(q) ||
        recipe.verb.toLowerCase().includes(q)
      );
    });
  }, [compatibilityFiltered, search, selectedItem]);

  const grouped = useMemo(() => {
    const buckets = new Map<string, Recipe[]>();
    visibleActions.forEach((recipe) => {
      const list = buckets.get(recipe.group);
      if (list) {
        list.push(recipe);
      } else {
        buckets.set(recipe.group, [recipe]);
      }
    });
    return Array.from(buckets.entries());
  }, [visibleActions]);

  const filterLabel = selectedItem
    ? visibleActions.length > 0
      ? `accepts # ${selectedItem.classMeta.name}`
      : "no matching actions"
    : "";

  return (
    <section className="recipes-panel">
      <div className="action-tab-row">
        <button type="button" className="action-tab-btn active">
          Actions
        </button>
      </div>
      <div className="action-toolbar">
        {!selectedItem ? (
          <input
            className="action-search"
            placeholder="search actions..."
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        ) : (
          <div className="action-filter-label">{filterLabel}</div>
        )}
      </div>
      <div className="action-list">
        {grouped.map(([group, entries]) => (
          <div key={group}>
            <div className="action-group-label">{group}</div>
            {entries.map((recipe) => (
              <button
                key={recipe.id}
                type="button"
                className={`action-row ${activeRecipeId === recipe.id ? "active" : ""}`}
                onClick={() => onSelectRecipe(recipe.id)}
                title={`${recipe.name} (${recipe.verb})`}
              >
                <span className="action-row-emoji">{recipe.emoji}</span>
                <span className="action-row-name">{recipe.name}</span>
                <span className="action-row-hash">{recipe.hash}</span>
              </button>
            ))}
          </div>
        ))}
        {visibleActions.length === 0 && (
          <div className="action-empty">No actions match.</div>
        )}
      </div>
    </section>
  );
}
