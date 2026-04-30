import { CONFIG, label, LEVEL_ORDER, type Recipe } from "../data";
import {
  currentLevel,
  invBusy,
  invCount,
  jobPhase,
  maxBatch,
  usesAvailable,
  type Inv,
  type JobInst,
} from "../sim";
import { SegmentedBar } from "./SegmentedBar";

const HIGHLIGHT_HOLD = 1500;
const HIGHLIGHT_FADE = 2000;

interface Props {
  recipe: Recipe;
  jobs: JobInst[];
  now: number;
  inv: Inv;
  /** Frontend-only station busy lock from useJobQueue. */
  busy: Record<string, boolean>;
  onStart: (recipeId: string, qty: number) => void;
  firstSeen: number;
}

export function RecipeRow({
  recipe,
  jobs,
  now,
  inv,
  busy,
  onStart,
  firstSeen,
}: Props) {
  const isMine = recipe.cat === "mine" || recipe.cat === "farm";

  const running = jobs.filter((j) => j.start !== 0);
  const queued = jobs.filter((j) => j.start === 0);

  const stationLockedClient = recipe.station ? busy[recipe.station] === true : false;
  const stationBusy = recipe.station
    ? invBusy(inv, recipe.station) || stationLockedClient
    : false;
  const stationMissing =
    recipe.station != null && invCount(inv, recipe.station) === 0;
  const lvl = currentLevel(inv);
  const levelMissing =
    !!recipe.level && LEVEL_ORDER[lvl] < LEVEL_ORDER[recipe.level];

  const age = firstSeen ? now - firstSeen : Number.POSITIVE_INFINITY;
  const alpha =
    age < HIGHLIGHT_HOLD
      ? 1
      : age < HIGHLIGHT_HOLD + HIGHLIGHT_FADE
        ? 1 - (age - HIGHLIGHT_HOLD) / HIGHLIGHT_FADE
        : 0;

  const affordable = isMine ? Number.POSITIVE_INFINITY : maxBatch(inv, recipe.inp);
  const wearsCap = usesAvailable(inv, recipe.uses);
  const canAffordQty = (qty: number): boolean =>
    !stationBusy &&
    !stationMissing &&
    !levelMissing &&
    (isMine || affordable >= qty) &&
    qty <= wearsCap &&
    (!recipe.station || qty === 1);

  const outStr = Object.entries(recipe.out)
    .map(([k, v]) => `+${v} ${label(k)}`)
    .join(", ");

  const bs = {
    fontSize: 10,
    padding: "2px 8px",
    cursor: "pointer",
    background: "transparent",
    border: "1px solid #ddd",
    color: "#888",
  } as const;

  // Requirement tags rendered inline with inputs.
  const reqTags: React.ReactNode[] = [];
  if (recipe.station) {
    const bad = stationBusy || stationMissing;
    reqTags.push(
      <span key="station" style={{ marginRight: 8, color: bad ? "#c00" : "#aaa" }}>
        {label(recipe.station)}
        {stationBusy ? " (busy)" : stationMissing ? " (missing)" : ""}
      </span>,
    );
  }
  if (recipe.level) {
    const have = LEVEL_ORDER[currentLevel(inv)] >= LEVEL_ORDER[recipe.level];
    reqTags.push(
      <span key="level" style={{ marginRight: 8, color: have ? "#aaa" : "#c00" }}>
        {recipe.level === "machine_1" ? "Machine I" : "Machine II"}
      </span>,
    );
  }
  if (recipe.uses) {
    const uses = usesAvailable(inv, recipe.uses);
    const bad = uses === 0;
    reqTags.push(
      <span key="uses" style={{ marginRight: 8, color: bad ? "#c00" : "#aaa" }}>
        {label(recipe.uses)}
        <span style={{ fontSize: 10, color: bad ? "#e55" : "#ccc" }}>({uses})</span>
      </span>,
    );
  }

  return (
    <div
      style={{
        padding: "6px 10px 4px",
        borderBottom: "1px solid #e8e8e8",
        background: `rgba(255,251,230,${alpha})`,
      }}
    >
      <div style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
        <span
          style={{
            flex: "0 0 140px",
            fontSize: 13,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
            color:
              running.length > 0
                ? "#555"
                : canAffordQty(1) || isMine
                  ? "#111"
                  : "#bbb",
          }}
        >
          {recipe.label}
          {running.length > 0 && (
            <span style={{ fontSize: 10, color: "#bbb", marginLeft: 4 }}>
              ×{running.length}
            </span>
          )}
          {queued.length > 0 && (
            <span style={{ fontSize: 10, color: "#bbb", marginLeft: 4 }}>
              (+{queued.length} queued)
            </span>
          )}
        </span>

        <span style={{ flex: 1, fontSize: 11, minWidth: 0 }}>
          {reqTags}
          {reqTags.length > 0 && Object.keys(recipe.inp).length > 0 && (
            <span style={{ marginRight: 8, color: "#ddd" }}>:</span>
          )}
          {Object.entries(recipe.inp).map(([k, v]) => {
            const have = invCount(inv, k);
            return (
              <span
                key={k}
                style={{ marginRight: 8, color: have >= v ? "#aaa" : "#c00" }}
              >
                {label(k)} ×{v}
                <span style={{ fontSize: 10, color: have >= v ? "#ccc" : "#e55" }}>
                  ({have})
                </span>
              </span>
            );
          })}
          {(Object.keys(recipe.inp).length > 0 ||
            isMine ||
            reqTags.length > 0) && (
            <span style={{ color: "#ddd" }}>→ {outStr}</span>
          )}
        </span>

        <span
          style={{
            flexShrink: 0,
            display: "flex",
            justifyContent: "flex-end",
            gap: 4,
          }}
        >
          {CONFIG.batchSizes.map((qty) => {
            const ok = canAffordQty(qty);
            return (
              <button
                key={qty}
                onClick={() => ok && onStart(recipe.id, qty)}
                style={{
                  ...bs,
                  opacity: ok ? 1 : 0.25,
                  cursor: ok ? "pointer" : "default",
                }}
              >
                ×{qty}
              </button>
            );
          })}
        </span>
      </div>

      {/* One progress bar per running instance. */}
      {running.map((inst) => {
        const phase = jobPhase(recipe, inst, now);
        return (
          <div
            key={inst.jid}
            style={{ display: "flex", alignItems: "center", gap: 4, marginTop: 4 }}
          >
            <div style={{ flex: 1 }}>
              <SegmentedBar recipe={recipe} job={inst} now={now} />
            </div>
            <span
              style={{
                fontSize: 10,
                color: phase.color,
                fontFamily: "monospace",
                whiteSpace: "nowrap",
                minWidth: 150,
                textAlign: "right",
                flexShrink: 0,
              }}
            >
              {phase.label}
            </span>
          </div>
        );
      })}
    </div>
  );
}
