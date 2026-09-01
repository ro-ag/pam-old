import { ArrowClockwise, FileText, FolderOpen, GitBranch, HardDrive, ListMagnifyingGlass, Terminal, Trash, Warning } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { withDaemonOperation } from "../bridge";
import { ConfirmAction, FailureNotice, SuccessNotice } from "../components/ActionFeedback";
import { PanelError, PanelLoading } from "../components/PanelState";
import type { AppSettingsDto, PamBridge, ResetDto, ResetResultDto } from "../domain";
import { presentError } from "../state";

// Reset is tiered, so the danger zone is four separate controls plus the
// factory one. Nothing here is a single nuke button.
const RESET_TIERS = [
  {
    id: "access",
    action: "Reset access",
    blurb: "Revoke every capability grant and approval. Callers stay paired.",
  },
  {
    id: "identity",
    action: "Reset identity",
    blurb: "Revoke every caller and purge its keychain entry. Every caller has to pair again.",
  },
  {
    id: "history",
    action: "Clear history",
    blurb: "Delete the audit ledger, retained evidence, and flow-run history.",
  },
  {
    id: "registry",
    action: "Reset the model registry",
    blurb: "Unregister every model. The weights on disk are not touched.",
  },
] as const;

type ResetTierId = (typeof RESET_TIERS)[number]["id"];
type ResetControlId = ResetTierId | "factory";

/// The word a factory reset requires, typed in full. A second click is not
/// enough for the one operation that also deletes the flow library.
const FACTORY_CONFIRMATION = "RESET";

/// What went wrong and how to get unstuck, kept apart so the refusal renders
/// as two lines instead of one run-on sentence.
interface ResetFailure {
  detail: string;
  recovery: string | null;
}

function runTierReset(bridge: PamBridge, tier: ResetTierId, dryRun: boolean): Promise<ResetDto> {
  switch (tier) {
    case "access":
      return bridge.resetAccess(withDaemonOperation(), dryRun);
    case "identity":
      return bridge.resetIdentity(withDaemonOperation(), dryRun);
    case "history":
      return bridge.resetHistory(withDaemonOperation(), dryRun);
    case "registry":
      return bridge.resetRegistry(withDaemonOperation(), dryRun);
  }
}

function resetSummary(result: ResetResultDto): string {
  return `${result.totalItems} items, ${formatBytes(result.totalBytes)}`;
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const exponent = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  const value = bytes / 1024 ** exponent;
  return `${exponent === 0 ? value : value.toFixed(1)} ${units[exponent]}`;
}

// The wire's snake_case kinds, translated for the one screen where a person
// decides what leaves. An unknown kind still renders, prettified.
const RESET_KIND_LABELS: Record<string, string> = {
  grants: "Capability grants",
  approvals: "Approvals",
  flow_authorizations: "Flow authorizations",
  callers: "Callers",
  caller_files: "Caller files",
  keychain_entries: "Keychain entries",
  audit_events: "Audit events",
  evidence: "Evidence",
  flow_runs: "Flow runs",
  models: "Model registrations",
  model_weights: "Model weights",
  flows: "Flows",
  settings: "Settings",
  logs: "Logs",
  runtime: "Runtime files",
  state_database: "State database",
  other_data_files: "Other data files",
};

function resetKindLabel(kind: string): string {
  return RESET_KIND_LABELS[kind] ?? `${kind.charAt(0).toUpperCase()}${kind.slice(1)}`.replaceAll("_", " ");
}

