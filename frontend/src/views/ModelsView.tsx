import { Brain, CaretDown, CaretRight, Check, Eject, FolderOpen, Power, PushPin, PushPinSlash, Stethoscope, Trash } from "@phosphor-icons/react";
import { Button, Menu, MenuItem, MenuTrigger, Popover } from "react-aria-components";
import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { withDaemonOperation } from "../bridge";
import { PanelEmpty, PanelLoading } from "../components/PanelState";
import type {
  DaemonStartupProgressDto,
  HostMemoryDto,
  ModelHealthDto,
  ModelImportParams,
  ModelInspectDto,
  ModelPresetDto,
  ModelStatusDto,
  ModelSweepDto,
  PamBridge,
} from "../domain";
import type { DaemonView } from "../selectors";
import { presentError } from "../state";
import { formatModelSize } from "./ActivityView";

// RAM reads in binary GiB, the way machines are sold: 34_359_738_368 -> "32 GB".
function formatHostMemory(bytes: number): string {
  return `${Math.round(bytes / (1 << 30))} GB`;
}

type VerifyState =
  | { state: "idle" }
  | { state: "running" }
  | { state: "pass"; ms: number; tokens: number }
  | { state: "fail"; detail: string };

type ImportState =
  | { state: "idle" }
  | { state: "running"; stage: "hashing" | "registering"; hashedBytes: number; totalBytes: number }
  | { state: "fail"; detail: string };

type DownloadPhase = "idle" | "running" | "complete" | "failed" | "cancelled";

interface DownloadProgress {
  receivedBytes: number;
  totalBytes: number;
}

// The one uncalibrated wording, shared by the download picker and the manual
// import result so a user never meets two phrasings for the same fact.
export const UNCALIBRATED_NOTICE =
  "Not in Pam's calibrated set — it may fail to load under this Mac's runtime profile.";

// The reason a preset is out of reach, naming both numbers: what the artifact
// needs, and what this Mac can devote to a model after the OS reserve.
export function tooLargeReason(expectedSizeBytes: number, hostModelBudgetBytes: number): string {
  return `Needs ${formatModelSize(expectedSizeBytes)}; this Mac can devote ${formatModelSize(hostModelBudgetBytes)} to a model.`;
}

