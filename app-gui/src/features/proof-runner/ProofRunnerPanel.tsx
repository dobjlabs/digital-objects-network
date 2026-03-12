import { useLayoutEffect, useRef, useState } from "react";
import { truncateDisplayHash } from "../../shared/format";
import { useStore } from "../../shared/state/store";

export function ProofRunnerPanel() {
  const proof = useStore((state) => state.proof);
  const contextSelection = useStore((state) => state.contextSelection);
  const selectAction = useStore((state) => state.selectAction);
  const prevStatusRef = useRef(proof.status);
  const [idleFadeIn, setIdleFadeIn] = useState(false);
  const [showCpuDuringRun, setShowCpuDuringRun] = useState(false);

  useLayoutEffect(() => {
    const prev = prevStatusRef.current;
    if (proof.status === "idle" && prev === "summary") {
      setIdleFadeIn(true);
      const timer = window.setTimeout(() => setIdleFadeIn(false), 420);
      prevStatusRef.current = proof.status;
      return () => window.clearTimeout(timer);
    }
    prevStatusRef.current = proof.status;
    return undefined;
  }, [proof.status]);

  const runActive =
    proof.status === "generating" ||
    proof.status === "committing" ||
    proof.status === "summary";

  useLayoutEffect(() => {
    if (!runActive) {
      setShowCpuDuringRun(false);
    }
  }, [runActive, proof.runActionId]);

  const globalRootRaw = proof.stats.globalStateRoot?.trim() ?? "";
  const globalRootDisplay = globalRootRaw
    ? truncateDisplayHash(globalRootRaw)
    : "0x----...----";

  const formatCpuDuration = (totalSecs: number) => {
    const secs = Math.max(0, Math.floor(totalSecs));
    const hours = Math.floor(secs / 3600);
    const minutes = Math.floor((secs % 3600) / 60);
    const seconds = secs % 60;
    if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
    if (minutes > 0) return `${minutes}m ${seconds}s`;
    return `${seconds}s`;
  };

  const canReturnToAction =
    !!proof.runActionId &&
    (proof.status === "generating" ||
      proof.status === "committing" ||
      proof.status === "summary");
  const alreadyViewingRunningAction =
    proof.runActionId !== null &&
    contextSelection.kind === "action" &&
    contextSelection.actionId === proof.runActionId;

  const returnToRunningAction = () => {
    if (!proof.runActionId) return;
    selectAction(proof.runActionId);
  };

  const toggleProofPanelView = () => {
    setShowCpuDuringRun((current) => !current);
  };

  const controlsRow = (
    <div className="proof-jump-row proof-controls-row">
      <button
        type="button"
        className="proof-jump-btn"
        onClick={toggleProofPanelView}
        title={showCpuDuringRun ? "Show action details" : "Show CPU chart"}
      >
        {showCpuDuringRun ? "Show Action" : "Show CPU Chart"}
      </button>
      {canReturnToAction && (
        <button
          type="button"
          className="proof-jump-btn"
          onClick={returnToRunningAction}
          disabled={alreadyViewingRunningAction}
          title={
            proof.runActionId
              ? `Open ${proof.runActionId}`
              : "Open running action"
          }
        >
          {alreadyViewingRunningAction ? "Viewing Action" : "Return to Action"}
        </button>
      )}
    </div>
  );

  const showCpuPanel =
    proof.status === "idle" || (runActive && showCpuDuringRun);
  const idlePanelClass =
    proof.status === "idle" && idleFadeIn ? " idle-fade-in" : "";

  if (showCpuPanel) {
    return (
      <section className={`cpu-panel proof-panel proof-panel-idle${idlePanelClass}`}>
        {runActive && showCpuDuringRun ? controlsRow : null}
        <div className="idle-section idle-cpu">
          <div className="proof-title cpu-title">CPU Usage</div>
          <div className="dash-cpu-bars">
            {proof.stats.cpuHistory.map((value, index) => (
              <div
                key={`${index}-${value}`}
                className="dash-cpu-bar"
                style={{
                  height: `${Math.max(4, Math.min(100, Math.round(value)))}%`,
                }}
              />
            ))}
          </div>
          <div className="proof-line cpu-total">
            Total:{" "}
            <span className="proof-muted">
              {formatCpuDuration(proof.stats.totalCpuSecs)}
            </span>
          </div>
        </div>
        <div className="idle-section idle-roots">
          <div className="root-row">
            <span className="root-row-left">
              <span className="root-dot live" />
              <span className="root-label">Global Valid State Roots</span>
            </span>
            <span className="root-hash" title={globalRootRaw || undefined}>
              {globalRootDisplay}
            </span>
          </div>
          <div className="root-row">
            <span className="root-row-left">
              <span className="root-dot nullified" />
              <span className="root-label">Global Nullified State Roots</span>
            </span>
            <span className="root-hash" title={globalRootRaw || undefined}>
              {globalRootDisplay}
            </span>
          </div>
        </div>
      </section>
    );
  }

  if (proof.status === "error") {
    return (
      <section className="cpu-panel proof-panel">
        <div className="proof-title">Proof Failed</div>
        <div className="proof-error">{proof.error}</div>
      </section>
    );
  }

  if (proof.status === "generating" || proof.status === "committing") {
    const generateProofStep = proof.steps.find(
      (step) => step.id === "generate-proof",
    );
    const commitStep = proof.steps.find((step) => step.id === "commit");

    const statusClass = (status: "pending" | "running" | "done") =>
      status === "done" ? "done" : status === "running" ? "running" : "pending";
    const stageClass = (status: "pending" | "running" | "done") =>
      status === "done" ? "done" : status === "running" ? "active" : "pending";
    const stageHeaderClass = (status: "pending" | "running" | "done") =>
      status === "pending" ? "stage-header pending" : "stage-header";

    const generateState: "pending" | "running" | "done" =
      generateProofStep?.status ??
      (proof.status === "generating" ? "running" : "pending");
    const commitState: "pending" | "running" | "done" =
      commitStep?.status ??
      (proof.status === "committing" ? "running" : "pending");

    return (
      <section className="cpu-panel proof-panel proof-run-card">
        {controlsRow}
        <div className={stageHeaderClass(generateState)}>
          <span className={`stage-num ${stageClass(generateState)}`}>1</span>
          <span className="stage-title">Generate Proof</span>
        </div>

        {proof.status === "generating" && (
          <div className="stage-details">
            <div className="stage-detail-line">
              <span
                className={`stage-detail-value ${statusClass(generateState)}`}
              >
                {generateProofStep?.detail ?? proof.cpuCost ?? "..."}
              </span>
            </div>
          </div>
        )}

        <div className={stageHeaderClass(commitState)}>
          <span className={`stage-num ${stageClass(commitState)}`}>2</span>
          <span className="stage-title">Commit</span>
        </div>

        {proof.status === "committing" && (
          <div className="stage-details">
            <div className="stage-detail-line">
              <span
                className={`stage-detail-value ${statusClass(commitState)}`}
              >
                {commitState === "pending"
                  ? "—"
                  : (commitStep?.detail ?? proof.newRoot ?? "pending")}
              </span>
            </div>
          </div>
        )}
      </section>
    );
  }

  if (proof.status === "summary") {
    const nullified = proof.summary?.nullified ?? [];
    const live = proof.summary?.live ?? [];

    return (
      <section className="cpu-panel proof-panel proof-summary-card">
        {controlsRow}
        <div className="summary-stage">
          <div className="summary-title">
            <span className="stage-num summary-danger">✗</span>
            Nullified
          </div>
          {nullified.length === 0 ? (
            <div className="summary-line summary-muted">none</div>
          ) : (
            nullified.map((entry, idx) => (
              <div
                key={`${entry}-${idx}`}
                className="summary-line summary-null"
              >
                {entry}
              </div>
            ))
          )}
        </div>
        <div className="summary-stage">
          <div className="summary-title">
            <span className="stage-num done">✓</span>
            Live
          </div>
          {live.map((entry, idx) => (
            <div key={`${entry}-${idx}`} className="summary-line summary-live">
              {entry}
            </div>
          ))}
        </div>
      </section>
    );
  }

  return null;
}
