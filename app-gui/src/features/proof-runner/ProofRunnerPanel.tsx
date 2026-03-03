import { useUiStore } from "../../shared/state/uiStore";

export function ProofRunnerPanel() {
  const proof = useUiStore((state) => state.proof);
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

  if (proof.status === "idle") {
    const maxCpu = Math.max(...proof.stats.cpuHistory, 1);
    return (
      <section className="cpu-panel proof-panel proof-panel-idle">
        <div className="idle-section idle-cpu">
          <div className="proof-title">CPU Usage</div>
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
          <div className="proof-line">
            Total:{" "}
            <span className="proof-muted">{proof.stats.totalCpuSecs}s</span>
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
    return (
      <section className="cpu-panel proof-panel proof-run-card">
        <div className="stage-header">
          <span className={`stage-num ${stage1Done ? "done" : "active"}`}>1</span>
          <span className="stage-title">Generating Recursive Proof</span>
        </div>

        <div className="stage-header">
          <span
            className={`stage-num ${stage2Done ? "done" : stage2Active ? "active" : "pending"}`}
          >
            2
          </span>
          <span className="stage-title">Committing New State Root</span>
        </div>

        {(proof.status === "committing" || proof.status === "done") && (
          <div className="stage-lines">
            <div className="proof-line">
              Nullifying{" "}
              <span className="proof-inline danger">{proof.oldRoot ?? "pending"}</span>
            </div>
            <div className="proof-line">
              New State Root{" "}
              <span className="proof-inline good">{proof.newRoot ?? "pending"}</span>
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
