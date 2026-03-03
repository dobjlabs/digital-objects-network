import { useEffect, useRef, useState } from "react";
import { useUiStore } from "../../shared/state/uiStore";

export function ProofRunnerPanel() {
  const proof = useUiStore((state) => state.proof);
  const prevStatusRef = useRef(proof.status);
  const [idleFadeIn, setIdleFadeIn] = useState(false);

  useEffect(() => {
    const prev = prevStatusRef.current;
    if (proof.status === "idle" && prev === "done") {
      setIdleFadeIn(true);
      const timer = window.setTimeout(() => setIdleFadeIn(false), 420);
      prevStatusRef.current = proof.status;
      return () => window.clearTimeout(timer);
    }
    prevStatusRef.current = proof.status;
    return undefined;
  }, [proof.status]);

  const liveRoots = proof.stats.roots.filter((root) => root.state === "live");
  const nullifiedRoots = proof.stats.roots.filter(
    (root) => root.state === "nullified",
  );

  const aggregateHash = (roots: Array<{ hash: string }>) => {
    if (roots.length === 0) return "0x----...----";
    const first = roots
      .map((root) => root.hash.slice(2, 6))
      .join("")
      .slice(0, 4);
    const last = roots
      .map((root) => root.hash.slice(-4))
      .join("")
      .slice(-4);
    return `0x${first}...${last}`;
  };

  const formatCpuDuration = (totalSecs: number) => {
    const secs = Math.max(0, Math.floor(totalSecs));
    const hours = Math.floor(secs / 3600);
    const minutes = Math.floor((secs % 3600) / 60);
    const seconds = secs % 60;
    if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
    if (minutes > 0) return `${minutes}m ${seconds}s`;
    return `${seconds}s`;
  };

  if (proof.status === "idle") {
    const maxCpu = Math.max(...proof.stats.cpuHistory, 1);
    return (
      <section
        className={`cpu-panel proof-panel proof-panel-idle ${idleFadeIn ? "idle-fade-in" : ""}`}
      >
        <div className="idle-section idle-cpu">
          <div className="proof-title cpu-title">CPU Usage</div>
          <div className="dash-cpu-bars">
            {proof.stats.cpuHistory.map((value, index) => (
              <div
                key={`${index}-${value}`}
                className="dash-cpu-bar"
                style={{
                  height: `${Math.max(4, Math.round((value / maxCpu) * 100))}%`,
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
            <span className="root-hash">{aggregateHash(liveRoots)}</span>
          </div>
          <div className="root-row">
            <span className="root-row-left">
              <span className="root-dot nullified" />
              <span className="root-label">Global Nullified State Roots</span>
            </span>
            <span className="root-hash">{aggregateHash(nullifiedRoots)}</span>
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

  if (
    proof.status === "generating" ||
    proof.status === "committing" ||
    proof.status === "done"
  ) {
    const stage1Done = proof.status === "committing" || proof.status === "done";
    const stage2Done = proof.status === "done";
    const stage2Active = proof.status === "committing";
    const hashStep = proof.steps.find((step) => step.id === "hash");
    const verifySteps = proof.steps.filter((step) => step.id.startsWith("verify-"));
    const nullifyStep = proof.steps.find((step) => step.id === "nullify");
    const commitStep = proof.steps.find((step) => step.id === "commit");

    const statusClass = (status: "pending" | "running" | "done") =>
      status === "done" ? "done" : status === "running" ? "running" : "pending";

    return (
      <section className="cpu-panel proof-panel proof-run-card">
        <div className="stage-header">
          <span className={`stage-num ${stage1Done ? "done" : "active"}`}>1</span>
          <span className="stage-title">Generating Recursive Proof</span>
        </div>

        {proof.status === "generating" && (
          <div className="stage-details">
            <div className="stage-detail-line">
              <span className="stage-detail-label">Hashing</span>
              <span
                className={`stage-detail-value ${statusClass(hashStep?.status ?? "running")}`}
              >
                {hashStep?.detail ?? proof.cpuCost ?? "..."}
              </span>
            </div>
            {verifySteps.map((step) => (
              <div key={step.id} className="stage-detail-line">
                <span className="stage-detail-label">Verifying</span>
                <span className={`stage-detail-value ${statusClass(step.status)}`}>
                  {step.detail}
                </span>
              </div>
            ))}
          </div>
        )}

        <div className={`stage-header ${stage2Active || stage2Done ? "" : "pending"}`}>
          <span
            className={`stage-num ${stage2Done ? "done" : stage2Active ? "active" : "pending"}`}
          >
            2
          </span>
          <span className="stage-title">Committing New State Root</span>
        </div>

        {(proof.status === "committing" || proof.status === "done") && (
          <div className="stage-details">
            <div className="stage-detail-line">
              <span className="stage-detail-label">Nullifying</span>
              <span
                className={`stage-detail-value danger ${
                  nullifyStep?.status === "running"
                    ? "running"
                    : nullifyStep?.status === "pending"
                      ? "pending"
                      : ""
                }`}
              >
                {nullifyStep?.detail ?? proof.oldRoot ?? "pending"}
              </span>
            </div>
            <div className="stage-detail-line">
              <span className="stage-detail-label">New State Root</span>
              <span
                className={`stage-detail-value ${
                  commitStep?.status === "done"
                    ? "done"
                    : commitStep?.status === "running"
                      ? "running"
                      : "pending"
                }`}
              >
                {commitStep?.status === "pending"
                  ? "—"
                  : (commitStep?.detail ?? proof.newRoot ?? "pending")}
              </span>
            </div>
          </div>
        )}

        {proof.status === "done" && (
          <div className="proof-complete-bar">✓ complete</div>
        )}
      </section>
    );
  }

  return null;
}
