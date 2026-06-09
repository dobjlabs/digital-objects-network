import { useRef, useState } from "react";
import type { ChangeEvent, DragEvent } from "react";
import type { ObjectListingPayload as ObjectListing } from "../../shared/api/wireTypes";
import { truncateDisplayHash } from "../../shared/format";
import {
  displayPathInObjectsDir,
  isLiveObject,
  isNullifiedObject,
  joinObjectsDirPath,
  pluginScopedLabel,
} from "../../shared/objectUtils";

interface ObjectsPanelProps {
  objects: ObjectListing[];
  objectsDirPath: string;
  activeObjectContentHash: string | null;
  showNullifiedItems: boolean;
  onSelectObject: (contentHash: string) => void;
  onToggleNullified: () => void;
  onOpenObjectsDir: () => void;
  onImportObject: (dobj: string) => Promise<void>;
}

export function ObjectsPanel({
  objects,
  objectsDirPath,
  activeObjectContentHash,
  showNullifiedItems,
  onSelectObject,
  onToggleNullified,
  onOpenObjectsDir,
  onImportObject,
}: ObjectsPanelProps) {
  const isDraggingRef = useRef(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);

  const handleImportClick = () => {
    setImportError(null);
    fileInputRef.current?.click();
  };

  const handleImportFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    // Reset the input so picking the same file again still fires `change`.
    event.target.value = "";
    if (!file) return;
    setImporting(true);
    setImportError(null);
    try {
      const text = await file.text();
      await onImportObject(text);
    } catch (err) {
      setImportError(err instanceof Error ? err.message : String(err));
    } finally {
      setImporting(false);
    }
  };

  const isUsable = (object: ObjectListing) => isLiveObject(object);

  const handleDragStart = (
    event: DragEvent<HTMLButtonElement>,
    object: ObjectListing,
  ) => {
    if (!isUsable(object)) {
      event.preventDefault();
      return;
    }
    const objectPath = joinObjectsDirPath(objectsDirPath, object.fileName);
    const displayLabel = pluginScopedLabel(object.class);

    const payload = JSON.stringify({
      objectPath,
      name: displayLabel,
      class: object.class,
    });
    event.dataTransfer.setData("application/x-dobj-object", payload);
    event.dataTransfer.setData("text/plain", displayLabel);
    event.dataTransfer.setData("text", displayLabel);
    event.dataTransfer.effectAllowed = "copy";
    isDraggingRef.current = true;
  };

  const handleDragEnd = () => {
    isDraggingRef.current = false;
  };

  const handleClickObject = (contentHash: string) => {
    if (isDraggingRef.current) return;
    onSelectObject(contentHash);
  };

  const activeObjects = objects.filter((object) => !isNullifiedObject(object));
  const nullifiedObjects = objects.filter((object) => isNullifiedObject(object));

  const renderObjectListing = (object: ObjectListing) => {
    const displayName = pluginScopedLabel(object.class);
    const hashLineRaw = object.status === "live"
      ? object.contentHash
      : object.status;
    const hashLine = truncateDisplayHash(hashLineRaw);
    return (
      <button
        key={object.contentHash}
        type="button"
        className={`objects-item ${activeObjectContentHash === object.contentHash ? "active" : ""}`}
        onClick={() => handleClickObject(object.contentHash)}
        draggable={isUsable(object)}
        onDragStart={(event) => handleDragStart(event, object)}
        onDragEnd={handleDragEnd}
      >
        <span className="objects-file-icon">
          <span className="objects-emoji">{object.emoji}</span>
        </span>
        <span className="objects-main">
          <span className="objects-name">{displayName}</span>
          <span className="objects-hash" title={hashLineRaw}>
            {hashLine}
          </span>
        </span>
        <span
          className={`objects-dot ${object.status}`}
          title={object.status !== "live" && object.status !== "nullified" ? object.status : undefined}
        />
      </button>
    );
  };

  return (
    <section className="objects-panel">
      <button
        type="button"
        className="panel-header panel-header-button"
        onClick={onOpenObjectsDir}
        title={displayPathInObjectsDir(objectsDirPath, objectsDirPath)}
      >
        Your Objects
      </button>

      <div className="objects-import">
        <input
          ref={fileInputRef}
          type="file"
          accept=".dobj,application/json"
          style={{ display: "none" }}
          onChange={handleImportFile}
        />
        <button
          type="button"
          className="objects-import-button"
          onClick={handleImportClick}
          disabled={importing}
        >
          {importing ? "Importing…" : "+ Import .dobj"}
        </button>
        {importError && (
          <span className="objects-import-error" title={importError}>
            {importError}
          </span>
        )}
      </div>

      <div className="objects-list">
        {activeObjects.map(renderObjectListing)}

        {nullifiedObjects.length > 0 && (
          <div className="nullified-section">
            <button
              type="button"
              className="nullified-toggle"
              onClick={onToggleNullified}
            >
              <span className="nullified-label">nullified</span>
              <span className="nullified-count">
                {showNullifiedItems ? "▴" : "▾"} {nullifiedObjects.length}
              </span>
            </button>
            {showNullifiedItems && nullifiedObjects.map(renderObjectListing)}
          </div>
        )}
      </div>
    </section>
  );
}
