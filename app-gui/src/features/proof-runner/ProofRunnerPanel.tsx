import { useUiStore } from "../../shared/state/uiStore";

export function ProofRunnerPanel() {
  const proof = useUiStore((state) => state.proof);

  if (proof.status === "idle") {
    return <section className="cpu-panel">Run a method to generate and commit proof.</section>;
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