// The curated path: pick a preset, review its license, download it. Pam
// verifies, hashes, and registers it once the bytes land — no terminal
// round-trip, and no floor warning here since every curated preset is
// comfortably large (>= 4.9 GB).
function PresetDownload({
  bridge,
  onImported,
  refreshTick = 0,
}: {
  bridge: PamBridge;
  onImported: () => void;
  /** Bumped by ⌘R; re-runs the mount-time fetch without a remount. */
  refreshTick?: number;
}) {
  const [presets, setPresets] = useState<ModelPresetDto[] | null>(null);
  const [hostModelBudgetBytes, setHostModelBudgetBytes] = useState<number | null>(null);
  const [hostMemory, setHostMemory] = useState<HostMemoryDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [accepted, setAccepted] = useState(false);
  const [phase, setPhase] = useState<DownloadPhase>("idle");
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const completedRef = useRef(false);

  // Reattach to whatever the daemon is already doing: the download manager
  // is single-flight and keeps running even if this component (or the whole
  // view) remounts, so a fresh mount always checks in before assuming idle.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [presetsResponse, memoryResponse, downloadStatus] = await Promise.all([
          bridge.modelPresets(withDaemonOperation()),
          bridge.hostMemory(withDaemonOperation()),
          bridge.modelDownloadStatus(withDaemonOperation()),
        ]);
        if (cancelled) return;
        setPresets(presetsResponse.presets);
        setHostModelBudgetBytes(presetsResponse.hostModelBudgetBytes);
        setHostMemory(memoryResponse);
        // Only a curated download belongs to this picker; a pasted-URL one
        // in flight is the other form's to reattach to.
        const mine = downloadStatus.downloadKind !== "url";
        if (mine && downloadStatus.downloadId) {
          setSelectedId(downloadStatus.downloadId);
          setAccepted(true);
        }
        if (mine && downloadStatus.status === "running") {
          setProgress({ receivedBytes: downloadStatus.receivedBytes, totalBytes: downloadStatus.totalBytes });
          setPhase("running");
        } else if (mine && downloadStatus.status === "complete" && downloadStatus.downloadId) {
          setProgress({ receivedBytes: downloadStatus.receivedBytes, totalBytes: downloadStatus.totalBytes });
          setPhase("complete");
          if (!completedRef.current) {
            completedRef.current = true;
            onImported();
          }
        } else if (mine && downloadStatus.status === "failed" && downloadStatus.downloadId) {
          setProgress({ receivedBytes: downloadStatus.receivedBytes, totalBytes: downloadStatus.totalBytes });
          setPhase("failed");
          setDownloadError(
            [downloadStatus.failure?.detail, downloadStatus.failure?.recovery].filter(Boolean).join(" ") ||
              "The download failed partway through.",
          );
        } else if (mine && downloadStatus.status === "cancelled" && downloadStatus.downloadId) {
          setProgress({ receivedBytes: downloadStatus.receivedBytes, totalBytes: downloadStatus.totalBytes });
          setPhase("cancelled");
        }
      } catch (error) {
        if (!cancelled) setLoadError(presentError(error));
      }
    })();
    return () => {
      cancelled = true;
    };
    // Reattachment runs on mount and again on each ⌘R tick; it never resets
    // picker state the user has already chosen.
  }, [bridge, refreshTick]);

  // Poll while a download is running; the daemon tracks one download at a
  // time and reports its own progress, so no local byte math is needed here.
  useEffect(() => {
    if (phase !== "running") return;
    let cancelled = false;
    const tick = async () => {
      try {
        const status = await bridge.modelDownloadStatus(withDaemonOperation());
        if (cancelled) return;
        setProgress({ receivedBytes: status.receivedBytes, totalBytes: status.totalBytes });
        if (status.status === "complete") {
          setPhase("complete");
          if (!completedRef.current) {
            completedRef.current = true;
            onImported();
          }
        } else if (status.status === "failed") {
          setPhase("failed");
          setDownloadError(
            [status.failure?.detail, status.failure?.recovery].filter(Boolean).join(" ") ||
              "The download failed partway through.",
          );
        } else if (status.status === "cancelled") {
          setPhase("cancelled");
        } else if (status.status === "idle") {
          setPhase("idle");
        }
      } catch (error) {
        if (!cancelled) {
          setPhase("failed");
          setDownloadError(presentError(error));
        }
      }
    };
    void tick();
    const interval = window.setInterval(() => void tick(), 800);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [phase, bridge, onImported]);

  const selected = presets?.find((preset) => preset.id === selectedId) ?? null;
  const fits = selected?.fitsHost ?? null;
  const busy = phase === "running";

  const startDownload = async () => {
    if (!selected || !accepted || busy) return;
    completedRef.current = false;
    setDownloadError(null);
    setProgress({ receivedBytes: 0, totalBytes: selected.expectedSizeBytes });
    setPhase("running");
    try {
      const response = await bridge.modelDownload(withDaemonOperation(), {
        kind: "preset",
        presetId: selected.id,
      });
      if (response.status !== "ok") {
        setPhase("failed");
        setDownloadError([response.failure.detail, response.failure.recovery].filter(Boolean).join(" "));
      }
    } catch (error) {
      setPhase("failed");
      setDownloadError(presentError(error));
    }
  };

  // Cancelling keeps the partial file on disk; the next start of the same
  // preset resumes from it, so the polled state settles via the daemon.
  const cancelDownload = async () => {
    try {
      const response = await bridge.modelDownloadCancel(withDaemonOperation());
      if (response.status !== "ok") {
        setDownloadError([response.failure.detail, response.failure.recovery].filter(Boolean).join(" "));
      }
    } catch (error) {
      setDownloadError(presentError(error));
    }
  };

  const percent = progress && progress.totalBytes > 0
    ? Math.min(100, Math.round((progress.receivedBytes / progress.totalBytes) * 100))
    : 0;

  return (
    <div className="model-presets">
      {loadError ? (
        <p className="model-verify is-fail" role="alert">{loadError}</p>
      ) : !presets ? (
        <p className="model-note">Looking at the curated models Pam can fetch for you…</p>
      ) : (
        <>
          {hostMemory !== null && hostMemory.totalBytes < hostMemory.supportedMinimumBytes && (
            <p className="model-fit-warn model-host-notice">
              Pam's local model is built for machines with {formatHostMemory(hostMemory.supportedMinimumBytes)} of
              memory or more; this Mac reports {formatHostMemory(hostMemory.totalBytes)}
              {hostModelBudgetBytes === null
                ? "."
                : `, leaving ${formatModelSize(hostModelBudgetBytes)} for a model after the OS reserve.`}
            </p>
          )}
          <MenuTrigger isOpen={pickerOpen && !busy} onOpenChange={(open) => !busy && setPickerOpen(open)}>
            <Button
              className="button button--secondary button--small model-preset-trigger"
              isDisabled={busy}
            >
              {selected ? selected.label : "Choose a model"}
              <CaretDown size={14} weight="bold" aria-hidden="true" />
            </Button>
            {busy && <small className="model-note">A download is already running — wait for it to finish before choosing another model.</small>}
            <Popover className="project-menu-popover model-preset-popover" placement="bottom start" offset={8}>
              <Menu
                className="menu-list"
                aria-label="Curated models"
                selectionMode="single"
                disallowEmptySelection
                selectedKeys={selectedId === null ? [] : [selectedId]}
                onSelectionChange={(keys) => {
                  if (busy) return;
                  for (const key of keys) {
                    setSelectedId(String(key));
                    setAccepted(false);
                    setPhase("idle");
                    setDownloadError(null);
                    setProgress(null);
                    return;
                  }
                }}
              >
                {presets.map((preset) => (
                  // A preset this Mac cannot run stays visible but
                  // unselectable, with the reason — never hidden, never
                  // downloadable behind a warning.
                  <MenuItem
                    key={preset.id}
                    id={preset.id}
                    className="project-menu-item model-preset-item"
                    textValue={preset.label}
                    isDisabled={!preset.fitsHost}
                  >
                    {({ isSelected }) => (<>
                      <span>
                        <strong>{preset.label}</strong>
                        <small>
                          {preset.paramsLabel} · {preset.quantLabel} · {formatModelSize(preset.expectedSizeBytes)} ·{" "}
                          {preset.licenseId}
                        </small>
                        <small className={preset.fitsHost ? undefined : "model-fit-warn"}>
                          {preset.fitsHost
                            ? "Runs on this Mac"
                            : hostModelBudgetBytes === null
                              ? "Checking this Mac's memory…"
                              : tooLargeReason(preset.expectedSizeBytes, hostModelBudgetBytes)}
                        </small>
                        {!preset.calibrated && <small className="model-fit-warn">{UNCALIBRATED_NOTICE}</small>}
                      </span>
                      {isSelected && <span className="menu-item-check"><Check size={15} weight="bold" aria-hidden="true" /></span>}
                    </>)}
                  </MenuItem>
                ))}
              </Menu>
            </Popover>
          </MenuTrigger>

          {selected && (
            <div className="model-preset-summary">
              <div className="model-identity">
                <strong title={selected.model}>{selected.model}</strong>
                <small>{formatModelSize(selected.expectedSizeBytes)} download · {selected.licenseId}</small>
              </div>
              <p className="model-note">{selected.licenseUrl}</p>
              <p className="model-note">{selected.licenseNoticeText}</p>
              {fits === false && hostModelBudgetBytes !== null && (
                <p className="model-fit-warn" role="status">
                  {tooLargeReason(selected.expectedSizeBytes, hostModelBudgetBytes)}
                </p>
              )}
              {!selected.calibrated && (
                <p className="model-fit-warn" role="status">{UNCALIBRATED_NOTICE}</p>
              )}
              <label className="model-import-consent">
                <input
                  type="checkbox"
                  checked={accepted}
                  disabled={busy}
                  onChange={(event) => setAccepted(event.target.checked)}
                />
                I accept the {selected.label} license exactly as stated above.
              </label>
              <div className="model-actions">
                <button
                  type="button"
                  className="button button--primary button--small"
                  disabled={!accepted || fits === false || busy || phase === "complete"}
                  onClick={() => void startDownload()}
                >
                  {phase === "running"
                    ? "Downloading…"
                    : phase === "complete"
                      ? "Downloaded"
                      : phase === "failed"
                        ? "Retry download"
                        : phase === "cancelled"
                          ? "Resume download"
                          : "Download"}
                </button>
                {busy && (
                  <button
                    type="button"
                    className="button button--secondary button--small"
                    onClick={() => void cancelDownload()}
                  >
                    Cancel
                  </button>
                )}
                {busy && <small>Pam fetches, hashes, and registers this model, all from this screen.</small>}
              </div>
              {phase === "running" && progress && (
                <div className="model-download-progress">
                  <div className="model-download-track" role="progressbar" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
                    <div className="model-download-fill" style={{ width: `${percent}%` }} />
                  </div>
                  <small>
                    {formatModelSize(progress.receivedBytes)} of {formatModelSize(progress.totalBytes)} · {percent}%
                  </small>
                </div>
              )}
              {phase === "complete" && (
                <p className="model-verify is-pass" role="status">
                  <Check size={16} aria-hidden="true" /> Downloaded and registered.
                </p>
              )}
              {phase === "cancelled" && progress && (
                <p className="model-note" role="status">
                  Download cancelled — {formatModelSize(progress.receivedBytes)} of{" "}
                  {formatModelSize(progress.totalBytes)} kept on disk. Resume picks up where it left off.
                </p>
              )}
              {phase === "failed" && downloadError && (
                <p className="model-verify is-fail" role="alert">{downloadError}</p>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}

// The vouching line, in one place: the pasted path has no hand-check behind
// it, and the digest is the only thing standing between the user and whatever
// the source sends.
export const PASTED_SOURCE_NOTICE =
  "Pam has not checked this source. By pasting it you are vouching for it, and the SHA-256 you " +
  "enter is what protects you: Pam refuses to register the file unless the bytes it receives " +
  "hash to exactly that digest.";

// The pasted path: a URL outside the curated catalog, with the same fields
// `pam model import` demands. It runs through the very same verified,
// resumable, cancellable download the presets use — only the source, and who
// vouched for it, differ.
function UrlDownload({
  bridge,
  onImported,
  refreshTick = 0,
}: {
  bridge: PamBridge;
  onImported: () => void;
  /** Bumped by ⌘R; re-runs the mount-time reattach without a remount. */
  refreshTick?: number;
}) {
  const [open, setOpen] = useState(false);
  const [form, setForm] = useState({
    model: "",
    url: "",
    expectedSizeBytes: "",
    sha256: "",
    licenseId: "",
    licenseUrl: "",
    licenseNoticeText: "",
  });
  const [accepted, setAccepted] = useState(false);
  const [phase, setPhase] = useState<DownloadPhase>("idle");
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  // True only between submitting the start and hearing back: the phase does
  // not become "running" until the desktop says the download actually
  // started, so a refusal is not overwritten by the polling effect.
  const [starting, setStarting] = useState(false);
  const completedRef = useRef(false);
  const busy = phase === "running" || starting;

  // Reattach to a pasted-URL download already in flight: the manager is
  // single-flight and survives a remount of this view, exactly like the
  // curated picker's own reattachment.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const status = await bridge.modelDownloadStatus(withDaemonOperation());
        if (cancelled || status.downloadKind !== "url" || !status.downloadId) return;
        setOpen(true);
        setForm((current) => (current.model.trim() === "" ? { ...current, model: status.downloadId ?? "" } : current));
        setProgress({ receivedBytes: status.receivedBytes, totalBytes: status.totalBytes });
        if (status.status === "running") setPhase("running");
        else if (status.status === "cancelled") setPhase("cancelled");
        else if (status.status === "complete") setPhase("complete");
        else if (status.status === "failed") {
          setPhase("failed");
          setDownloadError(
            [status.failure?.detail, status.failure?.recovery].filter(Boolean).join(" ") ||
              "The download failed partway through.",
          );
        }
      } catch {
        // Reattachment is an enhancement; the form still works without it.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [bridge, refreshTick]);

  // Poll while running, the same 800ms cadence the curated picker uses; the
  // desktop tracks one download at a time and reports its own progress.
  useEffect(() => {
    if (phase !== "running") return;
    let cancelled = false;
    const tick = async () => {
      try {
        const status = await bridge.modelDownloadStatus(withDaemonOperation());
        if (cancelled) return;
        setProgress({ receivedBytes: status.receivedBytes, totalBytes: status.totalBytes });
        if (status.status === "complete") {
          setPhase("complete");
          if (!completedRef.current) {
            completedRef.current = true;
            onImported();
          }
        } else if (status.status === "failed") {
          setPhase("failed");
          setDownloadError(
            [status.failure?.detail, status.failure?.recovery].filter(Boolean).join(" ") ||
              "The download failed partway through.",
          );
        } else if (status.status === "cancelled") {
          setPhase("cancelled");
        } else if (status.status === "idle") {
          setPhase("idle");
        }
      } catch (error) {
        if (!cancelled) {
          setPhase("failed");
          setDownloadError(presentError(error));
        }
      }
    };
    void tick();
    const interval = window.setInterval(() => void tick(), 800);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [phase, bridge, onImported]);

  const set = (field: keyof typeof form) => (value: string) =>
    setForm((current) => ({ ...current, [field]: value }));

  // Everything the form itself can check, named the way the desktop names it,
  // so a bad paste is corrected here instead of costing a round-trip.
  const localRefusal = (): string | null => {
    const missing = [
      form.model.trim() === "" && "model identity",
      form.url.trim() === "" && "download URL",
      form.expectedSizeBytes.trim() === "" && "expected size",
      form.sha256.trim() === "" && "SHA-256 digest",
      form.licenseId.trim() === "" && "license identifier",
      form.licenseUrl.trim() === "" && "license URL",
      form.licenseNoticeText.trim() === "" && "license notice",
    ].filter((field): field is string => Boolean(field));
    if (missing.length > 0) return `Fill in the ${missing.join(", ")} — Pam verifies every one of them.`;
    if (!/^[^/\s]+\/[^/\s]+$/.test(form.model.trim())) {
      return "Name the model as vendor/name, e.g. qwen/qwen3-4b-instruct-q4.";
    }
    if (!form.url.trim().startsWith("https://")) {
      return "Pam downloads models over HTTPS only. Paste the direct https:// URL of the .gguf file itself.";
    }
    if (!/^(sha256:)?[0-9a-fA-F]{64}$/.test(form.sha256.trim())) {
      return "The expected digest must be a 64-character hex SHA-256.";
    }
    if (!/^\d+$/.test(form.expectedSizeBytes.trim()) || Number(form.expectedSizeBytes.trim()) < 24) {
      return "The expected size must be the file's exact length in bytes.";
    }
    return null;
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    const refusal = localRefusal();
    if (refusal) {
      setPhase("failed");
      setDownloadError(refusal);
      return;
    }
    completedRef.current = false;
    setDownloadError(null);
    setProgress({ receivedBytes: 0, totalBytes: Number(form.expectedSizeBytes.trim()) });
    setStarting(true);
    try {
      const response = await bridge.modelDownload(withDaemonOperation(), {
        kind: "url",
        model: form.model.trim(),
        url: form.url.trim(),
        expectedSizeBytes: Number(form.expectedSizeBytes.trim()),
        sha256: form.sha256.trim().toLowerCase(),
        licenseId: form.licenseId.trim(),
        licenseUrl: form.licenseUrl.trim(),
        licenseNoticeText: form.licenseNoticeText,
        accepted,
      });
      if (response.status === "ok") {
        setPhase("running");
      } else {
        setPhase("failed");
        setDownloadError([response.failure.detail, response.failure.recovery].filter(Boolean).join(" "));
      }
    } catch (error) {
      setPhase("failed");
      setDownloadError(presentError(error));
    } finally {
      setStarting(false);
    }
  };

  // Cancelling keeps the partial file; restarting the same URL resumes from
  // it, exactly as it does for a preset.
  const cancelDownload = async () => {
    try {
      const response = await bridge.modelDownloadCancel(withDaemonOperation());
      if (response.status !== "ok") {
        setDownloadError([response.failure.detail, response.failure.recovery].filter(Boolean).join(" "));
      }
    } catch (error) {
      setDownloadError(presentError(error));
    }
  };

  const percent = progress && progress.totalBytes > 0
    ? Math.min(100, Math.round((progress.receivedBytes / progress.totalBytes) * 100))
    : 0;

  const field = (
    label: string,
    key: keyof typeof form,
    placeholder: string,
    type: "text" | "url" = "text",
  ) => (
    <label>
      {label}
      <input
        type={type}
        name={`model-url-${key}`}
        placeholder={placeholder}
        value={form[key]}
        disabled={busy}
        onChange={(event) => set(key)(event.target.value)}
      />
    </label>
  );

  return (
    <div className="model-advanced model-url-download">
      <button
        type="button"
        className="model-advanced-toggle"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        {open ? <CaretDown size={13} weight="bold" aria-hidden="true" /> : <CaretRight size={13} weight="bold" aria-hidden="true" />}
        Download from a URL you paste
      </button>
      {open && (
        <form
          className="model-import model-advanced-body"
          aria-label="Download from a URL you paste"
          onSubmit={(event) => void submit(event)}
        >
          <p className="model-fit-warn" role="note">{PASTED_SOURCE_NOTICE}</p>
          {field("Model identity", "model", "vendor/name, e.g. qwen/qwen3-4b-instruct-q4")}
          {field("Download URL", "url", "https://…/model.gguf", "url")}
          {field("Expected size in bytes", "expectedSizeBytes", "e.g. 17456012448")}
          {field("Expected SHA-256", "sha256", "64 hex characters")}
          {field("License identifier", "licenseId", "SPDX id, e.g. Apache-2.0")}
          {field("License URL", "licenseUrl", "https://…", "url")}
          <label>
            License notice
            <textarea
              name="model-url-notice"
              placeholder="Paste the exact license notice text you are accepting."
              rows={3}
              value={form.licenseNoticeText}
              disabled={busy}
              onChange={(event) => set("licenseNoticeText")(event.target.value)}
            />
          </label>
          <label className="model-import-consent">
            <input
              type="checkbox"
              checked={accepted}
              disabled={busy}
              onChange={(event) => setAccepted(event.target.checked)}
            />
            I accept this model's license exactly as stated above, and I vouch for this source.
          </label>
          <div className="model-actions">
            <button
              type="submit"
              className="button button--primary button--small"
              disabled={!accepted || busy || phase === "complete"}
            >
              {busy
                ? "Downloading…"
                : phase === "complete"
                  ? "Downloaded"
                  : phase === "failed"
                    ? "Retry download"
                    : phase === "cancelled"
                      ? "Resume download"
                      : "Download"}
            </button>
            {phase === "running" && (
              <button
                type="button"
                className="button button--secondary button--small"
                onClick={() => void cancelDownload()}
              >
                Cancel
              </button>
            )}
            {phase === "running" && (
              <small>Pam fetches, hashes, and registers this model, all from this screen.</small>
            )}
          </div>
          {phase === "running" && progress && (
            <div className="model-download-progress">
              <div className="model-download-track" role="progressbar" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
                <div className="model-download-fill" style={{ width: `${percent}%` }} />
              </div>
              <small>
                {formatModelSize(progress.receivedBytes)} of {formatModelSize(progress.totalBytes)} · {percent}%
              </small>
            </div>
          )}
          {phase === "complete" && (
            <p className="model-verify is-pass" role="status">
              <Check size={16} aria-hidden="true" /> Downloaded and registered.
            </p>
          )}
          {phase === "cancelled" && progress && (
            <p className="model-note" role="status">
              Download cancelled — {formatModelSize(progress.receivedBytes)} of{" "}
              {formatModelSize(progress.totalBytes)} kept on disk. Resume picks up where it left off.
            </p>
          )}
          {phase === "failed" && downloadError && (
            <p className="model-verify is-fail" role="alert">{downloadError}</p>
          )}
        </form>
      )}
    </div>
  );
}

// Canonical URL and notice sentence for the handful of SPDX IDs manual GGUF
// imports commonly declare. Unlisted IDs still prefill the license
// identifier alone (see runInspect below); only these get the URL and notice
// auto-filled too.
const KNOWN_SPDX_LICENSES: Record<string, string> = {
  "Apache-2.0": "https://www.apache.org/licenses/LICENSE-2.0",
  MIT: "https://opensource.org/license/mit",
  "BSD-3-Clause": "https://opensource.org/license/bsd-3-clause",
  "GPL-3.0": "https://www.gnu.org/licenses/gpl-3.0.html",
};

// GGUF metadata and Hugging Face license tags usually spell the id in
// lowercase; map onto the canonical SPDX key when one matches.
function canonicalSpdxId(licenseId: string): string {
  return (
    Object.keys(KNOWN_SPDX_LICENSES).find((id) => id.toLowerCase() === licenseId.toLowerCase()) ??
    licenseId
  );
}

// The manual path: point Pam at an already-downloaded GGUF. License fields
// collapse behind Advanced since most imports reuse the same license across
// re-imports; drag-drop still fills the path on the native shell.
function ManualImport({
  bridge,
  onImported,
  refreshTick = 0,
}: {
  bridge: PamBridge;
  onImported: () => void;
  /** Bumped by ⌘R; re-runs the mount-time reattach without a remount. */
  refreshTick?: number;
}) {
  const [form, setForm] = useState({
    model: "",
    path: "",
    licenseId: "",
    licenseUrl: "",
    licenseNoticeText: "",
  });
  const [accepted, setAccepted] = useState(false);
  const [allowSmall, setAllowSmall] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [importState, setImportState] = useState<ImportState>({ state: "idle" });
  // Set when an import completes against an artifact outside Pam's calibrated
  // set — the registration succeeded, but loading it is not a tested path.
  const [uncalibrated, setUncalibrated] = useState(false);
  const [inspect, setInspect] = useState<ModelInspectDto | null>(null);
  const inspectSequence = useRef(0);
  const busy = importState.state === "running";
  // Remembers the last identity runInspect wrote so a later inspection can
  // tell "still what we auto-filled" apart from "the user edited this" —
  // any manual edit to the model field clears it. The three license refs are
  // the same pattern, one per auto-fillable license field.
  const autoFilledModelRef = useRef<string | null>(null);
  const autoFilledLicenseIdRef = useRef<string | null>(null);
  const autoFilledLicenseUrlRef = useRef<string | null>(null);
  const autoFilledLicenseNoticeRef = useRef<string | null>(null);
  // Hugging Face license discovery, narrated: runs only when the GGUF's own
  // metadata declares no license, and quietly steps aside when it finds
  // nothing — manual entry always works.
  const [discovery, setDiscovery] = useState<
    | null
    | { state: "looking"; query: string }
    | { state: "found"; repoId: string; licenseId: string }
  >(null);
  // Reattach to an import the daemon is already hashing: the import manager
  // is single-flight and keeps running even if this component (or the whole
  // view) remounts, so a fresh mount checks in before assuming idle.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const status = await bridge.modelImportStatus(withDaemonOperation());
        if (cancelled || status.status !== "running") return;
        setImportState({
          state: "running",
          stage: status.stage ?? "hashing",
          hashedBytes: status.hashedBytes,
          totalBytes: status.totalBytes,
        });
      } catch {
        // Reattachment is an enhancement; the form still works without it.
      }
    })();
    return () => {
      cancelled = true;
    };
    // Reattachment runs on mount and again on each ⌘R tick; a running import
    // is picked up, everything else leaves the form untouched.
  }, [bridge, refreshTick]);

  // Poll while an import is running; the manager hashes in the background
  // (off the desktop command gate) and reports its own progress, exactly
  // like the download manager. A stale "idle" is ignored rather than
  // clearing state a concurrent submit may have just set.
  useEffect(() => {
    if (importState.state !== "running") return;
    let cancelled = false;
    const tick = async () => {
      try {
        const status = await bridge.modelImportStatus(withDaemonOperation());
        if (cancelled) return;
        if (status.status === "running") {
          setImportState({
            state: "running",
            stage: status.stage ?? "hashing",
            hashedBytes: status.hashedBytes,
            totalBytes: status.totalBytes,
          });
        } else if (status.status === "complete") {
          setImportState({ state: "idle" });
          setUncalibrated(status.calibrated === false);
          onImported();
        } else if (status.status === "failed") {
          setImportState({
            state: "fail",
            detail:
              [status.failure?.detail, status.failure?.recovery].filter(Boolean).join(" ") ||
              "The import failed partway through.",
          });
        }
      } catch (error) {
        if (!cancelled) setImportState({ state: "fail", detail: presentError(error) });
      }
    };
    void tick();
    const interval = window.setInterval(() => void tick(), 800);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [importState.state, bridge, onImported]);

  const set = (field: keyof typeof form) => (value: string) => {
    if (field === "model") autoFilledModelRef.current = null;
    if (field === "licenseId") autoFilledLicenseIdRef.current = null;
    if (field === "licenseUrl") autoFilledLicenseUrlRef.current = null;
    if (field === "licenseNoticeText") autoFilledLicenseNoticeRef.current = null;
    setForm((current) => ({ ...current, [field]: value }));
  };

  // Fires whenever a path lands (browse pick, drag-drop, or blur of a typed
  // path); prefills identity from what came back, but never overwrites text
  // the user already typed — an auto-filled value left over from a previous
  // path is fair game to refresh, since the user never typed it themselves.
  const runInspect = useCallback(
    async (path: string) => {
      const sequence = ++inspectSequence.current;
      try {
        const response = await bridge.modelInspect(withDaemonOperation(), path);
        if (sequence !== inspectSequence.current) return;
        setInspect(response);
        if (response.status === "ok" && response.architecture && response.modelName) {
          const slug = (value: string) => value.toLowerCase().replace(/[\s_]+/g, "-");
          const identity = `${slug(response.architecture)}/${slug(response.modelName)}`;
          setForm((current) => {
            const stillAutoFilled = !current.model.trim() || current.model === autoFilledModelRef.current;
            if (!stillAutoFilled) return current;
            autoFilledModelRef.current = identity;
            return { ...current, model: identity };
          });
        }
        // Same never-overwrite-the-user rule as identity above. The URL and
        // notice sentence are gated behind the license ID itself still being
        // auto-fillable: if the user already typed a different license ID,
        // filling in the detected file's URL/notice would describe the
        // wrong license, so both stay untouched too.
        const prefillLicense = (rawLicenseId: string, fileName: string) => {
          // GGUF metadata and Hugging Face tags usually carry the id in
          // lowercase ("apache-2.0"); canonicalize case-insensitively so the
          // known SPDX URL and notice prefill either way.
          const licenseId = canonicalSpdxId(rawLicenseId);
          const knownUrl = KNOWN_SPDX_LICENSES[licenseId];
          const notice = knownUrl ? `${fileName} is distributed under the ${licenseId} license at ${knownUrl}.` : null;
          setForm((current) => {
            const licenseIdStillAutoFilled =
              !current.licenseId.trim() || current.licenseId === autoFilledLicenseIdRef.current;
            if (!licenseIdStillAutoFilled) return current;
            const next = { ...current, licenseId };
            autoFilledLicenseIdRef.current = licenseId;
            if (knownUrl && (!current.licenseUrl.trim() || current.licenseUrl === autoFilledLicenseUrlRef.current)) {
              autoFilledLicenseUrlRef.current = knownUrl;
              next.licenseUrl = knownUrl;
            }
            if (
              notice &&
              (!current.licenseNoticeText.trim() || current.licenseNoticeText === autoFilledLicenseNoticeRef.current)
            ) {
              autoFilledLicenseNoticeRef.current = notice;
              next.licenseNoticeText = notice;
            }
            return next;
          });
        };
        if (response.status === "ok" && response.license) {
          setDiscovery(null);
          prefillLicense(response.license, response.fileName);
        } else if (response.status === "ok") {
          // The GGUF declares no license: ask Hugging Face, narrated. The
          // raw tag (e.g. "apache-2.0") maps case-insensitively onto the
          // known SPDX ids so the URL and notice prefill too.
          const query = response.modelName ?? response.fileName.replace(/\.gguf$/i, "");
          setDiscovery({ state: "looking", query });
          try {
            const found = await bridge.modelLicenseDiscover(withDaemonOperation(), query);
            if (sequence !== inspectSequence.current) return;
            if (found.status === "ok") {
              const canonical = canonicalSpdxId(found.licenseId);
              setDiscovery({ state: "found", repoId: found.repoId, licenseId: canonical });
              prefillLicense(canonical, response.fileName);
            } else {
              setDiscovery(null);
            }
          } catch {
            if (sequence === inspectSequence.current) setDiscovery(null);
          }
        }
      } catch {
        if (sequence === inspectSequence.current) setInspect(null);
      }
    },
    [bridge],
  );

  const setPath = (path: string) => {
    inspectSequence.current += 1; // invalidate any in-flight inspection
    setInspect(null);
    setDiscovery(null);
    setForm((current) => ({ ...current, path }));
  };

  const browse = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        filters: [{ name: "GGUF model", extensions: ["gguf"] }],
      });
      if (typeof selected === "string" && selected) {
        setPath(selected);
        void runInspect(selected);
      }
    } catch (error) {
      setInspect({
        status: "unavailable",
        failure: { kind: "unavailable", code: null, detail: presentError(error), recovery: "Type the GGUF path instead." },
      });
    }
  };

  // Dropping a GGUF from the file manager fills the path; native shell only.
  useEffect(() => {
    if (bridge.mode !== "native") return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void import("@tauri-apps/api/webview")
      .then(async ({ getCurrentWebview }) => {
        const stop = await getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type !== "drop") return;
          const dropped = event.payload.paths.find((path) => path.toLowerCase().endsWith(".gguf"));
          if (dropped) {
            setPath(dropped);
            void runInspect(dropped);
          }
        });
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // Drag and drop is an enhancement; the path field always works.
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [bridge]);

  const importPercent =
    importState.state === "running" && importState.totalBytes > 0
      ? Math.min(100, Math.round((importState.hashedBytes / importState.totalBytes) * 100))
      : 0;

  const missingLicenseFields = [
    form.licenseId.trim() === "" && "license identifier",
    form.licenseUrl.trim() === "" && "license URL",
    form.licenseNoticeText.trim() === "" && "license notice",
  ].filter((field): field is string => Boolean(field));
  const ready = accepted && form.path.trim() !== "" && form.model.trim() !== "";

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!ready || busy) return;
    // Missing license details are the one silent blocker: reveal them and
    // say so instead of a dead disabled button.
    if (missingLicenseFields.length > 0) {
      setAdvancedOpen(true);
      setImportState({
        state: "fail",
        detail: `Fill in the ${missingLicenseFields.join(", ")} under Advanced — Pam records exactly which license you accept.`,
      });
      return;
    }
    setImportState({ state: "running", stage: "hashing", hashedBytes: 0, totalBytes: 0 });
    setUncalibrated(false);
    const params: ModelImportParams = {
      model: form.model.trim(),
      path: form.path.trim(),
      licenseId: form.licenseId.trim(),
      licenseUrl: form.licenseUrl.trim(),
      licenseNoticeText: form.licenseNoticeText,
      allowSmall,
    };
    try {
      // The starter returns immediately; the polling effect above carries
      // the run through hashing and registration to its terminal state.
      const response = await bridge.modelImport(withDaemonOperation(), params);
      if (response.status !== "ok") {
        setImportState({
          state: "fail",
          detail: [response.failure.detail, response.failure.recovery].filter(Boolean).join(" "),
        });
      }
    } catch (error) {
      setImportState({ state: "fail", detail: presentError(error) });
    }
  };

  const field = (
    label: string,
    key: keyof typeof form,
    placeholder: string,
    type: "text" | "url" = "text",
  ) => (
    <label>
      {label}
      <input
        type={type}
        name={`model-import-${key}`}
        placeholder={placeholder}
        value={form[key]}
        disabled={busy}
        onChange={(event) => set(key)(event.target.value)}
      />
    </label>
  );

  return (
    <form className="model-import" onSubmit={(event) => void submit(event)}>
      <div className="model-import-path">
        <label>
          GGUF file path
          <input
            type="text"
            name="model-import-path"
            placeholder="/absolute/path/to/model.gguf"
            value={form.path}
            disabled={busy}
            onChange={(event) => setPath(event.target.value)}
            onBlur={(event) => {
              const trimmed = event.target.value.trim();
              if (trimmed) void runInspect(trimmed);
            }}
          />
        </label>
        {bridge.mode === "native" && (
          <button
            type="button"
            className="button button--secondary button--small"
            disabled={busy}
            onClick={() => void browse()}
          >
            Browse…
          </button>
        )}
      </div>
      {inspect?.status === "ok" && (
        <div className="model-identity">
          <strong title={inspect.fileName}>{inspect.fileName}</strong>
          <small>
            {formatModelSize(inspect.sizeBytes)}
            {inspect.architecture && inspect.modelName
              ? ` · ${inspect.architecture} · ${inspect.modelName}`
              : ""}
          </small>
          {inspect.belowFloor && (
            <p className="model-fit-warn" role="status">
              Below Pam's recommended minimum of {formatModelSize(inspect.floorBytes)} — override this
              under Advanced if you want to import it anyway.
            </p>
          )}
        </div>
      )}
      {inspect && inspect.status !== "ok" && (
        <p className="model-note">{inspect.failure.detail}</p>
      )}
      {discovery?.state === "looking" && (
        <p className="model-note" role="status">
          This file declares no license — looking up “{discovery.query}” on Hugging Face…
        </p>
      )}
      {discovery?.state === "found" && (
        <p className="model-note" role="status">
          License found on Hugging Face ({discovery.repoId}): {discovery.licenseId}. Review the
          prefilled details below before accepting.
        </p>
      )}
      {field("Model identity", "model", "vendor/name, e.g. qwen/qwen3-4b-instruct-q4")}
      <div className="model-advanced">
        <button
          type="button"
          className="model-advanced-toggle"
          aria-expanded={advancedOpen}
          onClick={() => setAdvancedOpen((current) => !current)}
        >
          {advancedOpen ? <CaretDown size={13} weight="bold" aria-hidden="true" /> : <CaretRight size={13} weight="bold" aria-hidden="true" />}
          Advanced — license details
        </button>
        {advancedOpen && (
          <div className="model-advanced-body">
            {field("License identifier", "licenseId", "SPDX id, e.g. Apache-2.0")}
            {field("License URL", "licenseUrl", "https://…", "url")}
            <label>
              License notice
              <textarea
                name="model-import-notice"
                placeholder="Paste the exact license notice text you are accepting."
                rows={3}
                value={form.licenseNoticeText}
                disabled={busy}
                onChange={(event) => set("licenseNoticeText")(event.target.value)}
              />
            </label>
            <label className="model-import-consent">
              <input
                type="checkbox"
                checked={allowSmall}
                disabled={busy}
                onChange={(event) => setAllowSmall(event.target.checked)}
              />
              Allow a model below Pam's recommended minimum size (results will fall short in real flows).
            </label>
          </div>
        )}
      </div>
      <label className="model-import-consent">
        <input
          type="checkbox"
          checked={accepted}
          disabled={busy}
          onChange={(event) => setAccepted(event.target.checked)}
        />
        I accept this model's license exactly as stated above.
      </label>
      <div className="model-actions">
        <button type="submit" className="button button--primary button--small" disabled={!ready || busy}>
          {busy ? "Verifying and registering…" : "Import model"}
        </button>
        {busy && <small>Pam reads and hashes the whole file in the background; the rest of the app stays responsive.</small>}
      </div>
      {importState.state === "running" && importState.stage === "hashing" && importState.totalBytes > 0 && (
        <div className="model-download-progress">
          <div
            className="model-download-track"
            role="progressbar"
            aria-valuenow={importPercent}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <div className="model-download-fill" style={{ width: `${importPercent}%` }} />
          </div>
          <small>
            Hashing {formatModelSize(importState.hashedBytes)} of {formatModelSize(importState.totalBytes)} · {importPercent}%
          </small>
        </div>
      )}
      {importState.state === "running" && importState.stage === "registering" && (
        <p className="model-note" role="status">Registering — verifying the copy…</p>
      )}
      {importState.state === "fail" && (
        <p className="model-verify is-fail" role="alert">{importState.detail}</p>
      )}
      {uncalibrated && (
        <p className="model-fit-warn" role="status">
          Registered, but this artifact is not in Pam's calibrated set — it may fail to load under
          this Mac's runtime profile. The calibrated presets in the download list above are the
          tested path.
        </p>
      )}
    </form>
  );
}

