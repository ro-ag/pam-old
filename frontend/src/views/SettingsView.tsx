import { ArrowClockwise, FileText, FolderOpen, GitBranch, HardDrive, Trash } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { withDaemonOperation } from "../bridge";
import type { AppSettingsDto, PamBridge } from "../domain";
import { presentError } from "../state";

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const exponent = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  const value = bytes / 1024 ** exponent;
  return `${exponent === 0 ? value : value.toFixed(1)} ${units[exponent]}`;
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
  const requestSequence = useRef(0);

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
        <div><h1>Settings</h1><p>Where PAM keeps things, and how to clear its logs.</p></div>
      </header>
      {loadError ? (
        <section className="panel"><p className="panel-empty" role="alert">{loadError}</p></section>
      ) : !settings ? (
        <section className="panel"><p className="panel-empty" aria-busy="true" aria-live="polite">Loading Settings…</p></section>
      ) : (
        <>
          <section className="panel" aria-labelledby="storage-heading">
            <div className="panel-title">
              <div><span className="eyebrow">Storage</span><h2 id="storage-heading">Where PAM keeps things</h2></div>
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
                  {modelsDirError && <p className="connector-note" role="alert">{modelsDirError}</p>}
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
            {revealError && <p className="connector-note" role="alert">{revealError}</p>}
          </section>

          <section className="panel" aria-labelledby="logs-heading">
            <div className="panel-title">
              <div><span className="eyebrow">Diagnostics</span><h2 id="logs-heading">Logs</h2></div>
            </div>
            <p className="connector-note">
              {formatBytes(settings.logsSizeBytes)} on disk today. PAM also keeps a live, bounded window
              of recent lines in memory — see it in{" "}
              <button type="button" className="button button--secondary button--small" onClick={onOpenConsole}>
                Console
              </button>
              .
            </p>
            {deleteError && <p className="connector-note" role="alert">{deleteError}</p>}
            {!confirmingDelete ? (
              <button
                type="button"
                className="button button--secondary button--small"
                disabled={settings.logsSizeBytes === 0}
                onClick={() => setConfirmingDelete(true)}
              >
                <Trash size={17} /> Delete logs
              </button>
            ) : (
              <span className="connector-confirm">
                Delete PAM&apos;s on-disk log files? The in-memory console keeps working either way.
                <button type="button" className="button button--secondary button--small" disabled={deleteBusy} onClick={() => void deleteLogs()}>
                  Delete
                </button>
                <button type="button" className="button button--secondary button--small" disabled={deleteBusy} onClick={() => setConfirmingDelete(false)}>
                  Keep
                </button>
              </span>
            )}
          </section>
        </>
      )}
    </main>
  );
}
