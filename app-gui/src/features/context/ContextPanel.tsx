import { useEffect, useState } from "react";
import type { DragEvent } from "react";
import type { ContextSelection, InventoryItem, Recipe } from "../../shared/types/domain";

interface ContextPanelProps {
  selection: ContextSelection;
  items: InventoryItem[];
  recipes: Recipe[];
  thingsDirPath: string;
  dragItemName: string | null;
  onConsumeDragItem: () => void;
  onRunProof: (input: { methodName: string; args: string[]; cpuCost: string }) => void;
  proofRunning: boolean;
}

export function ContextPanel({
  selection,
  items,
  recipes,
  thingsDirPath,
  dragItemName,
  onConsumeDragItem,
  onRunProof,
  proofRunning,
}: ContextPanelProps) {
  const [argBindings, setArgBindings] = useState<Record<string, string>>({});

  useEffect(() => {
    setArgBindings({});
  }, [selection]);

  const argKey = (methodName: string, arg: string, index: number) =>
    `${selection.kind}:${methodName}:${arg}:${index}`;

  const handleDropArg = (
    event: DragEvent<HTMLDivElement>,
    methodName: string,
    arg: string,
    index: number,
  ) => {
    event.preventDefault();
    const raw =
      event.dataTransfer.getData("application/x-zkcraft-item") ||
      event.dataTransfer.getData("text/plain") ||
      event.dataTransfer.getData("text");
    if (!raw && !dragItemName) return;

    let name = raw || dragItemName || "";
    try {
      const parsed = JSON.parse(raw) as { name?: string };
      if (parsed.name) name = parsed.name;
    } catch {
      // plain text payload fallback
    }

    const key = argKey(methodName, arg, index);
    setArgBindings((prev) => ({ ...prev, [key]: name || dragItemName || arg }));
    onConsumeDragItem();
  };

  const renderMethodCard = (config: {
    methodName: string;
    cpuCost: string;
    readsBlock: boolean;
    args: string[];
    onRun: (boundArgs: string[]) => void;
  }) => (
    (() => {
      const boundArgs = config.args.map(
        (arg, index) => argBindings[argKey(config.methodName, arg, index)] ?? "",
      );
      const filledCount = boundArgs.filter((value) => value.trim().length > 0).length;
      const allArgsBound = config.args.length === 0 || filledCount === config.args.length;

      return (
        <div className="method-card">
          {config.args.length > 0 && (
            <div className="method-args">
              {config.args.map((arg, index) => {
                const key = argKey(config.methodName, arg, index);
                const bound = argBindings[key];
                return (
                  <div key={`${arg}-${index}`} className="method-arg-line">
                    <span className="method-arg-label">{arg}</span>
                    <div
                      className={`method-arg-placeholder ${bound ? "filled" : "missing"}`}
                      onDrop={(event) => handleDropArg(event, config.methodName, arg, index)}
                      onDragEnter={(event) => event.preventDefault()}
                      onDragOver={(event) => event.preventDefault()}
                      title={bound ? "Bound from inventory" : "Drop from inventory"}
                    >
                      {bound ?? "drag .dobj file here"}
                    </div>
                    {bound && (
                      <button
                        type="button"
                        className="method-arg-clear"
                        onClick={() =>
                          setArgBindings((prev) => {
                            const next = { ...prev };
                            delete next[key];
                            return next;
                          })
                        }
                      >
                        clear
                      </button>
                    )}
                  </div>
                );
              })}
              <div className="method-bind-hint">
                Inputs bound: {filledCount}/{config.args.length}
              </div>
            </div>
          )}
          <div className="method-footer">
            <div className="method-meta">
              <div>CPU Time Cost: {config.cpuCost}</div>
              {config.readsBlock && <div>Reads Block Number</div>}
            </div>
            <button
              type="button"
              className="context-action"
              onClick={() => config.onRun(boundArgs)}
              disabled={proofRunning || !allArgsBound}
            >
              {proofRunning ? "Running..." : allArgsBound ? config.methodName : "Bind all inputs"}
            </button>
          </div>
        </div>
      );
    })()
  );

  const itemMethod = (item: InventoryItem) => {
    if (item.validity !== "live") return null;
    switch (item.type) {
      case "source":
        return { methodName: "extract", cpuCost: "5-15m", readsBlock: true, args: ["Pickaxe"] };
      case "tool":
        return { methodName: "use", cpuCost: "30s-2m", readsBlock: false, args: ["Asteroid"] };
      case "creature":
        return { methodName: "feed", cpuCost: "20-40s", readsBlock: true, args: ["Bread"] };
      case "coin":
        return { methodName: "send", cpuCost: "10s", readsBlock: false, args: ["0x...recipient"] };
      default:
        return { methodName: "inspect", cpuCost: "30s", readsBlock: false, args: [item.name] };
    }
  };

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
        {(() => {
          const method = itemMethod(item);
          if (!method) return null;
          return renderMethodCard({
            ...method,
            onRun: (boundArgs) =>
              onRunProof({
                ...method,
                args: boundArgs,
              }),
          });
        })()}
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
      {renderMethodCard({
        methodName: recipe.verb,
        cpuCost: recipe.cpu,
        readsBlock: recipe.readsBlock,
        args: [...recipe.consumes.map((c) => c.label), ...recipe.requires.map((r) => r.label)],
        onRun: (boundArgs) =>
          onRunProof({
            methodName: recipe.verb,
            args: boundArgs,
            cpuCost: recipe.cpu,
          }),
      })}
    </section>
  );
}
