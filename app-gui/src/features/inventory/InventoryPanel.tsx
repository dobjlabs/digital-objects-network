import { useRef } from "react";
import type { DragEvent } from "react";
import type { InventoryObjectPayload as InventoryObject } from "../../shared/api/wireTypes";
import { truncateDisplayHash } from "../../shared/format";
import {
  displayPathInObjectsDir,
  displayObjectFileName,
  isLiveObject,
  joinObjectsDirPath,
} from "../../shared/objectUtils";

interface InventoryPanelProps {
  inventory: InventoryObject[];
  objectsDirPath: string;
  activeObjectId: string | null;
  showNullifiedItems: boolean;
  onSelectObject: (objectId: string) => void;
  onToggleNullified: () => void;
  onOpenObjectsDir: () => void;
}

export function InventoryPanel({
  inventory,
  objectsDirPath,
  activeObjectId,
  showNullifiedItems,
  onSelectObject,
  onToggleNullified,
  onOpenObjectsDir,
}: InventoryPanelProps) {
  const isDraggingRef = useRef(false);

  const isUsable = (object: InventoryObject) =>
    isLiveObject(object) && object.grounded;

  const handleDragStart = (
    event: DragEvent<HTMLButtonElement>,
    object: InventoryObject,
  ) => {
    if (!isUsable(object)) {
      event.preventDefault();
      return;
    }
    const objectPath = joinObjectsDirPath(objectsDirPath, object.fileName);
    const displayName = displayObjectFileName(object.className);

    const payload = JSON.stringify({
      objectPath,
      name: displayName,
      className: object.className,
    });
    event.dataTransfer.setData("application/x-zkcraft-object", payload);
    event.dataTransfer.setData("text/plain", displayName);
    event.dataTransfer.setData("text", displayName);
    event.dataTransfer.effectAllowed = "copy";
    isDraggingRef.current = true;
  };

  const handleDragEnd = () => {
    isDraggingRef.current = false;
  };

  const handleClickObject = (objectId: string) => {
    if (isDraggingRef.current) return;
    onSelectObject(objectId);
  };

  const liveObjects = inventory.filter((object) => isLiveObject(object));
  const nullifiedObjects = inventory.filter((object) => !isLiveObject(object));

  const renderInventoryObject = (object: InventoryObject) => {
    const displayName = displayObjectFileName(object.className);
    const hashLineRaw = isLiveObject(object)
      ? object.id
      : object.status;
    const hashLine = truncateDisplayHash(hashLineRaw);
    return (
      <button
        key={object.id}
        type="button"
        className={`inventory-item ${activeObjectId === object.id ? "active" : ""}`}
        onClick={() => handleClickObject(object.id)}
        draggable={isUsable(object)}
        onDragStart={(event) => handleDragStart(event, object)}
        onDragEnd={handleDragEnd}
      >
        <span className="inventory-file-icon">
          <span className="inventory-emoji">{object.emoji}</span>
        </span>
        <span className="inventory-main">
          <span className="inventory-name">{displayName}</span>
          <span className="inventory-hash" title={hashLineRaw}>
            {hashLine}
          </span>
        </span>
        <span
          className={`inventory-dot ${!isLiveObject(object) ? "nullified" : object.grounded ? "live" : "pending"}`}
          title={isLiveObject(object) && !object.grounded ? "pending confirmation" : undefined}
        />
      </button>
    );
  };

  return (
    <section className="inventory-panel">
      <button
        type="button"
        className="panel-header panel-header-button"
        onClick={onOpenObjectsDir}
        title={displayPathInObjectsDir(objectsDirPath, objectsDirPath)}
      >
        Your Objects
      </button>

      <div className="inventory-list">
        {liveObjects.map(renderInventoryObject)}

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
            {showNullifiedItems && nullifiedObjects.map(renderInventoryObject)}
          </div>
        )}
      </div>
    </section>
  );
}
