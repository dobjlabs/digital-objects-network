import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DragEvent } from "react";
import type {
  ContextSelection,
  InventoryItem,
  MethodArg,
  Recipe,
} from "../../shared/types/domain";
import {
  pickDobjFilePath,
  readDobjFile,
  type ActionId,
} from "../../shared/api/tauriClient";
import {
  objectDisplayFileName,
  objectDisplayFileNameForClass,
} from "../../shared/objectDisplay";

interface ContextPanelProps {
  selection: ContextSelection;
  items: InventoryItem[];
  recipes: Recipe[];
  onClearSelection: () => void;
  onRunProof: (input: {
    actionId: ActionId;
    methodName: string;
    inputBindings: Array<{
      objectPath: string;
      label: string;
    }>;
    cpuCost: string;
  }) => Promise<void>;
  proofRunning: boolean;
  proofStatus: "idle" | "generating" | "committing" | "summary" | "error";
}

interface BoundArg {
  objectPath: string;
  label: string;
}

export function ContextPanel({
  selection,
  items,
  recipes,
  onClearSelection,
  onRunProof,
  proofRunning,
  proofStatus,
}: ContextPanelProps) {
  const [argBindings, setArgBindings] = useState<Record<string, BoundArg>>({});
  const [hoverArgKey, setHoverArgKey] = useState<string | null>(null);
  const [argErrors, setArgErrors] = useState<Record<string, string>>({});
  const previousProofStatusRef = useRef(proofStatus);
  const isLive = (item: InventoryItem) => item.nullifier == null;
  const selectionKey =
    selection.kind === "item"
      ? `item:${selection.itemId}`
      : selection.kind === "recipe"
        ? `recipe:${selection.recipeId}`
        : "none";

  useEffect(() => {
    setArgErrors({});
    setHoverArgKey(null);
  }, [selectionKey]);

  useEffect(() => {
    const previous = previousProofStatusRef.current;
    if (previous === "summary" && proofStatus === "idle") {
      setArgBindings({});
      setArgErrors({});
      setHoverArgKey(null);
    }
    previousProofStatusRef.current = proofStatus;
  }, [proofStatus]);

  const argKey = (methodId: string, index: number) =>
    `${selection.kind}:${methodId}:${index}`;

  const parseDropPayload = (raw: string): {
    objectPath?: string;
    name?: string;
    className?: string;
    classHash?: string;
  } => {
    try {
      return JSON.parse(raw) as {
        objectPath?: string;
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
    if (proofRunning) return;
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
    const droppedPath = parsed.objectPath?.trim() ?? "";

    if (!isArgCompatible(arg, parsed.classHash, parsed.className)) {
      const got = parsed.className ?? droppedName;
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${arg.label} but got ${got}`,
      }));
      return;
    }

    if (!droppedPath) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Dropped object missing path",
      }));
      return;
    }
    if (
      droppedPath.includes("/.nullified/") ||
      droppedPath.includes("\\.nullified\\")
    ) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Only live objects can be bound",
      }));
      return;
    }

    setArgBindings((prev) => ({
      ...prev,
      [key]: {
        objectPath: droppedPath,
        label: droppedName,
      },
    }));
    setArgErrors((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
    setHoverArgKey(null);
  };

  const fileNameFromPath = (path: string) => {
    const normalized = path.replace(/\\/g, "/");
    const index = normalized.lastIndexOf("/");
    if (index === -1) return normalized;
    return normalized.slice(index + 1);
  };

  const handleBrowseArgFile = async (
    methodId: string,
    arg: MethodArg,
    index: number,
  ) => {
    const key = argKey(methodId, index);
    let selectedPath = "";
    try {
      selectedPath = (await pickDobjFilePath()).trim();
    } catch (error) {
      const message =
        error instanceof Error ? error.message : typeof error === "string" ? error : "";
      if (message.includes("No file selected")) {
        return;
      }
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Failed to open file picker",
      }));
      return;
    }
    if (!selectedPath) return;

    const selectedName = fileNameFromPath(selectedPath);
    if (!selectedName.toLowerCase().endsWith(".dobj")) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Only .dobj files are supported",
      }));
      return;
    }

    let parsed: {
      className: string;
      nullifier: string | null;
    };
    try {
      parsed = await readDobjFile(selectedPath);
    } catch {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Invalid .dobj file: ${selectedName}`,
      }));
      return;
    }

    const className = parsed.className.trim();
    const isLive = parsed.nullifier === null;
    const fileLabel = objectDisplayFileNameForClass(className);

    if (!className) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Missing required fields in .dobj: ${selectedName}`,
      }));
      return;
    }

    if (
      !isArgCompatible(arg, undefined, className)
    ) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${arg.label} but got ${className}`,
      }));
      return;
    }

    if (!isLive) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: "Only live objects can be bound",
      }));
      return;
    }

    setArgBindings((prev) => ({
      ...prev,
      [key]: { objectPath: selectedPath, label: fileLabel },
    }));
    setArgErrors((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
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
        (value) => value?.objectPath?.trim().length,
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
                          if (proofRunning) return;
                          event.preventDefault();
                          setHoverArgKey(key);
                        }}
                        onDragLeave={() =>
                          setHoverArgKey((prev) => (prev === key ? null : prev))
                        }
                        onDragOver={(event) => {
                          if (proofRunning) return;
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
                        disabled={proofRunning}
                        onClick={() => {
                          if (proofRunning) return;
                          if (bound?.objectPath) {
                            setArgBindings((prev) => {
                              const next = { ...prev };
                              delete next[key];
                              return next;
                            });
                            return;
                          }
                          void handleBrowseArgFile(config.methodId, arg, index);
                        }}
                      >
                        {bound?.objectPath ? "Clear" : "Browse..."}
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

  const displayThingPath = (item: InventoryItem) => {
    const displayName = objectDisplayFileName(item);
    return !isLive(item)
      ? `~/.objects/.nullified/${displayName}`
      : `~/.objects/${displayName}`;
  };

  const truncateDisplayHash = (value: string) => {
    const trimmed = value.trim();
    if (!/^0x[0-9a-f]+$/i.test(trimmed)) return trimmed;
    if (trimmed.length <= 14) return trimmed;
    return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
  };

  const formatObjectValue = (value: string) => {
    const trimmed = value.trim();
    const isHex = /^0x[0-9a-f]+$/i.test(trimmed);
    if (isHex && trimmed.length > 24) {
      return {
        display: `${trimmed.slice(0, 10)}...${trimmed.slice(-8)}`,
        full: trimmed,
        mono: true,
      };
    }
    return {
      display: trimmed,
      full: undefined,
      mono: isHex,
    };
  };

  const renderObjectData = (item: InventoryItem) => {
    if (item.obj.length === 0) return null;
    return (
      <div className="object-data">
        {item.obj.map((entry) => {
          const formatted = formatObjectValue(entry.value);
          return (
            <div key={entry.key} className="object-data-row">
              <span className="object-data-key">{entry.key}</span>
              <span
                className={`object-data-value${formatted.mono ? " mono" : ""}`}
                title={formatted.full}
              >
                {formatted.display}
              </span>
            </div>
          );
        })}
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

    const titleName = item.classMeta.name;
    const liveValueRaw = isLive(item)
      ? item.id
      : (item.nullifier ?? "nullified");
    const liveValue = truncateDisplayHash(liveValueRaw);

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
              className={`context-inline-hash ${isLive(item) ? "live" : "nullified"}`}
              title={liveValueRaw}
            >
              {liveValue}
            </span>,
          )}
          {renderMetaRow(
            "Type",
            <span className="from-action-label"># {item.classMeta.name}</span>,
          )}
          {renderMetaRow(
            "Path",
            <span className="context-inline-path">
              {displayThingPath(item)}
            </span>,
          )}
        </div>

        {item.description && <div className="context-desc">{item.description}</div>}
        {renderObjectData(item)}
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
        {renderMetaRow(
          "Type",
          <span className="from-action-label">
            # {truncateDisplayHash(recipe.hash)}
          </span>,
        )}
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
            inputBindings: boundArgs.map((arg) => ({
              objectPath: arg.objectPath,
              label: arg.label,
            })),
            cpuCost: recipe.cpu,
          }),
      })}
    </section>
  );
}
