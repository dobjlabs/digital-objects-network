import { useRef } from "react";
import type { InventoryItem } from "../../shared/types/domain";
import type { DragEvent } from "react";

interface InventoryPanelProps {
  items: InventoryItem[];
  thingsDirPath: string;
  activeItemId: string | null;
  showNullifiedItems: boolean;
  onSelectItem: (itemId: string) => void;
  onToggleNullified: () => void;
  onOpenThingsDir: () => void;
}

export function InventoryPanel({
  items,
  thingsDirPath,
  activeItemId,
  showNullifiedItems,
  onSelectItem,
  onToggleNullified,
  onOpenThingsDir,
}: InventoryPanelProps) {
  const isDraggingRef = useRef(false);

  const handleDragStart = (
    event: DragEvent<HTMLButtonElement>,
    item: InventoryItem,
  ) => {
    if (item.validity !== "live") {
      event.preventDefault();
      return;
    }
    const payload = JSON.stringify({
      itemId: item.id,
      name: item.fileName,
      className: item.classMeta.name,
      classHash: item.classMeta.hash,
    });
    event.dataTransfer.setData("application/x-zkcraft-item", payload);
    event.dataTransfer.setData("text/plain", item.fileName);
    event.dataTransfer.setData("text", item.fileName);
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

  const liveItems = items.filter((item) => item.validity === "live");
  const nullifiedItems = items.filter((item) => item.validity === "nullified");

  const renderInventoryItem = (item: InventoryItem) => {
    const hashLine =
      item.validity === "live"
        ? item.stateRoot
        : (item.nullifier ?? "nullified");
    return (
      <button
        key={item.id}
        type="button"
        className={`inventory-item ${activeItemId === item.id ? "active" : ""}`}
        onClick={() => handleClickItem(item.id)}
        draggable={item.validity === "live"}
        onDragStart={(event) => handleDragStart(event, item)}
        onDragEnd={handleDragEnd}
      >
        <span className="inventory-file-icon">
          <span className="inventory-emoji">{item.emoji}</span>
        </span>
        <span className="inventory-main">
          <span className="inventory-name">{item.fileName}</span>
          <span className="inventory-hash">{hashLine}</span>
        </span>
        <span
          className={`inventory-dot ${item.validity === "live" ? "live" : "nullified"}`}
        />
      </button>
    );
  };

  return (
    <section className="inventory-panel">
      <button
        type="button"
        className="panel-header panel-header-button"
        onClick={onOpenThingsDir}
        title={thingsDirPath}
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
