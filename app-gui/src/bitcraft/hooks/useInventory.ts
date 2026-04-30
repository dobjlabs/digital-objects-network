import { useCallback, useEffect, useRef, useState } from "react";
import {
  listenObjectsChanged,
  loadGuiInventory,
  type ActionPayload,
  type InventoryObjectPayload,
} from "../../shared/api/tauriClient";
import { buildInvFromBackend } from "../adapter";
import type { Inv } from "../sim";

export interface InventoryState {
  inventory: InventoryObjectPayload[];
  actions: ActionPayload[];
  inv: Inv;
  loading: boolean;
  /** Force-refresh from the backend; resolves with the new inventory list. */
  refresh: () => Promise<InventoryObjectPayload[]>;
  /** Live ref so async loops (the job queue) can read current inventory. */
  inventoryRef: React.RefObject<InventoryObjectPayload[]>;
}

export function useInventory(): InventoryState {
  const [inventory, setInventory] = useState<InventoryObjectPayload[]>([]);
  const [actions, setActions] = useState<ActionPayload[]>([]);
  const [inv, setInv] = useState<Inv>({});
  const [loading, setLoading] = useState(true);
  const inventoryRef = useRef<InventoryObjectPayload[]>([]);

  const refresh = useCallback(async (): Promise<InventoryObjectPayload[]> => {
    try {
      const result = await loadGuiInventory();
      setInventory(result.inventory);
      setActions(result.actions);
      setInv(buildInvFromBackend(result.inventory));
      inventoryRef.current = result.inventory;
      return result.inventory;
    } catch (err) {
      console.error("loadGuiInventory failed:", err);
      return inventoryRef.current;
    } finally {
      setLoading(false);
    }
  }, []);

  // Initial load
  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Watch for filesystem changes (driver writes new .dobj files; relayer/sync
  // confirmations flip statuses).
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    let pending: number | null = null;

    const scheduleRefresh = () => {
      if (cancelled) return;
      if (pending !== null) window.clearTimeout(pending);
      pending = window.setTimeout(() => {
        pending = null;
        if (!cancelled) void refresh();
      }, 120);
    };

    listenObjectsChanged(scheduleRefresh)
      .then((dispose) => {
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((err) => {
        console.error("listenObjectsChanged failed:", err);
      });

    return () => {
      cancelled = true;
      if (pending !== null) window.clearTimeout(pending);
      if (unlisten) unlisten();
    };
  }, [refresh]);

  return { inventory, actions, inv, loading, refresh, inventoryRef };
}
