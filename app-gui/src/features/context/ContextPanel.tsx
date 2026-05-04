import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DragEvent } from "react";
import type {
  ActionPayload as Action,
  InventoryObjectPayload as InventoryObject,
} from "../../shared/api/wireTypes";
import { pickDobjFilePath, readDobjFile } from "../../shared/api/tauriClient";
import { truncateDisplayHash } from "../../shared/format";
import {
  displayPathInObjectsDir,
  isNullifiedObject,
  joinObjectsDirPath,
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
    actionId: string;
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

  const parseDropPayload = (
    raw: string,
  ): {
    objectPath?: string;
    name?: string;
    classId?: string;
    classDisplayName?: string;
  } => {
    try {
      return JSON.parse(raw) as {
        objectPath?: string;
        name?: string;
        classId?: string;
        classDisplayName?: string;
      };
    } catch {
      return { name: raw };
    }
  };

  /** Compatibility is by qualified class id; bare names alone are ambiguous
   * across plugins. */
  const isArgCompatible = (
    expectedClassId: string,
    droppedClassId?: string,
  ) => {
    if (!droppedClassId) return false;
    return droppedClassId === expectedClassId;
  };

  const handleDropArg = (
    event: DragEvent<HTMLDivElement>,
    methodId: string,
    expectedClassId: string,
    expectedClassDisplay: string,
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

    if (!isArgCompatible(expectedClassId, parsed.classId)) {
      const got = parsed.classDisplayName ?? parsed.classId ?? droppedName;
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${expectedClassDisplay} but got ${got}`,
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
    expectedClassId: string,
    expectedClassDisplay: string,
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
      classId: string;
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

    const classId = parsed.classId.trim();
    const objectIsLive = parsed.status === "live";
    const fileLabel = selectedName;

    if (!classId) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Missing required fields in .dobj: ${selectedName}`,
      }));
      return;
    }

    if (!isArgCompatible(expectedClassId, classId)) {
      setArgErrors((prev) => ({
        ...prev,
        [key]: `Expected ${expectedClassDisplay} but got ${classId}`,
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
    totalInputClassIds: string[];
    totalInputClassNames: string[];
    totalInputClassHashes: string[];
    onRun: (boundArgs: BoundArg[]) => void;
  }) =>
    (() => {
      const hasInputs = config.totalInputClassIds.length > 0;
      const boundArgs = config.totalInputClassIds.map(
        (_, index) => argBindings[argKey(config.methodId, index)] ?? null,
      );
      const filledCount = boundArgs.filter(
        (value) => value?.objectPath?.trim().length,
      ).length;
      const allArgsBound =
        !hasInputs || filledCount === config.totalInputClassIds.length;

      return (
        <div className="method-card">
          {hasInputs && (
            <div className="method-card-body">
              {config.totalInputClassIds.map((expectedClassId, index) => {
                const key = argKey(config.methodId, index);
                const bound = argBindings[key];
                const isDropActive = hoverArgKey === key;
                const err = argErrors[key];
                const classHash = config.totalInputClassHashes[index] ?? "";
                const expectedClassLabel =
                  config.totalInputClassNames[index] ?? expectedClassId;

                return (
                  <div
                    key={`${expectedClassId}:${index}`}
                    className="method-arg"
                  >
                    <div className="method-arg-row">
                      <span className="method-arg-label">
                        {renderClassChip(`# ${expectedClassLabel}`, classHash)}
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
                            expectedClassId,
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
                            expectedClassId,
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
    const normalizedObject = normalizePod2Value(object.obj);
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
      (candidate) => candidate.id === selection.objectId,
    );
    if (!object)
      return <section className="context-panel">Object not found.</section>;

    const titleName = object.classDisplayName;
    const liveValueRaw = object.status === "live" ? object.id : object.status;
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

  const action = actions.find(
    (candidate) => candidate.id === selection.actionId,
  );
  if (!action)
    return <section className="context-panel">Action not found.</section>;
  const actionHashRaw = action.hash.trim();
  const actionHashDisplay = truncateDisplayHash(actionHashRaw);
  const actionLabel = action.displayName;

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
        methodId: action.id,
        methodName: actionLabel,
        totalInputClassIds: action.totalInputClassIds,
        totalInputClassNames: action.totalInputClassNames,
        totalInputClassHashes: action.totalInputClassHashes,
        onRun: (boundArgs) =>
          onRunProof({
            actionId: action.id,
            inputBindings: boundArgs.map((arg) => ({
              objectPath: arg.objectPath,
              label: arg.label,
            })),
          }),
      })}
    </section>
  );
}
