import { useLayoutEffect, useRef, useState } from "react";
import { truncateDisplayHash } from "../../shared/format";
import { qualifiedEq } from "../../shared/objectUtils";
import { useStore } from "../../shared/state/store";

export function ProofRunnerPanel() {
  const proof = useStore((state) => state.proof);
  const contextSelection = useStore((state) => state.contextSelection);
  const selectAction = useStore((state) => state.selectAction);
  const prevStatusRef = useRef(proof.status);
  const [idleFadeIn, setIdleFadeIn] = useState(false);

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

  const stateRootRaw = proof.stats.stateRoot?.trim() ?? "";
  const globalRootDisplay = stateRootRaw
    ? truncateDisplayHash(stateRootRaw)
    : "0x----...----";

  const canReturnToAction =
    proof.action !== null &&
    (proof.status === "generating" ||
      proof.status === "committing" ||
      proof.status === "summary");
  const alreadyViewingRunningAction =
    proof.action !== null &&
    contextSelection.kind === "action" &&
    qualifiedEq(contextSelection.action, proof.action);

  const returnToRunningAction = () => {
    if (!proof.action) return;
    selectAction(proof.action);
  };

  const controlsRow = (
    <div className="proof-jump-row proof-controls-row">
      {canReturnToAction && (
        <button
          type="button"
          className="proof-jump-btn"
          onClick={returnToRunningAction}
          disabled={alreadyViewingRunningAction}
        >
          {alreadyViewingRunningAction ? "Viewing Action" : "Return to Action"}
        </button>
      )}
    </div>
  );

  if (proof.status === "idle") {
    return (
      <section className={`proof-panel-frame proof-panel proof-panel-idle${idleFadeIn ? " idle-fade-in" : ""}`}>
        <div className="proof-title">Ready to run an action</div>
        <div className="idle-section idle-roots">
          <div className="root-row">
            <span className="root-row-left">
              <span className="root-dot live" />
              <span className="root-label">Global State Root</span>
            </span>
            <span className="root-hash" title={stateRootRaw || undefined}>
              {globalRootDisplay}
            </span>
          </div>
        </div>
      </section>
    );
  }

  if (proof.status === "error") {
    return (
      <section className="proof-panel-frame proof-panel">
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
      <section className="proof-panel-frame proof-panel proof-run-card">
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
                {generateProofStep?.detail ?? "..."}
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
      <section className="proof-panel-frame proof-panel proof-summary-card">
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
