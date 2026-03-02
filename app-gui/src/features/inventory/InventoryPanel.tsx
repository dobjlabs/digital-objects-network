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
  onDragItemStart: (itemName: string) => void;
}

export function InventoryPanel({
  items,
  thingsDirPath,
  activeItemId,
  showNullifiedItems,
  onSelectItem,
  onToggleNullified,
  onOpenThingsDir,
  onDragItemStart,
}: InventoryPanelProps) {
  const isDraggingRef = useRef(false);

  const handleDragStart = (event: DragEvent<HTMLButtonElement>, item: InventoryItem) => {
    const payload = JSON.stringify({ itemId: item.id, name: item.name });
    event.dataTransfer.setData("application/x-zkcraft-item", payload);
    event.dataTransfer.setData("text/plain", item.name);
    event.dataTransfer.setData("text", item.name);
    event.dataTransfer.effectAllowed = "copy";
    isDraggingRef.current = true;
    onDragItemStart(item.name);
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

  return (
    <section className="inventory-panel">
      <button
        type="button"
        className="panel-header panel-header-button"
        onClick={onOpenThingsDir}
        title={thingsDirPath}
      >
        Your Things
      </button>

      <div className="inventory-list">
        {liveItems.map((item) => (
          <button
            key={item.id}
            type="button"
            className={`inventory-item ${activeItemId === item.id ? "active" : ""}`}
            onClick={() => handleClickItem(item.id)}
            draggable
            onDragStart={(event) => handleDragStart(event, item)}
            onDragEnd={handleDragEnd}
          >
            <span>{item.emoji}</span>
            <span>{item.name}</span>
          </button>
        ))}

        {nullifiedItems.length > 0 && (
          <div className="nullified-section">
            <button type="button" className="nullified-toggle" onClick={onToggleNullified}>
              {showNullifiedItems ? "▴" : "▾"} Nullified ({nullifiedItems.length})
            </button>
            {showNullifiedItems &&
              nullifiedItems.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  className={`inventory-item ${activeItemId === item.id ? "active" : ""}`}
                  onClick={() => handleClickItem(item.id)}
                  draggable
                  onDragStart={(event) => handleDragStart(event, item)}
                  onDragEnd={handleDragEnd}
                >
                  <span>{item.emoji}</span>
                  <span>{item.name}</span>
                </button>
              ))}
          </div>
        )}
      </div>
    </section>
  );
}
