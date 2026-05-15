import { useMemo, useState } from "react";
import type {
  ActionPayload as Action,
  InventoryObjectPayload as InventoryObject,
  QualifiedNamePayload,
} from "../../shared/api/wireTypes";
import { truncateDisplayHash } from "../../shared/format";
import { pluginScopedLabel, qualifiedEq, qualifiedId } from "../../shared/objectUtils";

interface ActionGridProps {
  actions: Action[];
  activeAction: QualifiedNamePayload | null;
  selectedObject: InventoryObject | null;
  onSelectAction: (action: QualifiedNamePayload) => void;
  onClearSelection: () => void;
}

export function ActionGrid({
  actions,
  activeAction,
  selectedObject,
  onSelectAction,
  onClearSelection,
}: ActionGridProps) {
  const [search, setSearch] = useState("");

  const compatibilityFiltered = useMemo(() => {
    if (!selectedObject) return actions;
    return actions.filter((action) =>
      action.totalInputs.some((ref) => qualifiedEq(ref.class, selectedObject.class)),
    );
  }, [actions, selectedObject]);

  const visibleActions = useMemo(() => {
    if (selectedObject) return compatibilityFiltered;
    const q = search.trim().toLowerCase();
    if (!q) return compatibilityFiltered;
    return compatibilityFiltered.filter((action) => {
      return (
        action.action.name.toLowerCase().includes(q) ||
        action.action.pluginName.toLowerCase().includes(q) ||
        action.description.toLowerCase().includes(q)
      );
    });
  }, [compatibilityFiltered, search, selectedObject]);

  const selectedObjectLabel = selectedObject
    ? pluginScopedLabel(selectedObject.class)
    : "";
  const filterLabel = selectedObject
    ? visibleActions.length > 0
      ? `accepts # ${selectedObjectLabel}`
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
        {visibleActions.map((action) => {
          const id = qualifiedId(action.action);
          const label = pluginScopedLabel(action.action);
          const isActive =
            activeAction !== null && qualifiedEq(activeAction, action.action);
          return (
            <button
              key={id}
              type="button"
              className={`action-row ${isActive ? "active" : ""}`}
              onClick={() => onSelectAction(action.action)}
            >
              <span className="action-row-emoji">{action.emoji}</span>
              <span className="action-row-name">{label}</span>
              <span className="action-row-hash" title={action.hash || "No hash"}>
                {truncateDisplayHash(action.hash)}
              </span>
            </button>
          );
        })}
        {visibleActions.length === 0 && (
          <div className="action-empty">No actions match.</div>
        )}
      </div>
    </section>
  );
}
