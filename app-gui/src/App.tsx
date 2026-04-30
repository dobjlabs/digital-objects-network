import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listenOpenSettings } from "./shared/api/tauriClient";
import { SettingsModal } from "./features/settings/SettingsModal";
import "./styles/tokens.css";
import "./features/settings/SettingsModal.css";
import { Equipment } from "./bitcraft/components/Equipment";
import { Leaderboard } from "./bitcraft/components/Leaderboard";
import { RecipeRow } from "./bitcraft/components/RecipeRow";
import { Stockpile } from "./bitcraft/components/Stockpile";
import { CAT_LABEL, RECIPES } from "./bitcraft/data";
import { useInventory } from "./bitcraft/hooks/useInventory";
import { useJobQueue } from "./bitcraft/hooks/useJobQueue";
import { currentLevel, invCount, isUnlocked } from "./bitcraft/sim";

export default function App() {
  const { inv, inventoryRef, loading, refresh } = useInventory();
  const { jobs, busy, now, startJob } = useJobQueue(inventoryRef, refresh);

  const [tab, setTab] = useState<string>("all");
  const [presentedScore, setPresentedScore] = useState<number | null>(null);
  const [presentFlash, setPresentFlash] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  // Sticky-recipe highlight: once a recipe becomes unlockable we keep showing
  // it even if the inputs deplete, and we flash it for ~3s on first appearance.
  const firstSeenRef = useRef<Record<string, number>>(
    Object.fromEntries(
      RECIPES.filter((r) => r.cat === "mine" || r.cat === "farm").map((r) => [
        r.id,
        0,
      ]),
    ),
  );

  const lvl = useMemo(() => currentLevel(inv), [inv]);
  const unlocked = useMemo(
    () =>
      RECIPES.filter(
        (r) => isUnlocked(r, lvl, inv) || r.id in firstSeenRef.current,
      ),
    [lvl, inv],
  );
  const cats = useMemo(
    () => Array.from(new Set(unlocked.map((r) => r.cat))),
    [unlocked],
  );
  const shown = useMemo(
    () => (tab === "all" ? unlocked : unlocked.filter((r) => r.cat === tab)),
    [tab, unlocked],
  );

  // Stamp newly-unlocked recipes for the highlight fade.
  useEffect(() => {
    const t = Date.now();
    unlocked.forEach((r) => {
      if (!(r.id in firstSeenRef.current)) firstSeenRef.current[r.id] = t;
    });
  }, [unlocked]);

  const research = invCount(inv, "rocket");

  const handlePresent = useCallback(() => {
    setPresentedScore(invCount(inv, "rocket"));
    setPresentFlash(true);
    setTimeout(() => setPresentFlash(false), 200);
  }, [inv]);

  // Tauri menu → "Preferences" opens the settings modal.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    listenOpenSettings(() => {
      if (!cancelled) setSettingsOpen(true);
    })
      .then((dispose) => {
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((err) => console.error("listenOpenSettings failed:", err));
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  return (
    <>
      <div
        style={{
          fontFamily: "system-ui,sans-serif",
          fontSize: 13,
          padding: 12,
          background: "#fff",
          minHeight: "100vh",
        }}
      >
        <style>{`@keyframes bc-pulse{0%,100%{opacity:0.3}50%{opacity:1}}`}</style>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 12,
            marginBottom: 12,
            borderBottom: "1px solid #e8e8e8",
            paddingBottom: 8,
            flexWrap: "wrap",
          }}
        >
          <span style={{ fontWeight: "bold" }}>Bitcraft v0.1</span>
          {research > 0 && (
            <span>
              rockets: <strong>{research}</strong>
            </span>
          )}
          {loading && (
            <span style={{ fontSize: 11, color: "#bbb" }}>loading…</span>
          )}
          <span style={{ flex: 1 }} />
        </div>

        <div
          style={{
            display: "grid",
            gridTemplateColumns: "1fr 200px",
            gap: 12,
            alignItems: "start",
          }}
        >
          <div>
            <div
              style={{ marginBottom: 8, display: "flex", gap: 4, flexWrap: "wrap" }}
            >
              {(["all", ...cats] as string[]).map((t) => (
                <button
                  key={t}
                  onClick={() => setTab(t)}
                  style={{
                    fontSize: 11,
                    padding: "2px 10px",
                    background: tab === t ? "#333" : "transparent",
                    color: tab === t ? "#fff" : "#666",
                    border: "1px solid #ccc",
                    cursor: "pointer",
                  }}
                >
                  {t === "all" ? "all" : (CAT_LABEL[t] ?? t)}
                </button>
              ))}
            </div>
            {shown.map((r) => (
              <RecipeRow
                key={r.id}
                recipe={r}
                jobs={jobs[r.id] ?? []}
                now={now}
                inv={inv}
                busy={busy}
                onStart={startJob}
                firstSeen={firstSeenRef.current[r.id] ?? 0}
              />
            ))}
          </div>

          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <Stockpile inv={inv} />
            <Equipment inv={inv} busy={busy} />
            <div>
              <div
                style={{
                  fontSize: 11,
                  color: "#aaa",
                  marginBottom: 4,
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                }}
              >
                <span>Leaderboard</span>
                <button
                  onClick={handlePresent}
                  style={{
                    fontSize: 11,
                    padding: "1px 8px",
                    background: presentFlash ? "#333" : "transparent",
                    color: presentFlash ? "#fff" : "#666",
                    border: "1px solid #ccc",
                    cursor: "pointer",
                  }}
                >
                  present
                </button>
              </div>
              <div style={{ border: "1px solid #e8e8e8", padding: 4 }}>
                <Leaderboard presentedScore={presentedScore} />
              </div>
            </div>
          </div>
        </div>
      </div>

      <SettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
      />
    </>
  );
}
