import type { ContextSelection, InventoryItem, Recipe } from "../../shared/types/domain";

interface ContextPanelProps {
  selection: ContextSelection;
  items: InventoryItem[];
  recipes: Recipe[];
  thingsDirPath: string;
}

export function ContextPanel({ selection, items, recipes, thingsDirPath }: ContextPanelProps) {
  if (selection.kind === "none") {
    return <section className="context-panel">Select an item or recipe.</section>;
  }

  if (selection.kind === "item") {
    const item = items.find((candidate) => candidate.id === selection.itemId);
    if (!item) return <section className="context-panel">Item not found.</section>;
    return (
      <section className="context-panel">
        <h2>
          {item.emoji} {item.name}
        </h2>
        <p>Type: {item.type}</p>
        <p>
          Validity: <strong>{item.validity}</strong>
        </p>
        <p>{item.validity === "live" ? item.stateRoot : item.nullifier}</p>
        <p className="path-line">
          {thingsDirPath}/{item.name}
        </p>
      </section>
    );
  }

  const recipe = recipes.find((candidate) => candidate.id === selection.recipeId);
  if (!recipe) return <section className="context-panel">Recipe not found.</section>;

  return (
    <section className="context-panel">
      <h2>
        {recipe.emoji} {recipe.name}
      </h2>
      <p>{recipe.desc}</p>
      <p>CPU Cost: {recipe.cpu}</p>
      <p>Method: {recipe.verb}</p>
    </section>
  );
}
