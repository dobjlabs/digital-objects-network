import { useMemo, useState } from "react";
import type {
  ActionPayload as Action,
  InventoryObjectPayload as InventoryObject,
} from "../../shared/api/wireTypes";

interface ActionGridProps {
  actions: Action[];
  activeActionId: string | null;
  selectedObject: InventoryObject | null;
  onSelectAction: (actionId: string) => void;
  onClearSelection: () => void;
}

export function ActionGrid({
  actions,
  activeActionId,
  selectedObject,
  onSelectAction,
  onClearSelection,
}: ActionGridProps) {
  const [search, setSearch] = useState("");

  const compatibilityFiltered = useMemo(() => {
    if (!selectedObject) return actions;
    return actions.filter((action) =>
      action.inputClasses.some(
        (className) => className === selectedObject.className,
      ),
    );
  }, [actions, selectedObject]);

  const visibleActions = useMemo(() => {
    if (selectedObject) return compatibilityFiltered;
    const q = search.trim().toLowerCase();
    if (!q) return compatibilityFiltered;
    return compatibilityFiltered.filter((action) => {
      return (
        action.id.toLowerCase().includes(q) ||
        action.description.toLowerCase().includes(q)
      );
    });
  }, [compatibilityFiltered, search, selectedObject]);

  const filterLabel = selectedObject
    ? visibleActions.length > 0
      ? `accepts # ${selectedObject.className}`
      : "no matching actions"
    : "";
  const tabLabel = selectedObject
    ? `Actions (${visibleActions.length}/${actions.length})`
    : "Actions";

  return (
    <section className="actions-panel">
      <div className="action-tab-row">
        <button type="button" className="action-tab-btn active">
          {tabLabel}
        </button>
      </div>
      <div className="action-toolbar">
        {!selectedObject ? (
          <input
            className="action-search"
            placeholder="search actions..."
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        ) : (
          <div className="action-filter-state">
            <span className="action-filter-pill">
              <span className="action-filter-icon">filter</span>
              <span className="action-filter-label">{filterLabel}</span>
            </span>
            <button
              type="button"
              className="action-clear-btn"
              onClick={onClearSelection}
              title="Clear selection"
            >
              x
            </button>
          </div>
        )}
      </div>
      <div className="action-list">
        {visibleActions.map((action) => (
          <button
            key={action.id}
            type="button"
            className={`action-row ${activeActionId === action.id ? "active" : ""}`}
            onClick={() => onSelectAction(action.id)}
            title={action.description}
          >
            <span className="action-row-emoji">{action.emoji}</span>
            <span className="action-row-name">{action.id}</span>
          </button>
        ))}
        {visibleActions.length === 0 && (
          <div className="action-empty">No actions match.</div>
        )}
      </div>
    </section>
  );
}