// The whole setup happens here: a curated preset Pam downloads for you, or
// point Pam at a GGUF you already have — no terminal round-trip either way.
function ModelImportForm({
  bridge,
  onImported,
  registered,
  refreshTick = 0,
}: {
  bridge: PamBridge;
  onImported: () => void;
  /** How many models are already registered; only the copy depends on it. */
  registered: number;
  refreshTick?: number;
}) {
  return (
    <div className="model-runtime model-setup">
      <p className="model-note">
        {registered === 0
          ? "No local model is registered yet. Choose a curated model for Pam to download, or import one you already have."
          : "Add another model: choose a curated one for Pam to download, or import one you already have. Registering a model never disturbs the one running."}
      </p>
      <p className="model-note">
        Every model in the list below is hand-checked: Pam ships its URL, size and digest as
        checked-in constants and confines its download to the publisher's own hosts.
      </p>
      <PresetDownload bridge={bridge} onImported={onImported} refreshTick={refreshTick} />
      <div className="model-setup-divider" role="separator">
        <span>or paste a download URL</span>
      </div>
      <UrlDownload bridge={bridge} onImported={onImported} refreshTick={refreshTick} />
      <div className="model-setup-divider" role="separator">
        <span>or import a downloaded GGUF</span>
      </div>
      <ManualImport bridge={bridge} onImported={onImported} refreshTick={refreshTick} />
    </div>
  );
}

