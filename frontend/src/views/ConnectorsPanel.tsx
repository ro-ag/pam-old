import { ArrowClockwise, PlugsConnected } from "@phosphor-icons/react";
import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { withDaemonOperation } from "../bridge";
import type {
  ConnectorConfigureParams,
  ConnectorCredentialAction,
  ConnectorSummaryDto,
  ConnectorsDto,
  ModelFailureDto,
  PamBridge,
} from "../domain";
import { presentError } from "../state";

function relativeTime(atMs: number): string {
  const diff = Date.now() - atMs;
  if (diff < 60_000) return "just now";
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

function failureText(failure: ModelFailureDto): string {
  return [failure.detail, failure.recovery].filter(Boolean).join(" ");
}

interface PendingAction {
  connector: string;
  action: "configure" | "test";
}

function ConnectorRow({
  summary,
  pending,
  error,
  testDetail,
  onConfigure,
  onTest,
}: {
  summary: ConnectorSummaryDto;
  pending: PendingAction | null;
  error: string | null;
  testDetail: string | null;
  onConfigure: (params: ConnectorConfigureParams) => Promise<boolean>;
  onTest: () => void;
}) {
  const [enabled, setEnabled] = useState(summary.enabled);
  const [baseUrl, setBaseUrl] = useState(summary.baseUrl ?? "");
  const [credentialOpen, setCredentialOpen] = useState(false);
  const [secret, setSecret] = useState("");
  const [confirmingRemoval, setConfirmingRemoval] = useState(false);

  useEffect(() => {
    setEnabled(summary.enabled);
    setBaseUrl(summary.baseUrl ?? "");
  }, [summary.enabled, summary.baseUrl]);

  const rowBusy = pending?.connector === summary.connectorId;
  const testBusy = rowBusy && pending?.action === "test";

  const submitCredential = (event: FormEvent) => {
    event.preventDefault();
    const value = secret;
    setSecret("");
    setCredentialOpen(false);
    if (value) void onConfigure({ connector: summary.connectorId, credential: { action: "set", secret: value } });
  };

  const credential = (action: ConnectorCredentialAction) =>
    void onConfigure({ connector: summary.connectorId, credential: action });

  return (
    <article aria-label={summary.connectorId}>
      <div className="connector-row-head">
        <span className="access-icon" aria-hidden="true"><PlugsConnected size={21} /></span>
        <strong>{summary.connectorId}</strong>
        <span className={`state-pill state-pill--${summary.credentialPresent ? "observed" : "not-reported"}`}>
          {summary.credentialPresent ? "credential stored" : "no credential"}
        </span>
        <span className={`state-pill state-pill--${summary.lastTestStatus === "passed" ? "succeeded" : summary.lastTestStatus === "failed" ? "failed" : "not-reported"}`}>
          {summary.lastTestStatus === null
            ? "never tested"
            : `test ${summary.lastTestStatus}${summary.lastTestAtMs === null ? "" : ` · ${relativeTime(summary.lastTestAtMs)}`}`}
        </span>
      </div>
      <div className="connector-row-controls">
        <label className="connector-enable-toggle">
          <input
            type="checkbox"
            role="switch"
            checked={enabled}
            disabled={rowBusy}
            onChange={(event) => setEnabled(event.target.checked)}
          />
          Enabled
        </label>
        <label>
          Base URL
          <input
            type="url"
            name={`${summary.connectorId}-base-url`}
            placeholder="https://api.example.com"
            value={baseUrl}
            disabled={rowBusy}
            onChange={(event) => setBaseUrl(event.target.value)}
          />
        </label>
        <button
          type="button"
          className="button button--secondary button--small"
          disabled={rowBusy}
          onClick={() => void onConfigure({ connector: summary.connectorId, enabled, baseUrl: baseUrl.trim() })}
        >
          Save
        </button>
        {!credentialOpen && (
          <button
            type="button"
            className="button button--secondary button--small"
            disabled={rowBusy}
            onClick={() => setCredentialOpen(true)}
          >
            {summary.credentialPresent ? "Replace credential…" : "Add credential…"}
          </button>
        )}
        {summary.credentialPresent && !confirmingRemoval && (
          <button
            type="button"
            className="button button--secondary button--small"
            disabled={rowBusy}
            onClick={() => setConfirmingRemoval(true)}
          >
            Remove credential
          </button>
        )}
        {confirmingRemoval && (
          <span className="connector-confirm">
            Remove the stored credential?
            <button
              type="button"
              className="button button--secondary button--small"
              disabled={rowBusy}
              onClick={() => { setConfirmingRemoval(false); credential({ action: "clear" }); }}
            >
              Remove
            </button>
            <button
              type="button"
              className="button button--secondary button--small"
              onClick={() => setConfirmingRemoval(false)}
            >
              Keep
            </button>
          </span>
        )}
        <button
          type="button"
          className="button button--secondary button--small"
          disabled={rowBusy}
          onClick={onTest}
        >
          <ArrowClockwise className={testBusy ? "is-spinning" : ""} size={17} /> Test connection
        </button>
      </div>
      {credentialOpen && (
        <form className="connector-credential-form" onSubmit={submitCredential}>
          <label>
            Credential
            <input
              type="password"
              name={`${summary.connectorId}-credential`}
              autoComplete="off"
              placeholder="Paste the token"
              value={secret}
              disabled={rowBusy}
              onChange={(event) => setSecret(event.target.value)}
            />
          </label>
          <button type="submit" className="button button--secondary button--small" disabled={rowBusy || !secret}>
            Save credential
          </button>
          <button
            type="button"
            className="button button--secondary button--small"
            onClick={() => { setSecret(""); setCredentialOpen(false); }}
          >
            Cancel
          </button>
        </form>
      )}
      {testDetail && <p className="connector-note">{testDetail}</p>}
      {error && <p className="connector-note" role="alert">{error}</p>}
    </article>
  );
}

export interface ConnectorsPanelProps {
  bridge: PamBridge;
}

export function ConnectorsPanel({ bridge }: ConnectorsPanelProps) {
  const [registry, setRegistry] = useState<ConnectorsDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<PendingAction | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string | null>>({});
  const [testDetails, setTestDetails] = useState<Record<string, string | null>>({});
  const requestSequence = useRef(0);

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setBusy(true);
    setLoadError(null);
    try {
      // The connector registry is daemon-global: always the daemon authority.
      const response = await bridge.connectorRegistry(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setRegistry(response);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    } finally {
      if (sequence === requestSequence.current) setBusy(false);
    }
  }, [bridge]);

  useEffect(() => {
    void load();
    return () => { requestSequence.current += 1; };
  }, [load]);

  const updateSummary = (summary: ConnectorSummaryDto) => {
    setRegistry((current) => current?.status === "ok"
      ? { ...current, connectors: current.connectors.map((candidate) => candidate.connectorId === summary.connectorId ? summary : candidate) }
      : current);
  };

  const configure = useCallback(async (params: ConnectorConfigureParams): Promise<boolean> => {
    const sequence = ++requestSequence.current;
    setPending({ connector: params.connector, action: "configure" });
    setRowErrors((current) => ({ ...current, [params.connector]: null }));
    try {
      const response = await bridge.connectorConfigure(withDaemonOperation(), params);
      if (sequence !== requestSequence.current) return false;
      if (response.status === "ok") {
        updateSummary(response.connector);
        return true;
      }
      setRowErrors((current) => ({ ...current, [params.connector]: failureText(response.failure) }));
      return false;
    } catch (error) {
      if (sequence === requestSequence.current) {
        setRowErrors((current) => ({ ...current, [params.connector]: presentError(error) }));
      }
      return false;
    } finally {
      if (sequence === requestSequence.current) setPending(null);
    }
  }, [bridge]);

  const test = useCallback(async (connector: string) => {
    const sequence = ++requestSequence.current;
    setPending({ connector, action: "test" });
    setRowErrors((current) => ({ ...current, [connector]: null }));
    setTestDetails((current) => ({ ...current, [connector]: null }));
    try {
      const response = await bridge.connectorTest(withDaemonOperation(), connector);
      if (sequence !== requestSequence.current) return;
      if (response.status === "ok") {
        setTestDetails((current) => ({ ...current, [connector]: response.detail }));
        setRegistry((current) => current?.status === "ok"
          ? {
              ...current,
              connectors: current.connectors.map((candidate) => candidate.connectorId === response.connectorId
                ? { ...candidate, lastTestStatus: response.result, lastTestAtMs: Date.now() }
                : candidate),
            }
          : current);
      } else {
        setRowErrors((current) => ({ ...current, [connector]: failureText(response.failure) }));
      }
    } catch (error) {
      if (sequence === requestSequence.current) {
        setRowErrors((current) => ({ ...current, [connector]: presentError(error) }));
      }
    } finally {
      if (sequence === requestSequence.current) setPending(null);
    }
  }, [bridge]);

  return (
    <section className="panel" aria-labelledby="connectors-heading">
      <div className="panel-title">
        <div><span className="eyebrow">Outbound bridges</span><h2 id="connectors-heading">Connectors</h2></div>
        <button
          type="button"
          className="button button--secondary button--small"
          aria-label="Refresh connectors"
          disabled={busy}
          onClick={() => void load()}
        >
          <ArrowClockwise className={busy ? "is-spinning" : ""} size={17} /> Refresh
        </button>
      </div>
      <p className="connector-intro">Connectors stay off until you enable them, add a credential, and run a test.</p>
      {loadError ? (
        <p className="panel-empty" role="alert">{loadError}</p>
      ) : !registry ? (
        <p className="panel-empty" aria-busy="true" aria-live="polite">Loading the connectors…</p>
      ) : registry.status !== "ok" ? (
        <p className="panel-empty">{failureText(registry.failure)}</p>
      ) : registry.connectors.length === 0 ? (
        <p className="panel-empty">No connectors are registered with the daemon yet.</p>
      ) : (
        <div className="connector-list">
          {registry.connectors.map((summary) => (
            <ConnectorRow
              key={summary.connectorId}
              summary={summary}
              pending={pending}
              error={rowErrors[summary.connectorId] ?? null}
              testDetail={testDetails[summary.connectorId] ?? null}
              onConfigure={configure}
              onTest={() => void test(summary.connectorId)}
            />
          ))}
        </div>
      )}
    </section>
  );
}
