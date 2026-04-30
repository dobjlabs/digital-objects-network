import { BAR_AMBER, BAR_BLUE, BAR_GREY, BASE_MS, type Recipe } from "../data";
import type { JobInst } from "../sim";

interface Props {
  recipe: Recipe;
  job: JobInst | null;
  now: number;
}

/** Three-segment progress bar matching the spec: amber (PoW) → blue (VDF)
 *  → grey (proof base). The PoW segment pulses while it's the active phase. */
export function SegmentedBar({ recipe, job, now }: Props) {
  if (!job || job.start === 0) {
    return <div style={{ marginTop: 4, height: 4, background: "#f0f0f0" }} />;
  }

  const vdf_ms = recipe.vdf_ms || 0;
  const pow_actual = Math.max(job.dur - BASE_MS - vdf_ms, 0);
  const elapsed = now - job.start;

  const powW = (pow_actual / job.dur) * 100;
  const vdfW = (vdf_ms / job.dur) * 100;
  const baseW = (BASE_MS / job.dur) * 100;

  const powDone = elapsed >= pow_actual;
  const vdfProgress =
    vdf_ms > 0 ? Math.min(Math.max(elapsed - pow_actual, 0) / vdf_ms, 1) : 0;
  const baseProgress = Math.min(
    Math.max(elapsed - pow_actual - vdf_ms, 0) / BASE_MS,
    1,
  );

  const seg = (w: number, fill: string, scale: number, pulse: boolean) => (
    <div
      style={{
        width: `${w}%`,
        position: "relative",
        background: "#f0f0f0",
        flexShrink: 0,
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0,
          background: fill,
          transformOrigin: "left",
          transform: `scaleX(${scale})`,
          animation: pulse ? "bc-pulse 1.2s ease-in-out infinite" : "none",
        }}
      />
    </div>
  );

  return (
    <div style={{ marginTop: 4, height: 4, display: "flex", overflow: "hidden" }}>
      {pow_actual > 0 && seg(powW, BAR_AMBER, 1, !powDone)}
      {vdf_ms > 0 && seg(vdfW, BAR_BLUE, vdfProgress, false)}
      {seg(baseW, BAR_GREY, baseProgress, false)}
    </div>
  );
}
