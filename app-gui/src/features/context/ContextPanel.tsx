import { useEffect, useState } from "react";
import type { DragEvent } from "react";
import type {
  ContextSelection,
  InventoryItem,
  Recipe,
} from "../../shared/types/domain";

interface ContextPanelProps {
  selection: ContextSelection;
  items: InventoryItem[];
  recipes: Recipe[];
  thingsDirPath: string;
  onRunProof: (input: {
    methodName: string;
    args: string[];
    cpuCost: string;
  }) => void;
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
  const [argBindings, setArgBindings] = useState<Record<string, string>>({});
  const [hoverArgKey, setHoverArgKey] = useState<string | null>(null);
  const [argErrors, setArgErrors] = useState<Record<string, string>>({});
  const selectionKey =
    selection.kind === "item"
      ? `item:${selection.itemId}`
      : selection.kind === "recipe"
        ? `recipe:${selection.recipeId}`
        : "none";

  useEffect(() => {
    setArgBindings({});
    setArgErrors({});
  }, [selectionKey]);

  const argKey = (methodName: string, arg: string, index: number) =>
    `${selection.kind}:${methodName}:${arg}:${index}`;

  const normalizeThingName = (value: string) =>
    value
      .replace(/\.dobj$/i, "")
      .trim()
      .toLowerCase();

  const parseDropPayload = (
    raw: string,
  ): { itemId?: string; name?: string } => {
    try {
      const parsed = JSON.parse(raw) as { itemId?: string; name?: string };
      return parsed;
    } catch {
      return { name: raw };
    }
  };

  const isArgCompatible = (arg: string, droppedName: string) => {
    const expected = normalizeThingName(arg);
    const actual = normalizeThingName(droppedName);

    if (!expected || !actual) return false;
    if (expected.startsWith("0x")) return false;
    return expected === actual;
  };

  const isManualArg = (arg: string) => arg.trim().startsWith("0x");

