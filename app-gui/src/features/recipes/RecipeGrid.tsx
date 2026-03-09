import type { Recipe } from "../../shared/types/domain";

interface RecipeGridProps {
  recipes: Recipe[];
  activeRecipeId: string | null;
  onSelectRecipe: (recipeId: string) => void;
}

export function RecipeGrid({
  recipes,
  activeRecipeId,
  onSelectRecipe,
}: RecipeGridProps) {
  const unlocked = recipes.filter((recipe) => recipe.unlocked);

  return (
    <section className="recipes-panel">
      <div className="action-tab-row">
        <button type="button" className="action-tab-btn active">
          Actions
        </button>
      </div>
      <div className="action-list">
        <div className="action-group-label">Global Production Must Grow</div>
        {unlocked.map((recipe) => (
          <button
            key={recipe.id}
            type="button"
            className={`action-row ${activeRecipeId === recipe.id ? "active" : ""}`}
            onClick={() => onSelectRecipe(recipe.id)}
            title={`${recipe.className} (${recipe.verb})`}
          >
            <span className="action-row-emoji">{recipe.emoji}</span>
            <span className="action-row-name">{recipe.name}</span>
            <span className="action-row-hash">{recipe.verb}</span>
          </button>
        ))}
      </div>
    </section>
  );
}
