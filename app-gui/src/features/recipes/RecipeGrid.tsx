import type { Recipe } from "../../shared/types/domain";

interface RecipeGridProps {
  recipes: Recipe[];
  activeRecipeId: string | null;
  onSelectRecipe: (recipeId: string) => void;
}

export function RecipeGrid({ recipes, activeRecipeId, onSelectRecipe }: RecipeGridProps) {
  return (
    <section className="recipes-panel">
      <header className="panel-header">Global Production Must Grow</header>
      <div className="recipe-grid">
        {recipes.filter((recipe) => recipe.unlocked).map((recipe) => (
          <button
            key={recipe.id}
            type="button"
            className={`recipe-cell ${activeRecipeId === recipe.id ? "active" : ""}`}
            onClick={() => onSelectRecipe(recipe.id)}
            title={recipe.name}
          >
            {recipe.emoji}
          </button>
        ))}
      </div>
    </section>
  );
}
