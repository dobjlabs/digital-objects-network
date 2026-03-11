import { useRef } from "react";
import type { InventoryItem } from "../../shared/types/domain";
import type { DragEvent } from "react";
import { objectDisplayFileName } from "../../shared/objectDisplay";

interface InventoryPanelProps {
  items: InventoryItem[];
  objectsDirPath: string;
  activeItemId: string | null;
  showNullifiedItems: boolean;
  onSelectItem: (itemId: string) => void;
  onToggleNullified: () => void;
  onOpenObjectsDir: () => void;
}

export function InventoryPanel({
  items,
  objectsDirPath,
  activeItemId,
  showNullifiedItems,
  onSelectItem,
  onToggleNullified,
  onOpenObjectsDir,
}: InventoryPanelProps) {
  const isDraggingRef = useRef(false);
  const isLive = (item: InventoryItem) => item.nullifier == null;

  const truncateDisplayHash = (value: string) => {
    const trimmed = value.trim();
    if (!/^0x[0-9a-f]+$/i.test(trimmed)) return trimmed;
    if (trimmed.length <= 14) return trimmed;
    return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
  };

  const handleDragStart = (
    event: DragEvent<HTMLButtonElement>,
    item: InventoryItem,
  ) => {
    if (!isLive(item)) {
      event.preventDefault();
      return;
    }
    const basePath = objectsDirPath.endsWith("/")
      ? objectsDirPath.slice(0, -1)
      : objectsDirPath;
    const objectPath = `${basePath}/${item.fileName}`;
    const displayName = objectDisplayFileName(item);

    const payload = JSON.stringify({
      objectPath,
      name: displayName,
      className: item.classMeta.name,
      classHash: item.classMeta.hash,
    });
    event.dataTransfer.setData("application/x-zkcraft-item", payload);
    event.dataTransfer.setData("text/plain", displayName);
    event.dataTransfer.setData("text", displayName);
    event.dataTransfer.effectAllowed = "copy";
    isDraggingRef.current = true;
  };

  const handleDragEnd = () => {
    isDraggingRef.current = false;
  };

  const handleClickItem = (itemId: string) => {
    if (isDraggingRef.current) return;
    onSelectItem(itemId);
  };

  const liveItems = items.filter((item) => isLive(item));
  const nullifiedItems = items.filter((item) => !isLive(item));

  const renderInventoryItem = (item: InventoryItem) => {
    const displayName = objectDisplayFileName(item);
    const hashLineRaw = isLive(item)
      ? item.id
      : (item.nullifier ?? "nullified");
    const hashLine = truncateDisplayHash(hashLineRaw);
    return (
      <button
        key={item.id}
        type="button"
        className={`inventory-item ${activeItemId === item.id ? "active" : ""}`}
        onClick={() => handleClickItem(item.id)}
        draggable={isLive(item)}
        onDragStart={(event) => handleDragStart(event, item)}
        onDragEnd={handleDragEnd}
      >
        <span className="inventory-file-icon">
          <span className="inventory-emoji">{item.emoji}</span>
        </span>
        <span className="inventory-main">
          <span className="inventory-name">{displayName}</span>
          <span className="inventory-hash" title={hashLineRaw}>
            {hashLine}
          </span>
        </span>
        <span
          className={`inventory-dot ${isLive(item) ? "live" : "nullified"}`}
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
        {liveItems.map(renderInventoryItem)}

        {nullifiedItems.length > 0 && (
          <div className="nullified-section">
            <button
              type="button"
              className="nullified-toggle"
              onClick={onToggleNullified}
            >
              <span className="nullified-label">nullified</span>
              <span className="nullified-count">
                {showNullifiedItems ? "▴" : "▾"} {nullifiedItems.length}
              </span>
            </button>
            {showNullifiedItems && nullifiedItems.map(renderInventoryItem)}
          </div>
        )}
      </div>
    </section>
  );
}
