import { useUiStore } from "../../shared/state/uiStore";

export function ProofRunnerPanel() {
  const proof = useUiStore((state) => state.proof);
  const liveCount = proof.stats.roots.filter((root) => root.state === "live").length;
  const nullifiedCount = proof.stats.roots.filter((root) => root.state === "nullified").length;

  if (proof.status === "idle") {
    const maxCpu = Math.max(...proof.stats.cpuHistory, 1);
    return (
      <section className="cpu-panel proof-panel">
        <div className="idle-section idle-cpu">
          <div className="proof-title">CPU Usage</div>
          <div className="dash-cpu-bars">
            {proof.stats.cpuHistory.map((value, index) => (
              <div
                key={`${index}-${value}`}
                className="dash-cpu-bar"
                style={{ height: `${Math.max(4, Math.round((value / maxCpu) * 100))}%` }}
              />
            ))}
          </div>
          <div className="proof-line">
            Total CPU time: <span className="proof-muted">{proof.stats.totalCpuSecs}s</span>
          </div>
        </div>
        <div className="idle-section idle-roots">
          <div className="proof-title">Global Roots</div>
          <div className="proof-line">
            <span className="root-dot live" /> live: {liveCount}
          </div>
          <div className="proof-line">
            <span className="root-dot nullified" /> nullified: {nullifiedCount}
          </div>
        </div>
      </section>
    );
  }

  if (proof.status === "generating" || proof.status === "committing") {
    return (
      <section className="cpu-panel proof-panel">
        <div className="proof-title">
          {proof.status === "generating" ? "Stage 1: Generating Proof" : "Stage 2: Committing State"}
        </div>
        {proof.steps.map((step) => (
          <div key={step.id} className="proof-step">
            <span className={`proof-step-dot ${step.status}`} />
            <span className="proof-line">
              {step.label}: <span className="proof-muted">{step.detail}</span>
            </span>
          </div>
        ))}
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

  return (
    <section className="cpu-panel proof-panel">
      <div className="proof-title">Proof + Commit Complete</div>
      <div className="proof-line">Method: {proof.methodName}</div>
      <div className="proof-line">Old root: {proof.oldRoot}</div>
      <div className="proof-line">New root: {proof.newRoot}</div>
      {proof.steps.length > 0 && (
        <div className="proof-log">
          {proof.steps.map((step) => (
            <div key={step.id} className="proof-step">
              <span className={`proof-step-dot ${step.status}`} />
              <span>{step.label}</span>
            </div>
          ))}
        </div>
      )}
      <div className="proof-log">
        {proof.messages.map((message) => (
          <div key={message}>{message}</div>
        ))}
      </div>
    </section>
  );
}
