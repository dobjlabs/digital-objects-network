import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DragEvent } from "react";
import type {
  ActionPayload as Action,
  InventoryObjectPayload as InventoryObject,
} from "../../shared/api/wireTypes";
import {
  pickDobjFilePath,
  readDobjFile,
  type ActionId,
} from "../../shared/api/tauriClient";
import type { ContextSelection } from "../../shared/state/store";
import {
  objectDisplayFileName,
  objectDisplayFileNameForClass,
} from "../../shared/objectDisplay";

interface ContextPanelProps {
  selection: ContextSelection;
  inventory: InventoryObject[];
  actions: Action[];
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
  inventory,
  actions,
  onClearSelection,
  onRunProof,
  proofRunning,
  proofStatus,
}: ContextPanelProps) {
  const [argBindings, setArgBindings] = useState<Record<string, BoundArg>>({});
  const [hoverArgKey, setHoverArgKey] = useState<string | null>(null);
  const [argErrors, setArgErrors] = useState<Record<string, string>>({});
  const previousProofStatusRef = useRef(proofStatus);
  const isLive = (object: InventoryObject) => object.nullifier == null;
  const selectionKey =
    selection.kind === "object"
      ? `object:${selection.objectId}`
      : selection.kind === "action"
        ? `action:${selection.actionId}`
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
  } => {
    try {
      return JSON.parse(raw) as {
        objectPath?: string;
        name?: string;
        className?: string;
      };
    } catch {
      return { name: raw };
    }
  };

  const normalizeName = (value: string) => value.trim().toLowerCase();

  const isArgCompatible = (expectedClassName: string, droppedClassName?: string) => {
    if (!droppedClassName) return false;
    return normalizeName(droppedClassName) === normalizeName(expectedClassName);
  };

  const handleDropArg = (
    event: DragEvent<HTMLDivElement>,
    methodId: string,
    expectedClassName: string,
    index: number,
  ) => {
    if (proofRunning) return;
    event.preventDefault();
    event.stopPropagation();
    const raw =
      event.dataTransfer.getData("application/x-zkcraft-object") ||
      event.dataTransfer.getData("application/x-zkcraft-item") ||
      event.dataTransfer.getData("text/plain") ||
      event.dataTransfer.getData("text");
    if (!raw) return;

    const parsed = parseDropPayload(raw);
    const key = argKey(methodId, index);
    const droppedName = parsed.name ?? raw;
    const droppedPath = parsed.objectPath?.trim() ?? "";

    if (!isArgCompatible(expectedClassName, parsed.className)) {
      const got = parsed.className ?? droppedName;
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${expectedClassName} but got ${got}`,
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
    expectedClassName: string,
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
    const objectIsLive = parsed.nullifier === null;
    const fileLabel = objectDisplayFileNameForClass(className);

    if (!className) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Missing required fields in .dobj: ${selectedName}`,
      }));
      return;
    }

    if (!isArgCompatible(expectedClassName, className)) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${expectedClassName} but got ${className}`,
      }));
      return;
    }

    if (!objectIsLive) {
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
    inputClasses: string[];
    onRun: (boundArgs: BoundArg[]) => void;
  }) =>
    (() => {
      const boundArgs = config.inputClasses.map(
        (_, index) => argBindings[argKey(config.methodId, index)] ?? null,
      );
      const filledCount = boundArgs.filter(
        (value) => value?.objectPath?.trim().length,
      ).length;
      const allArgsBound =
        config.inputClasses.length === 0 ||
        filledCount === config.inputClasses.length;

      return (
        <div className="method-card">
          {config.inputClasses.length > 0 && (
            <div className="method-card-body">
              {config.inputClasses.map((expectedClassName, index) => {
                const key = argKey(config.methodId, index);
                const bound = argBindings[key];
                const isDropActive = hoverArgKey === key;
                const err = argErrors[key];

                return (
                  <div key={`${expectedClassName}:${index}`} className="method-arg">
                    <div className="method-arg-row">
                      <span className="method-arg-label">
                        <span className="from-action-label">
                          # {expectedClassName}
                        </span>
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
                          handleDropArg(
                            event,
                            config.methodId,
                            expectedClassName,
                            index,
                          )
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
                          void handleBrowseArgFile(
                            config.methodId,
                            expectedClassName,
                            index,
                          );
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

  const displayThingPath = (object: InventoryObject) => {
    const displayName = objectDisplayFileName(object);
    return !isLive(object)
      ? `~/.objects/.nullified/${displayName}`
      : `~/.objects/${displayName}`;
  };

  const truncateDisplayHash = (value: string) => {
    const trimmed = value.trim();
    if (!/^0x[0-9a-f]+$/i.test(trimmed)) return trimmed;
    if (trimmed.length <= 14) return trimmed;
    return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
  };

  const objectValueString = (value: unknown) => {
    if (typeof value === "string") {
      const trimmed = value.trim();
      const rawInner = trimmed
        .trim()
        .replace(/^Raw\((.*)\)$/, "$1")
        .trim();
      return rawInner;
    }
    if (
      typeof value === "number" ||
      typeof value === "boolean" ||
      typeof value === "bigint"
    ) {
      return String(value);
    }
    if (value == null) {
      return "null";
    }
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  };

  const formatObjectValue = (value: unknown) => {
    const trimmed = objectValueString(value).trim();
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

  const renderObjectData = (object: InventoryObject) => {
    const entries = Object.entries(object.obj).sort(([left], [right]) =>
      left.localeCompare(right),
    );
    if (entries.length === 0) return null;

    return (
      <div className="object-data">
        {entries.map(([key, value]) => {
          const formatted = formatObjectValue(value);
          return (
            <div key={key} className="object-data-row">
              <span className="object-data-key">{key}</span>
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

  if (selection.kind === "object") {
    const object = inventory.find(
      (candidate) => candidate.id === selection.objectId,
    );
    if (!object)
      return <section className="context-panel">Object not found.</section>;

    const titleName = object.className;
    const liveValueRaw = isLive(object)
      ? object.id
      : (object.nullifier ?? "nullified");
    const liveValue = truncateDisplayHash(liveValueRaw);

    return (
      <section className="context-panel">
        <div className="context-title-row">
          <div className="context-title">
            {object.emoji} {titleName}
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
              className={`context-inline-hash ${isLive(object) ? "live" : "nullified"}`}
              title={liveValueRaw}
            >
              {liveValue}
            </span>,
          )}
          {renderMetaRow(
            "Type",
            <span className="from-action-label"># {object.className}</span>,
          )}
          {renderMetaRow(
            "Path",
            <span className="context-inline-path">{displayThingPath(object)}</span>,
          )}
        </div>

        {object.description && <div className="context-desc">{object.description}</div>}
        {renderObjectData(object)}
      </section>
    );
  }

  const action = actions.find((candidate) => candidate.id === selection.actionId);
  if (!action) return <section className="context-panel">Action not found.</section>;

  return (
    <section className="context-panel">
      <div className="context-title-row">
        <div className="context-title">
          {action.emoji} {action.id}
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

      <div className="context-desc">{action.description}</div>

      {renderMethodCard({
        methodId: action.id,
        methodName: action.id,
        cpuCost: action.cpuCost,
        readsBlock: action.readsBlock,
        inputClasses: action.inputClasses,
        onRun: (boundArgs) =>
          onRunProof({
            actionId: action.id,
            methodName: action.id,
            inputBindings: boundArgs.map((arg) => ({
              objectPath: arg.objectPath,
              label: arg.label,
            })),
            cpuCost: action.cpuCost,
          }),
      })}
    </section>
  );
}