/** Elapsed wall time for the verification phase, which has no byte signal. */
export function formatElapsed(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  return whole < 60 ? `${whole}s` : `${Math.floor(whole / 60)}m ${String(whole % 60).padStart(2, "0")}s`;
}

// What a weights check found, in the words a person reading the screen needs.
// The registry recorded a path, a size, a SHA-256 and a GGUF header; each label
// names exactly which of those stopped matching, so a failing row is never
// just "bad".
const WEIGHTS_HEALTH_LABELS: Record<ModelHealthDto["health"], string> = {
  ok: "Weights match the registry",
  path_missing: "Weights missing — nothing at the registered path",
  size_mismatch: "Weights resized since registration",
  digest_mismatch: "Weights changed since registration",
  metadata_mismatch: "Weights are no longer the registered GGUF",
  unsafe_path: "Weights path is no longer a plain file Pam can read",
  unreadable: "Weights could not be read",
};

export interface ModelPanelProps {
  bridge: PamBridge;
  daemon: DaemonView;
  modelStatus: ModelStatusDto | null;
  /** True while a daemon lifecycle command is in flight. */
  modelBusy: boolean;
  onOpenModelChat: (modelId: string, returnFocusTarget?: HTMLElement) => void;
  /** Brings a model into Pam: a plain load when it is already running, and a
   *  start carrying the model when it is not. */
  onStartWithModel: (modelId: string) => void;
  /** The model was dropped from the running daemon; the surface needs
   *  re-reading. */
  onModelUnloaded: (modelId: string) => void;
  onModelImported: () => void;
  /** A registration left the registry: the catalog needs re-reading. */
  onModelUnregistered: (modelId: string) => void;
  /** Weights were deleted, which also unregistered the model. */
  onModelWeightsDeleted: (modelId: string, bytesReclaimed: number) => void;
  /** Bumped by ⌘R; forwarded to the setup form's mount-time loaders. */
  refreshTick?: number;
}

