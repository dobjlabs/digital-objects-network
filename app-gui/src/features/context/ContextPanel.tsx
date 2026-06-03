import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DragEvent } from "react";
import type {
  ActionPayload as Action,
  ClassRefPayload,
  InventoryObjectPayload as InventoryObject,
  QualifiedNamePayload,
} from "../../shared/api/wireTypes";
import { pickDobjFilePath, readDobjFile } from "../../shared/api/tauriClient";
import { truncateDisplayHash } from "../../shared/format";
import {
  displayPathInObjectsDir,
  isNullifiedObject,
  joinObjectsDirPath,
  pluginScopedLabel,
  qualifiedEq,
  qualifiedId,
} from "../../shared/objectUtils";
import { isRecord, normalizePod2Value } from "../../shared/pod2utils";
import type { ContextSelection } from "../../shared/state/store";

interface ContextPanelProps {
  selection: ContextSelection;
  inventory: InventoryObject[];
  objectsDirPath: string;
  actions: Action[];
  onClearSelection: () => void;
  onRunProof: (input: {
    action: QualifiedNamePayload;
    inputBindings: Array<{
      objectPath: string;
      label: string;
    }>;
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
  objectsDirPath,
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
  const selectionKey =
    selection.kind === "object"
      ? `object:${selection.contentHash}`
      : selection.kind === "action"
        ? `action:${qualifiedId(selection.action)}`
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

  const parseDropPayload = (
    raw: string,
  ): {
    objectPath?: string;
    name?: string;
    class?: QualifiedNamePayload;
  } => {
    try {
      return JSON.parse(raw) as {
        objectPath?: string;
        name?: string;
        class?: QualifiedNamePayload;
      };
    } catch {
      return { name: raw };
    }
  };

  const handleDropArg = (
    event: DragEvent<HTMLDivElement>,
    methodId: string,
    expected: QualifiedNamePayload,
    expectedLabel: string,
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

    if (!parsed.class || !qualifiedEq(parsed.class, expected)) {
      const got = parsed.class
        ? pluginScopedLabel(parsed.class)
        : droppedName;
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${expectedLabel} but got ${got}`,
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
    expected: QualifiedNamePayload,
    expectedLabel: string,
    index: number,
  ) => {
    const key = argKey(methodId, index);
    let selectedPath = "";
    try {
      selectedPath = (await pickDobjFilePath()).trim();
    } catch (error) {
      const message =
        error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : "";
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
      class: QualifiedNamePayload;
      status: string;
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

    const objectIsLive = parsed.status === "live";
    const fileLabel = selectedName;

    if (!parsed.class) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Missing required fields in .dobj: ${selectedName}`,
      }));
      return;
    }

