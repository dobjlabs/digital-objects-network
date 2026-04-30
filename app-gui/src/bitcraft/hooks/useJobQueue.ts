import { useCallback, useEffect, useRef, useState } from "react";
import { runAction, type InventoryObjectPayload } from "../../shared/api/tauriClient";
import { pickInputFiles } from "../adapter";
import { RECIPE_TO_ACTION, RM } from "../data";
import { type JobInst, nextJid, resolveDur } from "../sim";

export type JobsMap = Record<string, JobInst[]>;

export interface JobQueueState {
  jobs: JobsMap;
  /** Frontend-only station busy lock. Backend doesn't enforce — this just
   *  prevents the user from double-queuing a station-bound recipe. */
  busy: Record<string, boolean>;
  /** Monotonic timestamp updated every 100ms — drives bar animations. */
  now: number;
  startJob: (recipeId: string, qty: number) => void;
}

export function useJobQueue(
  inventoryRef: React.RefObject<InventoryObjectPayload[]>,
  refresh: () => Promise<InventoryObjectPayload[]>,
): JobQueueState {
  const [jobs, setJobs] = useState<JobsMap>({});
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [now, setNow] = useState(Date.now());

  // 100ms tick for the segmented bar animation.
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(id);
  }, []);

  // Stable refs so the async startJob loop stays decoupled from re-renders.
  const refreshRef = useRef(refresh);
  useEffect(() => {
    refreshRef.current = refresh;
  }, [refresh]);

  const startJob = useCallback(
    (recipeId: string, qty: number) => {
      const recipe = RM[recipeId];
      if (!recipe) {
        console.warn(`startJob: unknown recipe ${recipeId}`);
        return;
      }
      const actionName = RECIPE_TO_ACTION[recipeId];
      if (!actionName) {
        console.warn(`startJob: no backend action mapped for ${recipeId}`);
        return;
      }

      // Station-bound recipes always serialize to qty=1 (only one physical
      // station). Mirrors the spec's behavior.
      const actualQty = recipe.station ? 1 : qty;

      // Lock station and seed N queued instances up front so the UI shows
      // the queue depth.
      if (recipe.station) {
        setBusy((s) => ({ ...s, [recipe.station as string]: true }));
      }

      const seeded: JobInst[] = Array.from({ length: actualQty }, () => ({
        jid: nextJid(),
        start: 0, // 0 = queued (not yet running). Set to Date.now() when fired.
        dur: resolveDur(recipe),
      }));
      setJobs((j) => ({
        ...j,
        [recipeId]: [...(j[recipeId] ?? []), ...seeded],
      }));

      // Fire sequentially. Each runAction is awaited before the next pick;
      // the inventory is refreshed in between so picks see consumed files
      // disappear.
      void (async () => {
        for (const inst of seeded) {
          const live = inventoryRef.current ?? [];
          const inputFiles = pickInputFiles(recipe, live);
          if (!inputFiles) {
            console.warn(
              `startJob: inventory drained mid-batch for ${recipeId}, dropping remaining`,
            );
            // Remove this and all subsequent unstarted instances.
            const dropFrom = seeded.indexOf(inst);
            const drop = new Set(seeded.slice(dropFrom).map((x) => x.jid));
            setJobs((j) => ({
              ...j,
              [recipeId]: (j[recipeId] ?? []).filter((x) => !drop.has(x.jid)),
            }));
            break;
          }

          // Flip from queued → running.
          setJobs((j) => ({
            ...j,
            [recipeId]: (j[recipeId] ?? []).map((x) =>
              x.jid === inst.jid ? { ...x, start: Date.now() } : x,
            ),
          }));

          try {
            await runAction({
              actionId: actionName,
              inputObjectPaths: inputFiles,
            });
          } catch (err) {
            console.error(`run_action ${actionName} failed:`, err);
            // Drop this and remaining queued instances on first failure.
            const dropFrom = seeded.indexOf(inst);
            const drop = new Set(seeded.slice(dropFrom).map((x) => x.jid));
            setJobs((j) => ({
              ...j,
              [recipeId]: (j[recipeId] ?? []).filter((x) => !drop.has(x.jid)),
            }));
            break;
          }

          // Remove the just-completed instance.
          setJobs((j) => ({
            ...j,
            [recipeId]: (j[recipeId] ?? []).filter((x) => x.jid !== inst.jid),
          }));

          // Refresh before the next pick so consumed files vanish from the pool.
          await refreshRef.current();
        }

        if (recipe.station) {
          setBusy((s) => ({ ...s, [recipe.station as string]: false }));
        }
      })();
    },
    [inventoryRef],
  );

  return { jobs, busy, now, startJob };
}