// The local model runtime, one panel on the launch view: state, identity,
// a live verification round-trip, and the path from "no model" to "loaded".
export function ModelPanel({
  bridge,
  daemon,
  modelStatus,
  modelBusy,
  onOpenModelChat,
  onStartWithModel,
  onModelUnloaded,
  onModelImported,
  onModelUnregistered,
  onModelWeightsDeleted,
  refreshTick = 0,
}: ModelPanelProps) {
  const [verify, setVerify] = useState<VerifyState>({ state: "idle" });
  const [startup, setStartup] = useState<DaemonStartupProgressDto | null>(null);
  const verifySequence = useRef(0);
  // A daemon that is up but still hashing and mapping its model answers
  // nothing at all, so health reads unreachable while the load runs. The
  // desktop reports that phase from the process it started; it outranks the
  // health read, which cannot tell "loading" from "gone".
  const loading = modelStatus?.status === "ok" && modelStatus.loading;
  const offline = daemon.state === "stopped" && !loading;
  const loaded = modelStatus?.status === "ok" ? modelStatus.loaded : null;
  const registered = modelStatus?.status === "ok" ? modelStatus.registered : [];
  // The daemon keeps serving when its model fails to load, and keeps saying
  // why for as long as it runs — a 2.6 s toast is not a report. A paused
  // daemon has no live reason to show.
  const loadFailure = !offline && modelStatus?.status === "ok" ? modelStatus.loadFailure : null;

  // A verify result only ever applies to the model it measured; once the
  // loaded model changes (e.g. a restart with a different one), the stale
  // pass/fail line must not keep rendering next to the new identity.
  useEffect(() => {
    verifySequence.current += 1;
    setVerify({ state: "idle" });
  }, [loaded?.modelId]);
  // A running daemon loads in place; only a paused one needs starting. The
  // copy stops promising a restart the moment there is nothing to restart.
  const loadLabel = offline ? "Start Pam with this model" : "Load";
  // The daemon's own load or unload, distinct from the startup load the
  // desktop infers above: this is the running daemon changing models.
  const transition = !offline && modelStatus?.status === "ok" ? modelStatus.transition : null;

  // A start holds the desktop command gate for the whole load, so every other
  // panel read is stuck behind it and the panel cannot tell a live eight-minute
  // load from a hang. The desktop samples the spawned daemon's resident memory
  // off that gate; there is no event bus here, so the GUI polls it.
  const starting = modelBusy || loading;
  useEffect(() => {
    if (!starting) {
      setStartup(null);
      return;
    }
    let cancelled = false;
    const tick = async () => {
      try {
        const progress = await bridge.daemonStartupProgress(withDaemonOperation());
        if (!cancelled) setStartup(progress.modelId ? progress : null);
      } catch {
        // A start that cannot be metered still shows its plain "starting" line.
        if (!cancelled) setStartup(null);
      }
    };
    void tick();
    const interval = window.setInterval(() => void tick(), 800);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [bridge, starting]);

  // Resident memory covers mapped weights, and the runtime releases pages as
  // it settles, so it never accounts for the whole artifact: the bar is capped
  // short of 100% and only the daemon's own "loaded" report ends the wait.
  const startupPercent = startup && startup.totalBytes > 0
    ? Math.min(99, Math.round((startup.loadedBytes / startup.totalBytes) * 100))
    : 0;

  const runVerify = async (modelId: string) => {
    const sequence = ++verifySequence.current;
    setVerify({ state: "running" });
    const startedAt = performance.now();
    try {
      const response = await bridge.modelInfer(
        withDaemonOperation(),
        modelId,
        [{ role: "user", content: "Reply with a single word: ready." }],
        16,
      );
      // A response that outlived its model (the loaded model changed while
      // the round-trip was in flight) must not credit the new identity.
      if (sequence !== verifySequence.current) return;
      const ms = Math.round(performance.now() - startedAt);
      if (response.status === "ok") {
        setVerify({ state: "pass", ms, tokens: response.usage.emittedOutputTokens });
      } else {
        setVerify({
          state: "fail",
          detail: [response.failure.detail, response.failure.recovery].filter(Boolean).join(" "),
        });
      }
    } catch (error) {
      if (sequence !== verifySequence.current) return;
      setVerify({ state: "fail", detail: presentError(error) });
    }
  };

  // Unregistering is destructive to durable state, so it is confirmed in the
  // row itself: one row at a time holds the confirmation, and a refusal stays
  // next to the model it refused.
  const [confirmingUnregister, setConfirmingUnregister] = useState<string | null>(null);
  const [unregisterBusy, setUnregisterBusy] = useState<string | null>(null);
  const [unregisterError, setUnregisterError] = useState<{ modelId: string; detail: string } | null>(null);

  const unregisterModel = async (modelId: string) => {
    setConfirmingUnregister(null);
    setUnregisterBusy(modelId);
    setUnregisterError(null);
    try {
      const response = await bridge.modelUnregister(withDaemonOperation(), modelId);
      if (response.status === "ok") {
        onModelUnregistered(response.model);
      } else {
        setUnregisterError({
          modelId,
          detail: [response.failure.detail, response.failure.recovery].filter(Boolean).join(" "),
        });
      }
    } catch (error) {
      setUnregisterError({ modelId, detail: presentError(error) });
    } finally {
      setUnregisterBusy(null);
    }
  };

  // Unloading frees the model's memory and leaves Pam serving. Nothing
  // durable goes — the registration and the weights both stay — so unlike
  // unregistering it needs no in-row confirmation, only its own busy state
  // and a place for a refusal to land.
  const [unloadBusy, setUnloadBusy] = useState(false);
  const [unloadFailure, setUnloadFailure] = useState<string | null>(null);

  const unloadModel = async () => {
    setUnloadBusy(true);
    setUnloadFailure(null);
    try {
      const response = await bridge.modelUnload(withDaemonOperation());
      if (response.status === "ok") {
        onModelUnloaded(response.model);
      } else {
        setUnloadFailure([response.failure.detail, response.failure.recovery].filter(Boolean).join(" "));
      }
    } catch (error) {
      setUnloadFailure(presentError(error));
    } finally {
      setUnloadBusy(false);
    }
  };

  // Which model a Pam start loads when nothing else is asked for. Read once
  // per mount and after every change, from the same Settings file the daemon
  // reads at start.
  const [defaultModel, setDefaultModel] = useState<string | null>(null);
  const [pinBusy, setPinBusy] = useState<string | null>(null);
  const [pinFailure, setPinFailure] = useState<{ modelId: string; detail: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const settings = await bridge.appSettings(withDaemonOperation());
        if (!cancelled) setDefaultModel(settings.defaultModel);
      } catch {
        // A pin that cannot be read leaves every row unpinned rather than
        // claiming a default Pam might not actually start with.
        if (!cancelled) setDefaultModel(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [bridge, refreshTick]);

  // `modelId` is the row that asked, so a refusal lands next to it; `pin` is
  // what to persist, which is null when that row is clearing its own pin.
  const setDefault = async (modelId: string, pin: string | null) => {
    setPinBusy(modelId);
    setPinFailure(null);
    try {
      const settings = await bridge.settingsSetDefaultModel(withDaemonOperation(), pin);
      setDefaultModel(settings.defaultModel);
    } catch (error) {
      setPinFailure({ modelId, detail: presentError(error) });
    } finally {
      setPinBusy(null);
    }
  };

  // The pin decides which model a later Pam start loads. It is the same
  // control wherever a model appears — a catalog row, or the loaded identity
  // above it — because the model you are running is the one you are most
  // likely to want back next time.
  const defaultModelPin = (modelId: string) => {
    const pinned = defaultModel === modelId;
    return (
      <button
        type="button"
        className="button button--secondary button--small"
        aria-pressed={pinned}
        disabled={pinBusy !== null}
        onClick={() => void setDefault(modelId, pinned ? null : modelId)}
      >
        {pinned ? <PushPinSlash size={17} /> : <PushPin size={17} />}{" "}
        {pinned ? "Don't start with this" : "Start with this"}
      </button>
    );
  };

  // Registry health is a different question from the model check above: that
  // one asks the loaded model to answer, this one asks whether the weights on
  // disk are still the bytes the registry recorded. It re-hashes the whole
  // artifact, so it never runs on its own — a row is checked when asked.
  const [weightsHealth, setWeightsHealth] = useState<Record<string, ModelHealthDto>>({});
  const [weightsBusy, setWeightsBusy] = useState<string | null>(null);
  const [weightsFailure, setWeightsFailure] = useState<{ modelId: string | null; detail: string } | null>(null);
  const [sweep, setSweep] = useState<ModelSweepDto | null>(null);

  const checkWeights = async (modelId?: string) => {
    setWeightsBusy(modelId ?? "*");
    setWeightsFailure(null);
    try {
      const response = await bridge.modelVerify(withDaemonOperation(), modelId);
      if (response.status === "ok") {
        setWeightsHealth((previous) => {
          const next = { ...previous };
          for (const model of response.models) next[model.model] = model;
          return next;
        });
      } else {
        setWeightsFailure({
          modelId: modelId ?? null,
          detail: [response.failure.detail, response.failure.recovery].filter(Boolean).join(" "),
        });
      }
    } catch (error) {
      setWeightsFailure({ modelId: modelId ?? null, detail: presentError(error) });
    } finally {
      setWeightsBusy(null);
    }
  };

  // The sweep only stats the models directory, so unlike a weights check it is
  // cheap enough to run whenever this panel opens.
  useEffect(() => {
    if (offline) {
      setSweep(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const response = await bridge.modelSweep(withDaemonOperation());
        if (!cancelled) setSweep(response);
      } catch {
        // A sweep that cannot run leaves the section out rather than
        // replacing the catalog with an error.
        if (!cancelled) setSweep(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [bridge, offline, refreshTick]);

  // Deleting weights removes bytes and unregisters in one operation, so it is
  // confirmed in the row exactly like unregistering, and only ever offered for
  // a row Pam has just re-read and found to be its own download.
  const [confirmingDelete, setConfirmingDelete] = useState<string | null>(null);
  const [deleteBusy, setDeleteBusy] = useState<string | null>(null);

  const deleteWeights = async (modelId: string) => {
    setConfirmingDelete(null);
    setDeleteBusy(modelId);
    setWeightsFailure(null);
    try {
      const response = await bridge.modelDeleteWeights(withDaemonOperation(), modelId);
      if (response.status === "ok") {
        onModelWeightsDeleted(response.model, response.bytesReclaimed);
      } else {
        setWeightsFailure({
          modelId,
          detail: [response.failure.detail, response.failure.recovery].filter(Boolean).join(" "),
        });
      }
    } catch (error) {
      setWeightsFailure({ modelId, detail: presentError(error) });
    } finally {
      setDeleteBusy(null);
    }
  };

  const revealWeights = async (path: string) => {
    try {
      await bridge.revealPath(withDaemonOperation(), path);
    } catch (error) {
      setWeightsFailure({ modelId: null, detail: presentError(error) });
    }
  };

  const pill = loading
    ? { label: "loading", tone: "elevated" }
    : offline || !modelStatus || modelStatus.status !== "ok"
    ? { label: offline ? "unreachable" : !modelStatus ? "checking" : "unreachable", tone: offline || modelStatus ? "attention" : "not-reported" }
    : loaded
      ? { label: "loaded", tone: "healthy" }
      : registered.length > 0
        ? { label: "on deck", tone: "observed" }
        : { label: "none", tone: "not-reported" };

  // One catalog row, and the home of every per-model action: loading it, the
  // "start with this" pin, the weights check, whichever disposal the
  // provenance gate allows, and unregistering.
  const catalogRow = (model: { modelId: string; sizeBytes: number }) => {
    const health = weightsHealth[model.modelId];
    const rowFailure = weightsFailure?.modelId === model.modelId ? weightsFailure.detail : null;
    const pinned = defaultModel === model.modelId;
    return (
      <article key={model.modelId}>
        <span className="access-icon"><Brain size={21} /></span>
        <div>
          <strong title={model.modelId}>{model.modelId}</strong>
          <p>{formatModelSize(model.sizeBytes)} on disk</p>
          {pinned && <p className="model-note">Pam starts with this model.</p>}
          {transition?.model === model.modelId && (
            <p className="model-note" role="status">
              {transition.phase === "loading" ? "Loading…" : "Unloading…"}
            </p>
          )}
          {pinFailure?.modelId === model.modelId && (
            <p className="model-verify is-fail" role="alert">{pinFailure.detail}</p>
          )}
          {health && (
            health.health === "ok" ? (
              <p className="model-verify is-pass" role="status">
                <Check size={16} aria-hidden="true" /> {WEIGHTS_HEALTH_LABELS.ok}
              </p>
            ) : (
              <p className="model-verify is-fail" role="alert">
                {WEIGHTS_HEALTH_LABELS[health.health]}
                {health.detail ? ` — ${health.detail}` : ""}
              </p>
            )
          )}
          {unregisterError?.modelId === model.modelId && (
            <p className="model-verify is-fail" role="alert">{unregisterError.detail}</p>
          )}
          {rowFailure && <p className="model-verify is-fail" role="alert">{rowFailure}</p>}
        </div>
        <div className="model-row-actions">
          <button
            type="button"
            className="button button--secondary button--small"
            disabled={modelBusy || transition !== null}
            onClick={() => onStartWithModel(model.modelId)}
          >
            <Power size={17} /> {loadLabel}
          </button>
          {/* The pin is a preference, not an action on the daemon: clicking a
              pinned row clears it, so Pam can be told to start with no model
              at all. */}
          {defaultModelPin(model.modelId)}
          <button
            type="button"
            className="button button--secondary button--small"
            disabled={weightsBusy !== null}
            onClick={() => void checkWeights(model.modelId)}
          >
            <Stethoscope size={17} /> {weightsBusy === model.modelId ? "Checking weights…" : "Check weights"}
          </button>
          {/* Deleting weights is offered only for a file Pam downloaded into its
              own models directory, and only after the check that proved it. A
              GGUF Pam verified in place belongs to whoever put it there: the
              honest offer for that row is to show them where it is. */}
          {health && health.weightsDeletable && (
            confirmingDelete === model.modelId ? (
              <span className="connector-confirm">
                Delete {formatModelSize(health.sizeBytes)} of weights at {health.path} and unregister {model.modelId}?
                <button
                  type="button"
                  className="button button--secondary button--small"
                  disabled={deleteBusy === model.modelId}
                  onClick={() => void deleteWeights(model.modelId)}
                >
                  Delete weights
                </button>
                <button
                  type="button"
                  className="button button--secondary button--small"
                  onClick={() => setConfirmingDelete(null)}
                >
                  Keep
                </button>
              </span>
            ) : (
              <button
                type="button"
                className="button button--secondary button--small"
                disabled={deleteBusy === model.modelId}
                onClick={() => { setWeightsFailure(null); setConfirmingDelete(model.modelId); }}
              >
                <Trash size={17} /> Delete weights
              </button>
            )
          )}
          {health && !health.weightsDeletable && (
            <button
              type="button"
              className="button button--secondary button--small"
              onClick={() => void revealWeights(health.path)}
            >
              <FolderOpen size={17} /> Reveal in Finder
            </button>
          )}
          {confirmingUnregister === model.modelId ? (
            <span className="connector-confirm">
              Remove {model.modelId} from Pam&apos;s registry? The GGUF file stays on disk.
              <button
                type="button"
                className="button button--secondary button--small"
                disabled={unregisterBusy === model.modelId}
                onClick={() => void unregisterModel(model.modelId)}
              >
                Unregister
              </button>
              <button
                type="button"
                className="button button--secondary button--small"
                onClick={() => setConfirmingUnregister(null)}
              >
                Keep
              </button>
            </span>
          ) : (
            <button
              type="button"
              className="button button--secondary button--small"
              disabled={unregisterBusy === model.modelId}
              onClick={() => { setUnregisterError(null); setConfirmingUnregister(model.modelId); }}
            >
              <Trash size={17} /> Unregister
            </button>
          )}
        </div>
      </article>
    );
  };

  // What the models directory holds that the registry cannot account for, and
  // the other way round. Reporting only: clearing a dangling row is
  // unregistering it, and an orphaned file is the user's to remove unless Pam
  // downloaded it, in which case its own row offers Delete weights.
  const weightsOnDisk = sweep?.status === "ok" ? sweep : null;
  const sweepSection = weightsOnDisk && (
    <section className="model-runtime" aria-labelledby="model-sweep-heading">
      <div className="model-identity">
        <strong id="model-sweep-heading">Weights on disk</strong>
        <small>
          {weightsOnDisk.modelsDir} · {formatModelSize(weightsOnDisk.totalBytes)} in this directory
        </small>
      </div>
      {weightsOnDisk.dangling.length === 0 && weightsOnDisk.orphans.length === 0 ? (
        <p className="model-note">
          Every registered model points at a file that is there, and every GGUF in this directory is
          registered.
        </p>
      ) : (
        <div className="access-list model-rows">
          {weightsOnDisk.dangling.map((row) => (
            <article key={`dangling-${row.model}`}>
              <span className="access-icon"><Brain size={21} /></span>
              <div>
                <strong title={row.model}>{row.model}</strong>
                <p>
                  Registered at {row.path}, but nothing is there ·{" "}
                  {formatModelSize(row.sizeBytes)} recorded
                </p>
              </div>
              <div className="model-row-actions">
                <button
                  type="button"
                  className="button button--secondary button--small"
                  disabled={unregisterBusy === row.model}
                  onClick={() => void unregisterModel(row.model)}
                >
                  <Trash size={17} /> Unregister
                </button>
              </div>
            </article>
          ))}
          {weightsOnDisk.orphans.map((orphan) => (
            <article key={`orphan-${orphan.path}`}>
              <span className="access-icon"><Brain size={21} /></span>
              <div>
                <strong title={orphan.path}>{orphan.path}</strong>
                <p>No registry entry points at this file · {formatModelSize(orphan.sizeBytes)}</p>
              </div>
              <div className="model-row-actions">
                <span className="state-pill state-pill--not-reported">not registered</span>
              </div>
            </article>
          ))}
        </div>
      )}
      {weightsFailure?.modelId === null && (
        <p className="model-verify is-fail" role="alert">{weightsFailure.detail}</p>
      )}
    </section>
  );

  const sweepFailure = sweep && sweep.status !== "ok"
    ? [sweep.failure.detail, sweep.failure.recovery].filter(Boolean).join(" ")
    : null;
  return (
    <section className="panel model-panel" aria-labelledby="model-panel-heading">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Local model</span>
          <h2 id="model-panel-heading">Model runtime</h2>
        </div>
        <span className={`state-pill state-pill--${pill.tone}`}>{pill.label}</span>
      </div>
      {/* Loose content inside a panel needs the panel's own gutter; the model
          rows below get it from `.model-runtime`, so these lines share it. */}
      {(loadFailure || startup?.phase) && (
        <div className="model-runtime">
          {loadFailure && <p className="model-verify is-fail" role="alert">{loadFailure}</p>}
          {startup?.phase === "verifying" && (
            <p className="model-note" role="status">
              Checking {startup.modelId} — verifying the artifact's integrity, {formatElapsed(startup.elapsedSeconds)} so far.
              Loading starts once the whole file is hashed.
            </p>
          )}
          {startup?.phase === "loading" && (
            <div className="model-download-progress">
              <div
                className="model-download-track"
                role="progressbar"
                aria-label="Model load progress"
                aria-valuenow={startupPercent}
                aria-valuemin={0}
                aria-valuemax={100}
              >
                <div className="model-download-fill" style={{ width: `${startupPercent}%` }} />
              </div>
              <small>
                Loading {startup.modelId} — {formatModelSize(startup.loadedBytes)} of{" "}
                {formatModelSize(startup.totalBytes)} in memory · {formatElapsed(startup.elapsedSeconds)}
              </small>
            </div>
          )}
        </div>
      )}
      {offline ? (
        registered.length > 0 ? (
          <div className="model-runtime">
            <p className="model-note">
              Pam is paused, so nothing is loaded right now. Start it with a registered model to
              chat and verify.
            </p>
            <div className="access-list model-rows">{registered.map(catalogRow)}</div>
          </div>
        ) : (
          <PanelEmpty>Pam is paused, so the local model runtime is not reachable. Start Pam to check on it.</PanelEmpty>
        )
      ) : loading ? (
        <PanelLoading>
          Pam is starting: the model is still loading. Checking and loading a large model takes a
          few minutes, and this panel updates when it finishes.
        </PanelLoading>
      ) : !modelStatus ? (
        <PanelEmpty>Checking the local model…</PanelEmpty>
      ) : modelStatus.status !== "ok" ? (
        <PanelEmpty>
          {[modelStatus.failure.detail, modelStatus.failure.recovery].filter(Boolean).join(" ")}
        </PanelEmpty>
      ) : loaded ? (
        <div className="model-runtime">
          <div className="model-identity">
            <strong title={loaded.modelId}>{loaded.modelId}</strong>
            <small>{formatModelSize(loaded.sizeBytes)} on disk · loads fully into memory</small>
          </div>
          <div className="model-actions">
            <button
              type="button"
              className="button button--secondary button--small"
              disabled={verify.state === "running"}
              onClick={() => void runVerify(loaded.modelId)}
            >
              {verify.state === "running" ? "Verifying…" : "Verify"}
            </button>
            <button
              type="button"
              className="button button--primary button--small"
              onClick={(event) => onOpenModelChat(loaded.modelId, event.currentTarget)}
            >
              Chat
            </button>
            {/* Unloading returns the model's memory and leaves Pam serving.
                The registration and the weights both stay, so this is not a
                disposal: the same model loads again from its catalog row. */}
            <button
              type="button"
              className="button button--secondary button--small"
              disabled={unloadBusy || transition !== null}
              onClick={() => void unloadModel()}
            >
              <Eject size={17} /> {unloadBusy ? "Unloading…" : "Unload"}
            </button>
            {/* The loaded model has no catalog row of its own below, so its
                pin lives here: the model you are running is the one you are
                most likely to want back on the next start. */}
            {defaultModelPin(loaded.modelId)}
          </div>
          {defaultModel === loaded.modelId && <p className="model-note">Pam starts with this model.</p>}
          {unloadFailure && <p className="model-verify is-fail" role="alert">{unloadFailure}</p>}
          {pinFailure?.modelId === loaded.modelId && (
            <p className="model-verify is-fail" role="alert">{pinFailure.detail}</p>
          )}
          {verify.state === "pass" && (
            <p className="model-verify is-pass" role="status">
              <Check size={16} aria-hidden="true" /> Verified · {verify.ms} ms · {verify.tokens} token{verify.tokens === 1 ? "" : "s"} back
            </p>
          )}
          {verify.state === "fail" && (
            <p className="model-verify is-fail" role="alert">{verify.detail}</p>
          )}
          {registered.some((model) => model.modelId !== loaded.modelId) && (
            <div className="access-list model-rows">
              {registered.filter((model) => model.modelId !== loaded.modelId).map(catalogRow)}
            </div>
          )}
        </div>
      ) : registered.length > 0 ? (
        <div className="model-runtime">
          <p className="model-note">A model is registered but not loaded. Bring it into memory to chat and verify.</p>
          <div className="access-list model-rows">{registered.map(catalogRow)}</div>
        </div>
      ) : null}
      {sweepFailure && (
        <div className="model-runtime">
          <p className="model-verify is-fail" role="alert">{sweepFailure}</p>
        </div>
      )}
      {sweepSection}
      {/* The curated picker and the manual import stay reachable whatever is
          registered or loaded: a user on a small quant needs a route to a
          larger one without leaving the app. */}
      <ModelImportForm
        bridge={bridge}
        onImported={onModelImported}
        registered={registered.length}
        refreshTick={refreshTick}
      />
    </section>
  );
}

// The single home for the local model: what is loaded, how a load is
// progressing, the registered catalog, and — always, whatever is registered —
// the way to get another model onto this Mac.
export function ModelsView(props: ModelPanelProps) {
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div>
          <h1>Models</h1>
          <p>The local model Pam runs on, and how to get another one.</p>
        </div>
      </header>
      <ModelPanel {...props} />
    </main>
  );
}