    if (!qualifiedEq(parsed.class, expected)) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${expectedLabel} but got ${pluginScopedLabel(parsed.class)}`,
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

  const renderClassChip = (label: string, classHash: string) => {
    const rawHash = classHash.trim();
    if (!rawHash) {
      return <span className="from-action-label">{label}</span>;
    }
    return (
      <span className="from-action-label" title={rawHash}>
        {label}
        <span className="proof-tooltip">{truncateDisplayHash(rawHash)}</span>
      </span>
    );
  };

  const renderMethodCard = (config: {
    methodId: string;
    methodName: string;
    totalInputs: ClassRefPayload[];
    onRun: (boundArgs: BoundArg[]) => void;
  }) =>
    (() => {
      const hasInputs = config.totalInputs.length > 0;
      const boundArgs = config.totalInputs.map(
        (_, index) => argBindings[argKey(config.methodId, index)] ?? null,
      );
      const filledCount = boundArgs.filter(
        (value) => value?.objectPath?.trim().length,
      ).length;
      const allArgsBound =
        !hasInputs || filledCount === config.totalInputs.length;

      return (
        <div className="method-card">
          {hasInputs && (
            <div className="method-card-body">
              {config.totalInputs.map((required, index) => {
                const key = argKey(config.methodId, index);
                const bound = argBindings[key];
                const isDropActive = hoverArgKey === key;
                const err = argErrors[key];
                const expectedClassLabel = pluginScopedLabel(required.class);

                return (
                  <div
                    key={`${qualifiedId(required.class)}:${index}`}
                    className="method-arg"
                  >
                    <div className="method-arg-row">
                      <span className="method-arg-label">
                        {renderClassChip(`# ${expectedClassLabel}`, required.hash)}
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
                            required.class,
                            expectedClassLabel,
                            index,
                          )
                        }
                      >
                        {bound?.label ??
                          (isDropActive
                            ? "release to drop"
                            : "drag .dobj here")}
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
                            required.class,
                            expectedClassLabel,
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
          <div
            className={`method-footer ${hasInputs ? "" : "no-inputs"}`.trim()}
          >
            <button
              type="button"
              className="method-execute"
              onClick={() =>
                config.onRun(boundArgs.filter(Boolean) as BoundArg[])
              }
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
    const absolutePath = joinObjectsDirPath(objectsDirPath, object.fileName, {
      nullified: isNullifiedObject(object),
    });
    return displayPathInObjectsDir(absolutePath, objectsDirPath);
  };

  const objectValueString = (value: unknown) => {
    const normalized = normalizePod2Value(value);
    if (typeof normalized === "string") return normalized;
    if (
      typeof normalized === "number" ||
      typeof normalized === "boolean" ||
      typeof normalized === "bigint"
    ) {
      return String(normalized);
    }
    if (normalized == null) {
      return "null";
    }
    try {
      return JSON.stringify(normalized);
    } catch {
      return String(normalized);
    }
  };

  const formatObjectValue = (value: unknown) => {
    const trimmed = objectValueString(value).trim();
    const isHexLike = (() => {
      if (/^0x[0-9a-f]+$/i.test(trimmed)) return true;
      if (!/^[0-9a-f]+$/i.test(trimmed)) return false;
      if (/[a-f]/i.test(trimmed)) return true;
      return trimmed.length >= 16;
    })();
    const normalizedHex = trimmed.startsWith("0x") ? trimmed : `0x${trimmed}`;
    const truncatedHex = truncateDisplayHash(normalizedHex);
    if (isHexLike && truncatedHex !== normalizedHex) {
      return {
        display: trimmed.startsWith("0x")
          ? truncatedHex
          : truncatedHex.slice("0x".length),
        full: trimmed,
        mono: true,
      };
    }
    return {
      display: trimmed,
      full: undefined,
      mono: isHexLike,
    };
  };

  const renderObjectData = (object: InventoryObject) => {
    const normalizedObject = normalizePod2Value(object.fields);
    const entries = isRecord(normalizedObject)
      ? Object.entries(normalizedObject).sort(([left], [right]) =>
          left.localeCompare(right),
        )
      : [["value", normalizedObject] as const];

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
      (candidate) => candidate.contentHash === selection.contentHash,
    );
    if (!object)
      return <section className="context-panel">Object not found.</section>;

    const titleName = pluginScopedLabel(object.class);
    const liveValueRaw =
      object.status === "live" ? object.contentHash : object.status;
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
              className={`context-inline-hash ${object.status}`}
              title={liveValueRaw}
            >
              {liveValue}
            </span>,
          )}
          {renderMetaRow(
            "Type",
            renderClassChip(`# ${titleName}`, object.classHash),
          )}
          {renderMetaRow(
            "Path",
            <span className="context-inline-path">
              {displayThingPath(object)}
            </span>,
          )}
        </div>

        {object.description && (
          <div className="context-desc">{object.description}</div>
        )}
        {renderObjectData(object)}
      </section>
    );
  }

  const action = actions.find((candidate) =>
    qualifiedEq(candidate.action, selection.action),
  );
  if (!action)
    return <section className="context-panel">Action not found.</section>;
  const actionHashRaw = action.hash.trim();
  const actionHashDisplay = truncateDisplayHash(actionHashRaw);
  const actionLabel = pluginScopedLabel(action.action);

  return (
    <section className="context-panel">
      <div className="context-title-row">
        <div className="context-title">
          {action.emoji} {actionLabel}
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
          "Type",
          <span className="context-inline-hash" title={actionHashRaw}>
            # {actionHashDisplay}
          </span>,
        )}
      </div>

      <div className="context-desc">{action.description}</div>

      {renderMethodCard({
        methodId: qualifiedId(action.action),
        methodName: actionLabel,
        totalInputs: action.totalInputs,
        onRun: (boundArgs) =>
          onRunProof({
            action: action.action,
            inputBindings: boundArgs.map((arg) => ({
              objectPath: arg.objectPath,
              label: arg.label,
            })),
          }),
      })}
    </section>
  );
}