// The blast radius as labeled rows: zero-count kinds are noise when something
// would leave, but with nothing to remove at all the empty kinds are the fact.
function ResetPreviewDetail({ result }: { result: ResetResultDto }) {
  const rows = result.items.filter((entry) => entry.count > 0);
  const flows = result.items.find((entry) => entry.kind === "flows");
  return (
    <div className="reset-preview">
      {rows.length > 0 && (
        <table>
          <tbody>
            {rows.map((entry) => (
              <tr key={entry.kind}>
                <td>{resetKindLabel(entry.kind)}</td>
                <td className="reset-preview-count">{entry.count}</td>
                <td>{formatBytes(entry.bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {flows && flows.names.length > 0 && (
        <p className="reset-preview-flows">Flows removed: {flows.names.join(", ")}.</p>
      )}
    </div>
  );
}

export interface SettingsViewProps {
  bridge: PamBridge;
  onOpenConsole: () => void;
}

export function SettingsView({ bridge, onOpenConsole }: SettingsViewProps) {
  const [settings, setSettings] = useState<AppSettingsDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [modelsDirInput, setModelsDirInput] = useState("");
  const [modelsDirError, setModelsDirError] = useState<string | null>(null);
  const [modelsDirSaving, setModelsDirSaving] = useState(false);
  const [revealError, setRevealError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  // One preview per control, held until it is spent or dismissed: the
  // confirm button only arms once its own dry run has been rendered.
  const [resetPreview, setResetPreview] = useState<Partial<Record<ResetControlId, ResetResultDto>>>({});
  const [resetOutcome, setResetOutcome] = useState<Partial<Record<ResetControlId, ResetResultDto>>>({});
  const [resetError, setResetError] = useState<Partial<Record<ResetControlId, ResetFailure>>>({});
  const [resetBusy, setResetBusy] = useState<ResetControlId | null>(null);
  const [includeWeights, setIncludeWeights] = useState(false);
  const [factoryTyped, setFactoryTyped] = useState("");
  const [receiptPath, setReceiptPath] = useState<string | null>(null);
  const requestSequence = useRef(0);

  const recordReset = (control: ResetControlId, response: ResetDto, dryRun: boolean) => {
    if (response.status !== "ok") {
      setResetError((current) => ({
        ...current,
        [control]: { detail: response.failure.detail, recovery: response.failure.recovery },
      }));
      setResetPreview((current) => ({ ...current, [control]: undefined }));
      return;
    }
    if (dryRun) {
      setResetPreview((current) => ({ ...current, [control]: response.result }));
      return;
    }
    setResetPreview((current) => ({ ...current, [control]: undefined }));
    setResetOutcome((current) => ({ ...current, [control]: response.result }));
    setReceiptPath(response.receiptPath);
  };

  const runReset = async (control: ResetControlId, dryRun: boolean) => {
    setResetBusy(control);
    setResetError((current) => ({ ...current, [control]: undefined }));
    try {
      const response =
        control === "factory"
          ? await bridge.factoryReset(withDaemonOperation(), dryRun, includeWeights)
          : await runTierReset(bridge, control, dryRun);
      recordReset(control, response, dryRun);
    } catch (error) {
      setResetError((current) => ({ ...current, [control]: { detail: presentError(error), recovery: null } }));
    } finally {
      setResetBusy(null);
      if (control === "factory" && !dryRun) setFactoryTyped("");
    }
  };

  const dismissPreview = (control: ResetControlId) => {
    setResetPreview((current) => ({ ...current, [control]: undefined }));
    if (control === "factory") setFactoryTyped("");
  };

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setBusy(true);
    setLoadError(null);
    try {
      // Settings is global: always the daemon authority, never a project fence.
      const response = await bridge.appSettings(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setSettings(response);
      setModelsDirInput(response.modelsDir);
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

  const reveal = async (path: string) => {
    setRevealError(null);
    try {
      await bridge.revealPath(withDaemonOperation(), path);
    } catch (error) {
      setRevealError(presentError(error));
    }
  };

  const updateModelsDir = async (nextDir: string | null) => {
    setModelsDirSaving(true);
    setModelsDirError(null);
    try {
      const response = await bridge.settingsUpdate(withDaemonOperation(), nextDir);
      setSettings(response);
      setModelsDirInput(response.modelsDir);
    } catch (error) {
      setModelsDirError(presentError(error));
    } finally {
      setModelsDirSaving(false);
    }
  };

  const deleteLogs = async () => {
    setDeleteBusy(true);
    setDeleteError(null);
    try {
      const response = await bridge.logsDelete(withDaemonOperation());
      setSettings(response);
    } catch (error) {
      setDeleteError(presentError(error));
    } finally {
      setDeleteBusy(false);
      setConfirmingDelete(false);
    }
  };

  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Settings</h1><p>Where Pam keeps things, and how to clear its logs.</p></div>
      </header>
      {loadError ? (
        <section className="panel"><PanelError>{loadError}</PanelError></section>
      ) : !settings ? (
        <section className="panel"><PanelLoading>Loading Settings…</PanelLoading></section>
      ) : (
        <>
          <section className="panel" aria-labelledby="storage-heading">
            <div className="panel-title">
              <div><span className="eyebrow">Storage</span><h2 id="storage-heading">Where Pam keeps things</h2></div>
              <button
                type="button"
                className="button button--secondary button--small"
                aria-label="Refresh settings"
                disabled={busy}
                onClick={() => void load()}
              >
                <ArrowClockwise className={busy ? "is-spinning" : ""} size={17} /> Refresh
              </button>
            </div>
            <div className="access-list">
              <article>
                <span className="access-icon" aria-hidden="true"><HardDrive size={21} /></span>
                <div>
                  <strong>Models</strong>
                  <p>{settings.modelsDir}</p>
                  <form
                    className="connector-row-controls"
                    onSubmit={(event) => { event.preventDefault(); void updateModelsDir(modelsDirInput.trim()); }}
                  >
                    <label>
                      Change directory
                      <input
                        type="text"
                        value={modelsDirInput}
                        disabled={modelsDirSaving}
                        onChange={(event) => setModelsDirInput(event.target.value)}
                      />
                    </label>
                    <button
                      type="submit"
                      className="button button--secondary button--small"
                      disabled={modelsDirSaving || modelsDirInput.trim() === "" || modelsDirInput.trim() === settings.modelsDir}
                    >
                      Save
                    </button>
                    {!settings.modelsDirIsDefault && (
                      <button
                        type="button"
                        className="button button--secondary button--small"
                        disabled={modelsDirSaving}
                        onClick={() => void updateModelsDir(null)}
                      >
                        Reset to default
                      </button>
                    )}
                  </form>
                  {modelsDirError && <FailureNotice detail={modelsDirError} />}
                </div>
                <button type="button" className="button button--secondary button--small" onClick={() => void reveal(settings.modelsDir)}>
                  Reveal
                </button>
              </article>
              <article>
                <span className="access-icon" aria-hidden="true"><FolderOpen size={21} /></span>
                <div><strong>Data</strong><p>{settings.dataDir}</p></div>
                <button type="button" className="button button--secondary button--small" onClick={() => void reveal(settings.dataDir)}>
                  Reveal
                </button>
              </article>
              <article>
                <span className="access-icon" aria-hidden="true"><GitBranch size={21} /></span>
                <div><strong>Flows</strong><p>{settings.flowsDir}</p></div>
                <button type="button" className="button button--secondary button--small" onClick={() => void reveal(settings.flowsDir)}>
                  Reveal
                </button>
              </article>
              <article>
                <span className="access-icon" aria-hidden="true"><FileText size={21} /></span>
                <div><strong>Logs</strong><p>{settings.logsDir}</p></div>
                <button type="button" className="button button--secondary button--small" onClick={() => void reveal(settings.logsDir)}>
                  Reveal
                </button>
              </article>
            </div>
            {revealError && <div className="panel-body"><FailureNotice detail={revealError} /></div>}
          </section>

          <section className="panel" aria-labelledby="logs-heading">
            <div className="panel-title">
              <div><span className="eyebrow">Diagnostics</span><h2 id="logs-heading">Logs</h2></div>
            </div>
            <div className="panel-body">
              <p className="connector-note">
                {formatBytes(settings.logsSizeBytes)} on disk today. Pam also keeps a live, bounded
                window of recent lines in memory.
              </p>
              {deleteError && <FailureNotice detail={deleteError} />}
              <div className="settings-actions">
                <button type="button" className="button button--secondary button--small" onClick={onOpenConsole}>
                  <Terminal size={17} /> Console
                </button>
                {!confirmingDelete && (
                  <button
                    type="button"
                    className="button button--secondary button--small"
                    disabled={settings.logsSizeBytes === 0}
                    onClick={() => setConfirmingDelete(true)}
                  >
                    <Trash size={17} /> Delete logs
                  </button>
                )}
              </div>
              {confirmingDelete && (
                <ConfirmAction
                  question="Delete Pam's on-disk log files? The in-memory console keeps working either way."
                  actionLabel="Delete"
                  busy={deleteBusy}
                  onConfirm={() => void deleteLogs()}
                  onCancel={() => setConfirmingDelete(false)}
                />
              )}
            </div>
          </section>

          <section className="panel panel--danger" aria-labelledby="danger-heading">
            <div className="panel-title">
              <div>
                <span className="eyebrow">Danger zone</span>
                <h2 id="danger-heading">Reset Pam</h2>
              </div>
            </div>
            <div className="panel-body">
              <p className="connector-note">
                Each reset is scoped. Preview one to see exactly what it would remove, in counts and
                bytes, before you confirm it.
              </p>
              {RESET_TIERS.map((tier) => {
                const preview = resetPreview[tier.id];
                const outcome = resetOutcome[tier.id];
                const failure = resetError[tier.id];
                const busy = resetBusy === tier.id;
                return (
                  <article key={tier.id} className="panel-body">
                    <p className="connector-note">
                      <strong>{tier.action}.</strong> {tier.blurb}
                    </p>
                    {failure && <FailureNotice detail={failure.detail} recovery={failure.recovery} />}
                    {outcome && <SuccessNotice>Removed {resetSummary(outcome)}.</SuccessNotice>}
                    {preview ? (
                      <ConfirmAction
                        testId={`reset-preview-${tier.id}`}
                        question={`This removes ${resetSummary(preview)}.`}
                        actionLabel={tier.action}
                        busy={busy}
                        onConfirm={() => void runReset(tier.id, false)}
                        onCancel={() => dismissPreview(tier.id)}
                      >
                        <ResetPreviewDetail result={preview} />
                      </ConfirmAction>
                    ) : (
                      <button
                        type="button"
                        className="button button--secondary button--small"
                        disabled={busy}
                        onClick={() => void runReset(tier.id, true)}
                      >
                        <ListMagnifyingGlass size={17} /> Preview {tier.action.toLowerCase()}
                      </button>
                    )}
                  </article>
                );
              })}

              <article className="panel-body">
                <p className="connector-note">
                  <strong>Factory reset.</strong> Every scope above, settings back to their defaults,
                  and the authored flow library at {settings.flowsDir}. Pam has to be stopped first.
                </p>
                <label className="connector-note">
                  <input
                    type="checkbox"
                    checked={includeWeights}
                    onChange={(event) => {
                      setIncludeWeights(event.target.checked);
                      dismissPreview("factory");
                    }}
                  />{" "}
                  Also delete the weights of every registered model
                </label>
                {resetError.factory && (
                  <FailureNotice detail={resetError.factory.detail} recovery={resetError.factory.recovery} />
                )}
                {resetOutcome.factory && (
                  <SuccessNotice>
                    Removed {resetSummary(resetOutcome.factory)}.
                    {receiptPath ? ` Receipt written to ${receiptPath}.` : ""}
                  </SuccessNotice>
                )}
                {resetPreview.factory ? (
                  <ConfirmAction
                    testId="reset-preview-factory"
                    question={`This removes ${resetSummary(resetPreview.factory)}.`}
                    actionLabel="Factory reset"
                    busy={resetBusy === "factory"}
                    confirmDisabled={factoryTyped !== FACTORY_CONFIRMATION}
                    onConfirm={() => void runReset("factory", false)}
                    onCancel={() => dismissPreview("factory")}
                  >
                    <ResetPreviewDetail result={resetPreview.factory} />
                    <label>
                      Type {FACTORY_CONFIRMATION} to confirm
                      <input
                        type="text"
                        aria-label={`Type ${FACTORY_CONFIRMATION} to confirm the factory reset`}
                        value={factoryTyped}
                        onChange={(event) => setFactoryTyped(event.target.value)}
                      />
                    </label>
                  </ConfirmAction>
                ) : (
                  <button
                    type="button"
                    className="button button--secondary button--small"
                    disabled={resetBusy === "factory"}
                    onClick={() => void runReset("factory", true)}
                  >
                    <Warning size={17} /> Preview factory reset
                  </button>
                )}
              </article>
            </div>
          </section>
        </>
      )}
    </main>
  );
}
