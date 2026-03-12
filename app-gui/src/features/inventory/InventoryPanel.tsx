import { useRef } from "react";
import type { DragEvent } from "react";
import type { InventoryObjectPayload as InventoryObject } from "../../shared/api/wireTypes";

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
  const isLive = (object: InventoryObject) => object.nullifier == null;
  const displayFileName = (className: string) => `${className}.dobj`;

  const truncateDisplayHash = (value: string) => {
    const trimmed = value.trim();
    if (!/^0x[0-9a-f]+$/i.test(trimmed)) return trimmed;
    if (trimmed.length <= 14) return trimmed;
    return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
  };

  const handleDragStart = (
    event: DragEvent<HTMLButtonElement>,
    object: InventoryObject,
  ) => {
    if (!isLive(object)) {
      event.preventDefault();
      return;
    }
    const basePath = objectsDirPath.endsWith("/")
      ? objectsDirPath.slice(0, -1)
      : objectsDirPath;
    const objectPath = `${basePath}/${object.fileName}`;
    const displayName = displayFileName(object.className);

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

  const liveObjects = inventory.filter((object) => isLive(object));
  const nullifiedObjects = inventory.filter((object) => !isLive(object));

  const renderInventoryObject = (object: InventoryObject) => {
    const displayName = displayFileName(object.className);
    const hashLineRaw = isLive(object)
      ? object.id
      : (object.nullifier ?? "nullified");
    const hashLine = truncateDisplayHash(hashLineRaw);
    return (
      <button
        key={object.id}
        type="button"
        className={`inventory-item ${activeObjectId === object.id ? "active" : ""}`}
        onClick={() => handleClickObject(object.id)}
        draggable={isLive(object)}
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
          className={`inventory-dot ${isLive(object) ? "live" : "nullified"}`}
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
        title={objectsDirPath}
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
