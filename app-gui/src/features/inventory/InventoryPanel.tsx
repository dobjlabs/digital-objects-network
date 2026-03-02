import type { InventoryItem } from "../../shared/types/domain";

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
            onClick={() => onSelectItem(item.id)}
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
                  onClick={() => onSelectItem(item.id)}
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
