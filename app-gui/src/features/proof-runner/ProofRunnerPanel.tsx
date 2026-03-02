import { useUiStore } from "../../shared/state/uiStore";

export function ProofRunnerPanel() {
  const proof = useUiStore((state) => state.proof);

  if (proof.status === "idle") {
    return <section className="cpu-panel">Run a method to generate and commit proof.</section>;
  }

  if (proof.status === "generating") {
    return (
      <section className="cpu-panel proof-panel">
        <div className="proof-title">Stage 1: Generating Proof</div>
        <div className="proof-line">Method: {proof.methodName}</div>
        <div className="proof-line">CPU: {proof.cpuCost}</div>
        <div className="proof-line">Status: running...</div>
        <div className="proof-line proof-muted">Stage 2: Commit pending</div>
      </section>
    );
  }

  if (proof.status === "committing") {
    return (
      <section className="cpu-panel proof-panel">
        <div className="proof-title">Stage 2: Committing State</div>
        <div className="proof-line">Old root: {proof.oldRoot}</div>
        <div className="proof-line">New root: {proof.newRoot}</div>
        <div className="proof-line">Status: committing...</div>
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
      <div className="proof-log">
        {proof.messages.map((message) => (
          <div key={message}>{message}</div>
        ))}
      </div>
    </section>
  );
}
