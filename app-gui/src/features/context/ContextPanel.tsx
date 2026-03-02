import type { ContextSelection, InventoryItem, Recipe } from "../../shared/types/domain";

interface ContextPanelProps {
  selection: ContextSelection;
  items: InventoryItem[];
  recipes: Recipe[];
  thingsDirPath: string;
  onRunProof: (input: { methodName: string; args: string[]; cpuCost: string }) => void;
  proofRunning: boolean;
}

export function ContextPanel({
  selection,
  items,
  recipes,
  thingsDirPath,
  onRunProof,
  proofRunning,
}: ContextPanelProps) {
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
        {item.validity === "live" && (
          <button
            type="button"
            className="context-action"
            onClick={() =>
              onRunProof({
                methodName: "inspect",
                args: [item.name],
                cpuCost: "30s",
              })
            }
            disabled={proofRunning}
          >
            {proofRunning ? "Running..." : "Generate Proof"}
          </button>
        )}
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
      <button
        type="button"
        className="context-action"
        onClick={() =>
          onRunProof({
            methodName: recipe.verb,
            args: [...recipe.consumes.map((c) => c.label), ...recipe.requires.map((r) => r.label)],
            cpuCost: recipe.cpu,
          })
        }
        disabled={proofRunning}
      >
        {proofRunning ? "Running..." : "Generate Proof"}
      </button>
    </section>
  );
}
