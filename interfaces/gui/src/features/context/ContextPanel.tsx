import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DragEvent } from "react";
import type {
  ActionArgPayload,
  ActionArgValues,
  ActionPayload as Action,
  ClassRefPayload,
  ObjectSummaryPayload as ObjectSummary,
  QualifiedNamePayload,
} from "../../shared/api/wireTypes";
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
  objects: ObjectSummary[];
  objectsDirPath: string;
  actions: Action[];
  onClearSelection: () => void;
  onRunProof: (input: {
    action: QualifiedNamePayload;
    inputBindings: Array<{
      objectPath: string;
      label: string;
    }>;
    args?: ActionArgValues;
  }) => Promise<void>;
  proofRunning: boolean;
  proofStatus: "idle" | "generating" | "committing" | "summary" | "error";
}

interface BoundArg {
  objectPath: string;
  label: string;
}

function validBinding(
  binding: BoundArg | undefined,
  required: ClassRefPayload,
  objects: ObjectSummary[],
): BoundArg | null {
  if (!binding) return null;
  return objects.some(
    (object) =>
      object.fileName === binding.objectPath &&
      object.status === "live" &&
      qualifiedEq(object.class, required.class),
  ) ? binding : null;
}

/** Parse one typed-in argument value by its declared pod2 type into the
 * JSON form dobjd accepts. Empty text means "use the script default". */
function parseArgValue(
  arg: ActionArgPayload,
  raw: string,
): { value?: unknown; error?: string } {
  const text = raw.trim();
  if (!text) return {};
  switch (arg.type) {
    case "Raw": {
      const hex = text.startsWith("0x") ? text.slice(2) : text;
      if (!/^[0-9a-fA-F]{1,64}$/.test(hex)) {
        return { error: "expected up to 64 hex digits" };
      }
      return { value: { Raw: hex.toLowerCase().padStart(64, "0") } };
    }
    case "Int":
      if (!/^-?\d+$/.test(text)) return { error: "expected an integer" };
      return { value: { Int: text } };
    case "Str":
      return { value: text };
    default:
      try {
        return { value: JSON.parse(text) };
      } catch {
        return { error: "expected a pod2 value in JSON form" };
      }
  }
}

function parseActionArgs(
  args: ActionArgPayload[],
  values: Record<string, string>,
): { args: ActionArgValues; errors: Record<string, string> } {
  const parsed: ActionArgValues = Object.create(null);
  const errors: Record<string, string> = Object.create(null);
  for (const arg of args) {
    const { value, error } = parseArgValue(arg, values[arg.name] ?? "");
    if (error) errors[arg.name] = error;
    else if (value !== undefined) parsed[arg.name] = value;
  }
  return { args: parsed, errors };
}

export function ContextPanel({
  selection,
  objects,
  objectsDirPath,
  actions,
  onClearSelection,
  onRunProof,
  proofRunning,
  proofStatus,
}: ContextPanelProps) {
  const [argBindings, setArgBindings] = useState<Record<string, BoundArg>>({});
  // Typed-in values for the selected action's declared arguments, keyed by
  // `<action id>::<arg name>` so switching actions keeps each one's text.
  const [argValues, setArgValues] = useState<Record<string, string>>({});
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
    setArgBindings((previous) => {
      const next: Record<string, BoundArg> = {};
      for (const action of actions) {
        action.totalInputs.forEach((required, index) => {
          const key = `action:${qualifiedId(action.action)}:${index}`;
          const binding = validBinding(previous[key], required, objects);
          if (binding) next[key] = binding;
        });
      }
      return Object.keys(next).length === Object.keys(previous).length
        ? previous
        : next;
    });
  }, [objects, actions]);

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
      event.dataTransfer.getData("application/x-dobj-object") ||
      event.dataTransfer.getData("application/x-dobj-object") ||
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
    /** When set, the run button is disabled and shows this label. */
    runBlocked?: string;
  }) =>
    (() => {
      const hasInputs = config.totalInputs.length > 0;
      const boundArgs = config.totalInputs.map(
        (required, index) =>
          validBinding(
            argBindings[argKey(config.methodId, index)], required, objects,
          ),
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
                const bound = boundArgs[index];
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
                      <select
                        className="method-arg-browse"
                        aria-label={`Choose ${expectedClassLabel} for input ${index + 1}`}
                        disabled={proofRunning}
                        value={bound?.objectPath ?? ""}
                        onChange={(event) => {
                          const fileName = event.target.value;
                          setArgBindings((prev) => {
                            const next = { ...prev };
                            if (fileName) {
                              next[key] = { objectPath: fileName, label: fileName };
                            } else {
                              delete next[key];
                            }
                            return next;
                          });
                          setArgErrors((prev) => {
                            const next = { ...prev };
                            delete next[key];
                            return next;
                          });
                        }}
                      >
                        <option value="">Choose object…</option>
                        {objects
                          .filter((object) => object.status === "live" && qualifiedEq(object.class, required.class))
                          .map((object) => (
                            <option key={object.contentHash} value={object.fileName}>
                              {object.fileName}
                            </option>
                          ))}
                      </select>
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
              disabled={proofRunning || !allArgsBound || !!config.runBlocked}
            >
              {proofRunning
                ? "running..."
                : (config.runBlocked ??
                  (allArgsBound ? config.methodName : "bind all inputs"))}
            </button>
          </div>
        </div>
      );
    })();

  const displayObjectPath = (object: ObjectSummary) => {
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

  const renderObjectData = (object: ObjectSummary) => {
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
    const object = objects.find(
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
              {displayObjectPath(object)}
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
  const actionId = qualifiedId(action.action);
  const declaredArgs = action.args ?? [];
  const argValueKey = (name: string) => `${actionId}::${name}`;
  const typedArgs = Object.fromEntries(
    declaredArgs.map((arg) => [arg.name, argValues[argValueKey(arg.name)] ?? ""]),
  );
  const parsedArgs = parseActionArgs(declaredArgs, typedArgs);
  const argErrorCount = Object.keys(parsedArgs.errors).length;

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

      {declaredArgs.length > 0 && (
        <div className="method-args-block">
          <div className="method-args-title">
            arguments (blank = script default)
          </div>
          {declaredArgs.map((arg) => {
            const err = parsedArgs.errors[arg.name];
            return (
              <div key={arg.name} className="method-arg">
                <div className="method-arg-row">
                  <span
                    className="method-arg-label"
                    title={`${arg.type}; default: ${arg.default}`}
                  >
                    {arg.name}: {arg.type}
                  </span>
                  <input
                    type="text"
                    className={`method-arg-input ${err ? "error" : ""}`.trim()}
                    value={typedArgs[arg.name]}
                    placeholder={`default: ${arg.default}`}
                    disabled={proofRunning}
                    spellCheck={false}
                    onChange={(event) => {
                      const text = event.target.value;
                      setArgValues((prev) => ({
                        ...prev,
                        [argValueKey(arg.name)]: text,
                      }));
                    }}
                  />
                </div>
                {err && <div className="method-arg-error">{err}</div>}
              </div>
            );
          })}
        </div>
      )}

      {renderMethodCard({
        methodId: actionId,
        methodName: actionLabel,
        totalInputs: action.totalInputs,
        runBlocked: argErrorCount > 0 ? "fix arguments" : undefined,
        onRun: (boundArgs) =>
          onRunProof({
            action: action.action,
            inputBindings: boundArgs.map((arg) => ({
              objectPath: arg.objectPath,
              label: arg.label,
            })),
            args: parsedArgs.args,
          }),
      })}
    </section>
  );
}
