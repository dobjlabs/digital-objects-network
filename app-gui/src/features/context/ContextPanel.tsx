import { useEffect, useState, type ReactNode } from "react";
import type { DragEvent } from "react";
import type {
  ContextSelection,
  FieldValue,
  InventoryItem,
  MethodArg,
  Recipe,
} from "../../shared/types/domain";

interface ContextPanelProps {
  selection: ContextSelection;
  items: InventoryItem[];
  recipes: Recipe[];
  onClearSelection: () => void;
  onRunProof: (input: {
    actionId: string;
    methodName: string;
    inputObjectIds: string[];
    inputLabels: string[];
    cpuCost: string;
  }) => void;
  proofRunning: boolean;
}

interface BoundArg {
  objectId: string;
  label: string;
}

export function ContextPanel({
  selection,
  items,
  recipes,
  onClearSelection,
  onRunProof,
  proofRunning,
}: ContextPanelProps) {
  const [argBindings, setArgBindings] = useState<Record<string, BoundArg>>({});
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

  const argKey = (methodId: string, index: number) =>
    `${selection.kind}:${methodId}:${index}`;

  const parseDropPayload = (raw: string): {
    itemId?: string;
    name?: string;
    className?: string;
    classHash?: string;
  } => {
    try {
      return JSON.parse(raw) as {
        itemId?: string;
        name?: string;
        className?: string;
        classHash?: string;
      };
    } catch {
      return { name: raw };
    }
  };

  const normalizeName = (value: string) => value.trim().toLowerCase();

  const isArgCompatible = (
    arg: MethodArg,
    droppedClassHash?: string,
    droppedClassName?: string,
  ) => {
    if (droppedClassHash && droppedClassHash === arg.classHash) return true;
    if (!droppedClassName) return false;
    return normalizeName(droppedClassName) === normalizeName(arg.label);
  };

  const handleDropArg = (
    event: DragEvent<HTMLDivElement>,
    methodId: string,
    arg: MethodArg,
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
    const key = argKey(methodId, index);
    const droppedName = parsed.name ?? raw;
    const droppedId = parsed.itemId;

    if (!isArgCompatible(arg, parsed.classHash, parsed.className)) {
      const got = parsed.className ?? droppedName;
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${arg.label} but got ${got}`,
      }));
      return;
    }

    if (!droppedId) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Dropped object missing ID",
      }));
      return;
    }

    const droppedItem = items.find((candidate) => candidate.id === droppedId);
    if (!droppedItem) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Dropped object not found",
      }));
      return;
    }

    if (droppedItem.validity !== "live") {
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Only live objects can be bound",
      }));
      return;
    }

    setArgBindings((prev) => ({
      ...prev,
      [key]: { objectId: droppedId, label: droppedName },
    }));
    setArgErrors((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
    setHoverArgKey(null);
  };

  const renderHashChip = (label: string, hash: string) => (
    <span className="from-action-label">
      {label}
      <span className="proof-tooltip">{hash}</span>
    </span>
  );

  const renderMetaRow = (label: string, value: ReactNode) => (
    <div className="context-meta-row">
      <span className="context-meta-key">{label}</span>
      <span className="context-meta-val">{value}</span>
    </div>
  );

  const renderMethodCard = (config: {
    methodId: string;
    methodName: string;
    cpuCost: string;
    readsBlock: boolean;
    args: MethodArg[];
    onRun: (boundArgs: BoundArg[]) => void;
  }) =>
    (() => {
      const boundArgs = config.args.map(
        (_, index) => argBindings[argKey(config.methodId, index)] ?? null,
      );
      const filledCount = boundArgs.filter(
        (value) => value?.objectId?.trim().length,
      ).length;
      const allArgsBound =
        config.args.length === 0 || filledCount === config.args.length;

      return (
        <div className="method-card">
          {config.args.length > 0 && (
            <div className="method-card-body">
              {config.args.map((arg, index) => {
                const key = argKey(config.methodId, index);
                const bound = argBindings[key];
                const isDropActive = hoverArgKey === key;
                const err = argErrors[key];

                return (
                  <div key={`${arg.classHash}:${index}`} className="method-arg">
                    <div className="method-arg-row">
                      <span className="method-arg-label">
                        {renderHashChip(`# ${arg.label}`, arg.classHash)}
                      </span>
                      <div
                        className={`method-arg-drop ${bound ? "filled" : ""} ${isDropActive ? "drop-active" : ""} ${err ? "error" : ""}`}
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
                        {bound?.label ??
                          (isDropActive ? "release to drop" : "drag .dobj here")}
                      </div>
                      <button
                        type="button"
                        className="method-arg-browse"
                        onClick={() => {
                          if (!bound?.objectId) return;
                          setArgBindings((prev) => {
                            const next = { ...prev };
                            delete next[key];
                            return next;
                          });
                        }}
                      >
                        {bound?.objectId ? "Clear" : "Browse..."}
                      </button>
                    </div>
                    {err && <div className="method-arg-error">{err}</div>}
                  </div>
                );
              })}
            </div>
          )}
          <div className="method-footer">
            <div className="method-meta-row">
              <div className="method-meta-line">
                ⏱ CPU <span className="mval">{config.cpuCost}</span>
              </div>
              {config.readsBlock && (
                <div className="method-meta-line">reads block number</div>
              )}
            </div>
            <button
              type="button"
              className="method-execute"
              onClick={() => config.onRun(boundArgs.filter(Boolean) as BoundArg[])}
              disabled={proofRunning || !allArgsBound}
            >
              {proofRunning
                ? "running..."
                : allArgsBound
                  ? config.methodName
                  : "bind all inputs"}
            </button>
          </div>
        </div>
      );
    })();

  const displayThingPath = (
    filename: string,
    validity: InventoryItem["validity"],
  ) =>
    validity === "nullified"
      ? `~/.objects/.nullified/${filename}`
      : `~/.objects/${filename}`;

  const stringifyField = (value: FieldValue) => {
    if (value === null) return "null";
    if (typeof value === "boolean") return value ? "true" : "false";
    return `${value}`;
  };

  const renderItemStats = (item: InventoryItem) => {
    if (item.stats.length === 0) return null;
    return (
      <div className="item-stats">
        {item.stats.map((stat) => (
          <div key={stat.key} className="stat-row">
            <span className="stat-key">{stat.key}</span>
            <span className={`stat-val ${stat.tone ?? "good"}`.trim()}>
              {stringifyField(stat.value)}
            </span>
          </div>
        ))}
      </div>
    );
  };

  if (selection.kind === "none") {
    return (
      <section className="context-panel context-empty">
        <span>
          select an object
          <br />
          or action
        </span>
      </section>
    );
  }

  if (selection.kind === "item") {
    const item = items.find((candidate) => candidate.id === selection.itemId);
    if (!item)
      return <section className="context-panel">Object not found.</section>;

    const titleName = item.fileName.replace(/\.dobj$/i, "");

    return (
      <section className="context-panel">
        <div className="context-title-row">
          <div className="context-title">
            {item.emoji} {titleName}
          </div>
          <button
            type="button"
            className="context-clear-btn"
            onClick={onClearSelection}
            title="Clear selection"
          >
            x
          </button>
        </div>

        <div className="context-meta-block compact">
          {renderMetaRow(
            "Live",
            <span
              className={`context-inline-hash ${item.validity === "live" ? "live" : "nullified"}`}
            >
              {item.validity === "live"
                ? item.stateRoot
                : (item.nullifier ?? "nullified")}
            </span>,
          )}
          {renderMetaRow(
            "Type",
            renderHashChip(`# ${item.classMeta.name}`, item.classMeta.hash),
          )}
          {renderMetaRow(
            "Path",
            <span className="context-inline-path">
              {displayThingPath(item.fileName, item.validity)}
            </span>,
          )}
        </div>

        {item.description && <div className="context-desc">{item.description}</div>}
        {renderItemStats(item)}
      </section>
    );
  }

  const recipe = recipes.find(
    (candidate) => candidate.id === selection.recipeId,
  );
  if (!recipe)
    return <section className="context-panel">Action not found.</section>;

  return (
    <section className="context-panel">
      <div className="context-title-row">
        <div className="context-title">
          {recipe.emoji} {recipe.name}
        </div>
        <button
          type="button"
          className="context-clear-btn"
          onClick={onClearSelection}
          title="Clear selection"
        >
          x
        </button>
      </div>

      <div className="context-meta-block">
        {renderMetaRow("Type", renderHashChip(`# ${recipe.hash}`, recipe.hash))}
      </div>

      <div className="context-desc">{recipe.desc}</div>

      {renderMethodCard({
        methodId: `${recipe.id}:${recipe.verb}`,
        methodName: recipe.verb,
        cpuCost: recipe.cpu,
        readsBlock: recipe.readsBlock,
        args: recipe.args,
        onRun: (boundArgs) =>
          onRunProof({
            actionId: recipe.id,
            methodName: recipe.verb,
            inputObjectIds: boundArgs.map((arg) => arg.objectId),
            inputLabels: boundArgs.map((arg) => arg.label),
            cpuCost: recipe.cpu,
          }),
      })}
    </section>
  );
}
