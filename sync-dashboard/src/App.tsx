import { startTransition, useEffect, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import {
  ApiClientError,
  type DashboardRecentSlot,
  type DashboardRecentSlotsResponse,
  type DashboardStatus,
  type DashboardSummary,
  type TxLookupResult,
  fetchDashboardSummary,
  fetchRecentSlots,
  fetchTxLookup,
  synchronizerApiBaseUrl,
} from "./api";
import "./styles.css";

type LoadState = "idle" | "loading" | "ready" | "error";

interface ResourceState<T> {
  state: LoadState;
  data: T | null;
  error: string | null;
  updatedAt: number | null;
}

const POLL_SUMMARY_MS = 5_000;
const POLL_SLOTS_MS = 10_000;
const RECENT_SLOT_LIMIT = 25;

function initialResource<T>(): ResourceState<T> {
  return {
    state: "idle",
    data: null,
    error: null,
    updatedAt: null,
  };
}

function formatHash(value: string | null, prefix = 10, suffix = 8): string {
  if (!value) {
    return "None";
  }
  if (value.length <= prefix + suffix + 1) {
    return value;
  }
  return `${value.slice(0, prefix)}...${value.slice(-suffix)}`;
}

function formatNumber(value: number | null): string {
  if (value === null) {
    return "None";
  }
  return new Intl.NumberFormat().format(value);
}

function formatTimestamp(value: string | null): string {
  if (!value) {
    return "Unknown";
  }

  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(date);
}

function formatAgeFromNow(value: string | null): string {
  if (!value) {
    return "Unknown";
  }

  const time = new Date(value).getTime();
  if (Number.isNaN(time)) {
    return "Unknown";
  }

  const deltaMs = Math.max(0, Date.now() - time);
  const seconds = Math.floor(deltaMs / 1_000);

  if (seconds < 60) {
    return `${seconds}s ago`;
  }

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m ago`;
  }

  const hours = Math.floor(minutes / 60);
  if (hours < 48) {
    return `${hours}h ago`;
  }

  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function statusTone(status: DashboardStatus | "unknown"): string {
  switch (status) {
    case "healthy":
      return "status-healthy";
    case "lagging":
      return "status-lagging";
    case "recovering":
      return "status-recovering";
    case "stalled":
      return "status-stalled";
    default:
      return "status-unknown";
  }
}

function connectionTone(resource: ResourceState<unknown>): string {
  if (resource.state === "ready") {
    return resource.error ? "health-stale" : "health-ok";
  }
  if (resource.state === "loading" || resource.state === "idle") {
    return "health-pending";
  }
  return "health-error";
}

function resourceLabel(resource: ResourceState<unknown>): string {
  if (resource.state === "loading" || resource.state === "idle") {
    return "Loading";
  }
  if (resource.state === "ready" && resource.error) {
    return "Using stale data";
  }
  if (resource.state === "ready") {
    return "Connected";
  }
  return "Unavailable";
}

function explainResourceError(error: string | null): string {
  if (!error) {
    return "Receiving data normally.";
  }

  return `${error}. Check the synchronizer URL, CORS_ALLOWED_ORIGINS, and whether the API is reachable from this browser.`;
}

async function copyText(value: string) {
  try {
    await navigator.clipboard.writeText(value);
  } catch {
    // Clipboard failures are non-fatal in public browsers.
  }
}

function App() {
  const [summary, setSummary] = useState<ResourceState<DashboardSummary>>(
    initialResource(),
  );
  const [recentSlots, setRecentSlots] =
    useState<ResourceState<DashboardRecentSlotsResponse>>(initialResource());
  const [refreshToken, setRefreshToken] = useState(0);
  const [expandedSlots, setExpandedSlots] = useState<Set<number>>(new Set());
  const [lookupInput, setLookupInput] = useState("");
  const [lookupResult, setLookupResult] = useState<ResourceState<TxLookupResult>>(
    initialResource(),
  );
  const [lookupSubmitted, setLookupSubmitted] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    const loadSummary = async (silent: boolean) => {
      if (!silent) {
        setSummary((previous) => ({
          ...previous,
          state: previous.data ? previous.state : "loading",
          error: null,
        }));
      }

      try {
        const next = await fetchDashboardSummary();
        if (cancelled) {
          return;
        }

        startTransition(() => {
          setSummary({
            state: "ready",
            data: next,
            error: null,
            updatedAt: Date.now(),
          });
        });
      } catch (error) {
        if (cancelled) {
          return;
        }

        const message =
          error instanceof ApiClientError ? error.message : "Failed to load summary";
        startTransition(() => {
          setSummary((previous) => ({
            state: previous.data ? "ready" : "error",
            data: previous.data,
            error: message,
            updatedAt: previous.updatedAt,
          }));
        });
      }
    };

    void loadSummary(false);
    const intervalId = window.setInterval(() => {
      void loadSummary(true);
    }, POLL_SUMMARY_MS);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [refreshToken]);

  useEffect(() => {
    let cancelled = false;

    const loadRecentSlots = async (silent: boolean) => {
      if (!silent) {
        setRecentSlots((previous) => ({
          ...previous,
          state: previous.data ? previous.state : "loading",
          error: null,
        }));
      }

      try {
        const next = await fetchRecentSlots(RECENT_SLOT_LIMIT);
        if (cancelled) {
          return;
        }

        startTransition(() => {
          setRecentSlots({
            state: "ready",
            data: next,
            error: null,
            updatedAt: Date.now(),
          });
        });
      } catch (error) {
        if (cancelled) {
          return;
        }

        const message =
          error instanceof ApiClientError
            ? error.message
            : "Failed to load recent slot activity";
        startTransition(() => {
          setRecentSlots((previous) => ({
            state: previous.data ? "ready" : "error",
            data: previous.data,
            error: message,
            updatedAt: previous.updatedAt,
          }));
        });
      }
    };

    void loadRecentSlots(false);
    const intervalId = window.setInterval(() => {
      void loadRecentSlots(true);
    }, POLL_SLOTS_MS);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [refreshToken]);

  const slotCards = recentSlots.data?.slots ?? [];
  const heroStatus = summary.data?.status ?? "unknown";
  const heroLag =
    summary.data?.blockLag ?? summary.data?.slotLag ?? null;
  const heroLagLabel =
    summary.data?.blockLag !== null && summary.data?.blockLag !== undefined
      ? "blocks behind"
      : "slots behind";
  const heroReason =
    summary.data?.statusReason ??
    (summary.error
      ? `Summary unavailable: ${summary.error}`
      : "Loading dashboard summary");
  const latestTimestamp = Math.max(summary.updatedAt ?? 0, recentSlots.updatedAt ?? 0);
  const lastRefreshLabel = latestTimestamp
    ? `Last refreshed ${formatAgeFromNow(new Date(latestTimestamp).toISOString())}`
    : "Waiting for the first successful refresh";

  const handleManualRefresh = () => {
    setRefreshToken((value) => value + 1);
  };

  const handleLookupSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();

    const nextQuery = lookupInput.trim();
    setLookupSubmitted(nextQuery);

    if (!nextQuery) {
      setLookupResult({
        state: "error",
        data: null,
        error: "Enter a transaction hash to inspect",
        updatedAt: null,
      });
      return;
    }

    setLookupResult({
      state: "loading",
      data: null,
      error: null,
      updatedAt: null,
    });

    try {
      const next = await fetchTxLookup(nextQuery);
      setLookupResult({
        state: "ready",
        data: next,
        error: null,
        updatedAt: Date.now(),
      });
    } catch (error) {
      const message =
        error instanceof ApiClientError
          ? error.message
          : "Failed to load transaction lookup";
      setLookupResult({
        state: "error",
        data: null,
        error: message,
        updatedAt: null,
      });
    }
  };

  const toggleExpandedSlot = (slot: number) => {
    setExpandedSlots((previous) => {
      const next = new Set(previous);
      if (next.has(slot)) {
        next.delete(slot);
      } else {
        next.add(slot);
      }
      return next;
    });
  };

  return (
    <main className="page-shell">
      <section className="hero-card">
        <div className="hero-topline">
          <div>
            <p className="eyebrow">Standalone status site</p>
            <h1>Synchronizer Dashboard</h1>
            <p className="hero-copy">
              Live synchronizer status for users who need a quick answer and
              operators who need the last few slots without opening the desktop
              client.
            </p>
          </div>
          <div className="hero-actions">
            <button className="refresh-button" onClick={handleManualRefresh}>
              Refresh now
            </button>
            <p className="hero-hint">{lastRefreshLabel}</p>
          </div>
        </div>

        <div className="hero-meta">
          <div className="meta-card">
            <span className={`status-pill ${statusTone(heroStatus)}`}>
              {heroStatus}
              {heroLag !== null && heroLag > 0 && heroStatus !== "healthy" && (
                <span className="lag-badge">
                  {heroLag.toLocaleString()} {heroLagLabel}
                </span>
              )}
            </span>
            <p className="meta-label">Latest status</p>
            <p className="meta-value">{heroReason}</p>
          </div>
          <div className="meta-card">
            <p className="meta-label">API base URL</p>
            <p className="meta-value meta-mono">{synchronizerApiBaseUrl}</p>
          </div>
          <div className="meta-card">
            <p className="meta-label">Cursor updated</p>
            <p className="meta-value">
              {summary.data?.cursorUpdatedAt
                ? `${formatTimestamp(summary.data.cursorUpdatedAt)} (${formatAgeFromNow(
                    summary.data.cursorUpdatedAt,
                  )})`
                : "Unknown"}
            </p>
          </div>
        </div>

        <div className="metrics-grid">
          <MetricCard label="Last processed slot" value={formatNumber(summary.data?.lastProcessedSlot ?? null)} />
          <MetricCard label="Beacon head slot" value={formatNumber(summary.data?.beaconHeadSlot ?? null)} />
          <MetricCard label="Slot lag" value={formatNumber(summary.data?.slotLag ?? null)} />
          <MetricCard label="Last processed block" value={formatNumber(summary.data?.lastProcessedBlockNumber ?? null)} />
          <MetricCard label="Beacon head block" value={formatNumber(summary.data?.beaconHeadBlockNumber ?? null)} />
          <MetricCard label="Block lag" value={formatNumber(summary.data?.blockLag ?? null)} />
          <MetricCard label="Current state block" value={formatNumber(summary.data?.currentBlockNumber ?? null)} />
          <MetricCard label="Pending recoveries" value={formatNumber(summary.data?.pendingRecoveryCount ?? null)} />
          <MetricCard label="Accepted txs" value={formatNumber(summary.data?.txCount ?? null)} />
          <MetricCard label="Spent nullifiers" value={formatNumber(summary.data?.nullifierCount ?? null)} />
          <MetricCard label="Tracked GSRs" value={formatNumber(summary.data?.gsrCount ?? null)} />
        </div>

        <div className="hash-banner">
          <div>
            <p className="meta-label">Current GSR</p>
            <p className="meta-value meta-mono">
              {summary.data?.currentGsr ? formatHash(summary.data.currentGsr, 14, 10) : "None"}
            </p>
          </div>
          <button
            className="copy-button"
            disabled={!summary.data?.currentGsr}
            onClick={() => {
              if (summary.data?.currentGsr) {
                void copyText(summary.data.currentGsr);
              }
            }}
          >
            Copy full hash
          </button>
        </div>
      </section>

      <div className="dashboard-grid">
        <section className="panel recent-panel">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Recent canonical activity</p>
              <h2>Latest {RECENT_SLOT_LIMIT} slots</h2>
            </div>
            <p className="panel-subtle">
              Empty slots, pending apply states, and the most recent GSR emitted
              by each canonical slot.
            </p>
          </div>

          {recentSlots.state === "loading" && slotCards.length === 0 ? (
            <PanelMessage
              tone="pending"
              title="Loading recent slot activity"
              body="Waiting for the synchronizer API to return canonical slot history."
            />
          ) : null}

          {recentSlots.state === "error" && slotCards.length === 0 ? (
            <PanelMessage
              tone="error"
              title="Recent slot activity unavailable"
              body={explainResourceError(recentSlots.error)}
            />
          ) : null}

          {recentSlots.state === "ready" && slotCards.length === 0 ? (
            <PanelMessage
              tone="neutral"
              title="No canonical slots yet"
              body="The synchronizer has not persisted any canonical slot history yet."
            />
          ) : null}

          {slotCards.length > 0 ? (
            <div className="slot-list">
              {slotCards.map((slot) => {
                const expanded = expandedSlots.has(slot.slot);
                return (
                  <article className="slot-card" key={slot.slot}>
                    <button
                      className="slot-summary"
                      onClick={() => toggleExpandedSlot(slot.slot)}
                    >
                      <div className="slot-heading">
                        <span className="slot-title">
                          Slot {formatNumber(slot.slot)}
                        </span>
                        <span
                          className={`mini-pill ${
                            slot.status === "applied"
                              ? "mini-pill-applied"
                              : "mini-pill-pending"
                          }`}
                        >
                          {slot.status}
                        </span>
                        <span
                          className={`mini-pill ${
                            slot.isEmpty ? "mini-pill-empty" : "mini-pill-filled"
                          }`}
                        >
                          {slot.isEmpty ? "empty" : "non-empty"}
                        </span>
                      </div>
                      <div className="slot-stats">
                        <span>Exec block {formatNumber(slot.executionBlockNumber)}</span>
                        <span>{formatNumber(slot.txCount)} txs</span>
                        <span>{formatNumber(slot.nullifierCount)} nullifiers</span>
                        <span>{formatAgeFromNow(slot.updatedAt)}</span>
                      </div>
                    </button>

                    {expanded ? <ExpandedSlot slot={slot} /> : null}
                  </article>
                );
              })}
            </div>
          ) : null}
        </section>

        <section className="panel lookup-panel">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Interactive drill-down</p>
              <h2>Transaction lookup</h2>
            </div>
            <p className="panel-subtle">
              Check whether a transaction hash has been observed and grounded by
              the synchronizer.
            </p>
          </div>

          <form className="lookup-form" onSubmit={handleLookupSubmit}>
            <label className="lookup-label" htmlFor="tx-hash-input">
              Transaction hash
            </label>
            <div className="lookup-row">
              <input
                id="tx-hash-input"
                className="lookup-input"
                type="text"
                value={lookupInput}
                placeholder="0x..."
                onChange={(event) => setLookupInput(event.target.value)}
              />
              <button className="lookup-button" type="submit">
                Inspect
              </button>
            </div>
          </form>

          {lookupResult.state === "idle" ? (
            <PanelMessage
              tone="neutral"
              title="No lookup yet"
              body="Enter a transaction hash to see whether it is present in the synchronizer state."
            />
          ) : null}

          {lookupResult.state === "loading" ? (
            <PanelMessage
              tone="pending"
              title="Looking up transaction"
              body={`Checking ${lookupSubmitted ?? "the requested hash"} against the synchronizer.`}
            />
          ) : null}

          {lookupResult.state === "error" ? (
            <PanelMessage
              tone="error"
              title="Lookup failed"
              body={lookupResult.error ?? "The lookup request failed."}
            />
          ) : null}

          {lookupResult.state === "ready" && lookupResult.data ? (
            <div className="lookup-result">
              <div className="lookup-status">
                <span
                  className={`status-pill ${
                    lookupResult.data.present ? "status-healthy" : "status-lagging"
                  }`}
                >
                  {lookupResult.data.present ? "present" : "missing"}
                </span>
                <p className="meta-value meta-mono">
                  {formatHash(lookupResult.data.txHash, 16, 10)}
                </p>
              </div>

              <dl className="lookup-details">
                <DetailRow
                  label="Last processed slot"
                  value={formatNumber(lookupResult.data.lastProcessedSlot)}
                />
                <DetailRow
                  label="Current GSR"
                  value={
                    lookupResult.data.currentGsr
                      ? formatHash(lookupResult.data.currentGsr, 14, 10)
                      : "None"
                  }
                />
                <DetailRow
                  label="Interpretation"
                  value={
                    lookupResult.data.present
                      ? "The synchronizer has already accepted this transaction."
                      : "The synchronizer has not accepted this transaction at the current head."
                  }
                />
              </dl>
            </div>
          ) : null}
        </section>

        <section className="panel health-panel">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Feed health</p>
              <h2>API availability states</h2>
            </div>
            <p className="panel-subtle">
              The dashboard keeps rendering stale data when it can and surfaces
              connectivity problems explicitly when it cannot.
            </p>
          </div>

          <div className="health-grid">
            <HealthFeedCard
              title="Summary feed"
              label={resourceLabel(summary)}
              tone={connectionTone(summary)}
              detail={explainResourceError(summary.error)}
            />
            <HealthFeedCard
              title="Recent slots feed"
              label={resourceLabel(recentSlots)}
              tone={connectionTone(recentSlots)}
              detail={explainResourceError(recentSlots.error)}
            />
            <HealthFeedCard
              title="Lookup endpoint"
              label={
                lookupResult.state === "error"
                  ? "Request failed"
                  : lookupResult.state === "loading"
                    ? "In progress"
                    : lookupResult.state === "ready"
                      ? "Responding"
                      : "Idle"
              }
              tone={
                lookupResult.state === "error"
                  ? "health-error"
                  : lookupResult.state === "ready"
                    ? "health-ok"
                    : "health-pending"
              }
              detail={
                lookupResult.state === "error"
                  ? explainResourceError(lookupResult.error)
                  : "Uses the existing synchronizer transaction-status API for direct inspection."
              }
            />
          </div>
        </section>
      </div>
    </main>
  );
}

function MetricCard(props: { label: string; value: string }) {
  return (
    <article className="metric-card">
      <p className="metric-label">{props.label}</p>
      <p className="metric-value">{props.value}</p>
    </article>
  );
}

function PanelMessage(props: {
  tone: "pending" | "error" | "neutral";
  title: string;
  body: string;
}) {
  return (
    <div className={`panel-message panel-message-${props.tone}`}>
      <h3>{props.title}</h3>
      <p>{props.body}</p>
    </div>
  );
}

function ExpandedSlot(props: { slot: DashboardRecentSlot }) {
  const { slot } = props;

  return (
    <div className="slot-expanded">
      <dl className="expanded-grid">
        <DetailRow label="Updated at" value={formatTimestamp(slot.updatedAt)} />
        <DetailRow
          label="Latest GSR"
          value={slot.gsrHash ? formatHash(slot.gsrHash, 14, 10) : "None"}
          action={
            slot.gsrHash ? (
              <button
                className="copy-button copy-inline"
                onClick={() => {
                  void copyText(slot.gsrHash as string);
                }}
              >
                Copy
              </button>
            ) : null
          }
        />
        <DetailRow
          label="Block root"
          value={slot.blockRoot ? formatHash(slot.blockRoot, 14, 10) : "None"}
          action={
            slot.blockRoot ? (
              <button
                className="copy-button copy-inline"
                onClick={() => {
                  void copyText(slot.blockRoot as string);
                }}
              >
                Copy
              </button>
            ) : null
          }
        />
        <DetailRow
          label="Parent root"
          value={slot.parentRoot ? formatHash(slot.parentRoot, 14, 10) : "None"}
          action={
            slot.parentRoot ? (
              <button
                className="copy-button copy-inline"
                onClick={() => {
                  void copyText(slot.parentRoot as string);
                }}
              >
                Copy
              </button>
            ) : null
          }
        />
      </dl>
    </div>
  );
}

function DetailRow(props: {
  label: string;
  value: string;
  action?: ReactNode;
}) {
  return (
    <div className="detail-row">
      <dt>{props.label}</dt>
      <dd>
        <span>{props.value}</span>
        {props.action}
      </dd>
    </div>
  );
}

function HealthFeedCard(props: {
  title: string;
  label: string;
  tone: string;
  detail: string;
}) {
  return (
    <article className="health-card">
      <div className="health-topline">
        <h3>{props.title}</h3>
        <span className={`mini-pill ${props.tone}`}>{props.label}</span>
      </div>
      <p>{props.detail}</p>
    </article>
  );
}

export default App;