  const handleDropArg = (
    event: DragEvent<HTMLDivElement>,
    methodName: string,
    arg: string,
    index: number,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    const raw =
      event.dataTransfer.getData("application/x-zkcraft-item") ||
      event.dataTransfer.getData("text/plain") ||
      event.dataTransfer.getData("text");
    if (!raw) return;

    const parsed = parseDropPayload(raw);
    const name = parsed.name ?? raw;

    const key = argKey(methodName, arg, index);
    if (!isArgCompatible(arg, name)) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${arg} but got ${name}`,
      }));
      return;
    }

    setArgBindings((prev) => ({ ...prev, [key]: name || arg }));
    setArgErrors((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
    setHoverArgKey(null);
  };

  const renderMethodCard = (config: {
    methodId: string;
    methodName: string;
    cpuCost: string;
    readsBlock: boolean;
    args: string[];
    onRun: (boundArgs: string[]) => void;
  }) =>
    (() => {
      const boundArgs = config.args.map(
        (arg, index) => argBindings[argKey(config.methodId, arg, index)] ?? "",
      );
      const filledCount = boundArgs.filter(
        (value) => value.trim().length > 0,
      ).length;
      const allArgsBound =
        config.args.length === 0 || filledCount === config.args.length;

      return (
        <div className="method-card">
          {config.args.length > 0 && (
            <div className="method-args">
              {config.args.map((arg, index) => {
                const key = argKey(config.methodId, arg, index);
                const bound = argBindings[key];
                const isDropActive = hoverArgKey === key;
                const err = argErrors[key];
                return (
                  <div
                    key={`${arg}-${index}`}
                    className={`method-arg-line ${isDropActive ? "drop-active" : ""}`}
                    onDragEnter={(event) => {
                      event.preventDefault();
                      setHoverArgKey(key);
                    }}
                    onDragLeave={() =>
                      setHoverArgKey((prev) => (prev === key ? null : prev))
                    }
                    onDragOver={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      event.dataTransfer.dropEffect = "copy";
                      if (hoverArgKey !== key) setHoverArgKey(key);
                    }}
                    onDrop={(event) =>
                      handleDropArg(event, config.methodId, arg, index)
                    }
                  >
                    <span className="method-arg-label">{arg}</span>
                    {isManualArg(arg) ? (
                      <>
                        <input
                          className={`method-arg-input ${err ? "error" : ""}`}
                          placeholder={arg}
                          value={bound ?? ""}
                          onChange={(event) => {
                            const value = event.target.value;
                            setArgBindings((prev) => ({
                              ...prev,
                              [key]: value,
                            }));
                            setArgErrors((prev) => {
                              const next = { ...prev };
                              delete next[key];
                              return next;
                            });
                          }}
                        />
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
                      </>
                    ) : (
                      <>
                        <div
                          className={`method-arg-placeholder ${bound ? "filled" : "missing"} ${err ? "error" : ""}`}
                          title={
                            bound
                              ? "Bound from inventory"
                              : "Drop from inventory"
                          }
                        >
                          {bound ??
                            (isDropActive
                              ? "release to drop"
                              : "drag .dobj file here")}
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
                      </>
                    )}
                    {err && <div className="method-arg-error">{err}</div>}
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
              {proofRunning
                ? "Running..."
                : allArgsBound
                  ? config.methodName
                  : "Bind all inputs"}
            </button>
          </div>
        </div>
      );
    })();

  const itemMethod = (item: InventoryItem) => {
    if (item.validity !== "live") return null;
    switch (item.type) {
      case "source":
        return [
          {
            methodName: "extract",
            cpuCost: "5-15m",
            readsBlock: true,
            args: ["Pickaxe"],
          },
          { methodName: "survey", cpuCost: "2-5m", readsBlock: true, args: [] },
        ];
      case "tool":
        return [
          {
            methodName: "repair",
            cpuCost: "1-3m",
            readsBlock: false,
            args: ["CopperIngot"],
          },
          {
            methodName: "use",
            cpuCost: "30s-2m",
            readsBlock: false,
            args: ["Asteroid"],
          },
        ];
      case "creature":
        return [
          {
            methodName: "feed",
            cpuCost: "20-40s",
            readsBlock: true,
            args: ["Bread"],
          },
          {
            methodName: "inspect",
            cpuCost: "10-20s",
            readsBlock: false,
            args: [],
          },
        ];
      case "coin":
        return [
          {
            methodName: "send",
            cpuCost: "10s",
            readsBlock: false,
            args: ["0x...recipient"],
          },
          {
            methodName: "bundle",
            cpuCost: "10-20s",
            readsBlock: false,
            args: ["Coin"],
          },
        ];
      default:
        return [
          {
            methodName: "inspect",
            cpuCost: "30s",
            readsBlock: false,
            args: [item.name],
          },
        ];
    }
  };

  const progressClass = (value: number, lowDanger = false) => {
    if (lowDanger) {
      if (value < 25) return "danger";
      if (value < 50) return "warn";
      return "good";
    }
    if (value > 80) return "danger";
    if (value > 50) return "warn";
    return "good";
  };

  const displayThingPath = (filename: string) => {
    const normalized = thingsDirPath.trim();
    if (!normalized) return filename;
    const thingsIndex = normalized.lastIndexOf("/.things");
    if (thingsIndex >= 0) {
      return `~${normalized.slice(thingsIndex)}/${filename}`;
    }
    return `${normalized}/${filename}`;
  };

  const renderItemStats = (item: InventoryItem) => {
    if (item.type === "source" && item.charge !== undefined) {
      return (
        <div className="item-stats">
          <div className="stat-row">
            <span className="stat-key">charge</span>
            <span className="stat-val good">{item.charge}%</span>
          </div>
          <div className="stat-progress">
            <div
              className="stat-progress-fill good"
              style={{ width: `${item.charge}%` }}
            />
          </div>
          <div className="stat-row">
            <span className="stat-key">recharge rate</span>
            <span className="stat-val good">{item.rechargeRate}</span>
          </div>
        </div>
      );
    }

    if (
      item.type === "tool" &&
      item.durability !== undefined &&
      item.maxDurability
    ) {
      const pct = Math.round((item.durability / item.maxDurability) * 100);
      const cls = progressClass(pct, true);
      return (
        <div className="item-stats">
          <div className="stat-row">
            <span className="stat-key">tier</span>
            <span className="stat-val good">{item.tier ?? 1}</span>
          </div>
          <div className="stat-row">
            <span className="stat-key">durability</span>
            <span className={`stat-val ${cls}`}>
              {item.durability}/{item.maxDurability}
            </span>
          </div>
          <div className="stat-progress">
            <div
              className={`stat-progress-fill ${cls}`}
              style={{ width: `${pct}%` }}
            />
          </div>
          <div className="stat-row">
            <span className="stat-key">skill</span>
            <span className="stat-val good">{item.skill ?? 0}</span>
          </div>
        </div>
      );
    }

    if (
      item.type === "creature" &&
      item.hunger !== undefined &&
      item.health !== undefined
    ) {
      const hungerCls = progressClass(item.hunger);
      const healthCls = progressClass(100 - item.health);
      return (
        <div className="item-stats">
          <div className="stat-row">
            <span className="stat-key">hunger</span>
            <span className={`stat-val ${hungerCls}`}>{item.hunger}%</span>
          </div>
          <div className="stat-progress">
            <div
              className={`stat-progress-fill ${hungerCls}`}
              style={{ width: `${item.hunger}%` }}
            />
          </div>
          <div className="stat-row">
            <span className="stat-key">health</span>
            <span className={`stat-val ${healthCls}`}>{item.health}%</span>
          </div>
          <div className="stat-row">
            <span className="stat-key">last fed</span>
            <span className="stat-val warn">{item.lastFed}</span>
          </div>
        </div>
      );
    }

    if (
      item.type === "vehicle" &&
      item.fuel !== undefined &&
      item.condition !== undefined
    ) {
      const fuelCls = progressClass(item.fuel, true);
      return (
        <div className="item-stats">
          <div className="stat-row">
            <span className="stat-key">fuel</span>
            <span className={`stat-val ${fuelCls}`}>{item.fuel}%</span>
          </div>
          <div className="stat-progress">
            <div
              className={`stat-progress-fill ${fuelCls}`}
              style={{ width: `${item.fuel}%` }}
            />
          </div>
          <div className="stat-row">
            <span className="stat-key">condition</span>
            <span className="stat-val good">{item.condition}%</span>
          </div>
        </div>
      );
    }

    if (item.type === "raw" && item.qty !== undefined) {
      return (
        <div className="item-stats">
          <div className="stat-row">
            <span className="stat-key">quantity</span>
            <span className="stat-val good">{item.qty}</span>
          </div>
          <div className="stat-row">
            <span className="stat-key">decay</span>
            <span className="stat-val danger">{item.decay}</span>
          </div>
        </div>
      );
    }

    if (item.type === "coin" && item.value !== undefined) {
      return (
        <div className="item-stats">
          <div className="stat-row">
            <span className="stat-key">value</span>
            <span className="stat-val good">{item.value}¢</span>
          </div>
        </div>
      );
    }

    return null;
  };

  if (selection.kind === "none") {
    return (
      <section className="context-panel">Select an item or recipe.</section>
    );
  }

  if (selection.kind === "item") {
    const item = items.find((candidate) => candidate.id === selection.itemId);
    if (!item)
      return <section className="context-panel">Item not found.</section>;
    return (
      <section className="context-panel">
        <div className="context-title-row">
          <h2>
            {item.emoji} {item.name}
          </h2>
        </div>
        <div
          className={`context-hash-line ${item.validity === "live" ? "live" : "nullified"}`}
        >
          {item.validity === "live"
            ? `${item.stateRoot} · ✓ live`
            : `${item.nullifier ?? "nullified"} · ✗ nullified`}
        </div>
        <div className="context-path-line">{displayThingPath(item.name)}</div>
        {renderItemStats(item)}
        {(() => {
          const methods = itemMethod(item);
          if (!methods) return null;
          return (
            <div className="method-list">
              {methods.map((method, index) =>
                renderMethodCard({
                  ...method,
                  methodId: `${item.id}:${method.methodName}:${index}`,
                  onRun: (boundArgs) =>
                    onRunProof({
                      ...method,
                      args: boundArgs,
                    }),
                }),
              )}
            </div>
          );
        })()}
      </section>
    );
  }

  const recipe = recipes.find(
    (candidate) => candidate.id === selection.recipeId,
  );
  if (!recipe)
    return <section className="context-panel">Recipe not found.</section>;

  return (
    <section className="context-panel">
      <div className="context-title-row">
        <h2>
          {recipe.emoji} {recipe.name}
        </h2>
      </div>
      <div className="context-desc">{recipe.desc}</div>
      {renderMethodCard({
        methodId: `${recipe.id}:${recipe.verb}`,
        methodName: recipe.verb,
        cpuCost: recipe.cpu,
        readsBlock: recipe.readsBlock,
        args: [
          ...recipe.consumes.map((c) => c.label),
          ...recipe.requires.map((r) => r.label),
        ],
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
