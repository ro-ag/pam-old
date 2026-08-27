import { Brain, CalendarBlank, CaretDown, CaretRight, Check, Lightning, Power, Pulse, Queue, UserCircle } from "@phosphor-icons/react";
import { DropdownMenu } from "radix-ui";
import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { withDaemonOperation } from "../bridge";
import type {
  ActivityEventDto,
  ActivityDayDto,
  CallerDto,
  DaemonStatsDto,
  HostMemoryDto,
  ModelImportParams,
  ModelInspectDto,
  ModelPresetDto,
  ModelStatusDto,
  PamBridge,
  ProjectSummaryDto,
  ProjectUsageDto,
} from "../domain";
import { basename, type DaemonView } from "../selectors";
import { presentError } from "../state";
import { formatModelSize } from "./ActivityView";

const DAY_MS = 86_400_000;
export const HEATMAP_WEEKS = 26;

export interface HeatmapCell {
  dayStartMs: number;
  events: number;
  /** 0 (none) through 4 (busiest quartile). */
  intensity: number;
  inWindow: boolean;
}

// Lays the trailing window out as GitHub-style columns of seven UTC days,
// newest column last, aligned so every column starts on a Sunday.
export function buildHeatmapWeeks(days: ActivityDayDto[], todayStartMs: number): HeatmapCell[][] {
  const counts = new Map(days.map((day) => [day.dayStartMs, day.events]));
  const busiest = Math.max(1, ...days.map((day) => day.events));
  // Epoch day zero (1970-01-01) was a Thursday: weekday index 4.
  const weekday = (dayStartMs: number) => (Math.floor(dayStartMs / DAY_MS) + 4) % 7;
  const lastColumnStart = todayStartMs - weekday(todayStartMs) * DAY_MS;
  const firstColumnStart = lastColumnStart - (HEATMAP_WEEKS - 1) * 7 * DAY_MS;
  const windowStart = todayStartMs - (HEATMAP_WEEKS * 7 - 1) * DAY_MS;
  const weeks: HeatmapCell[][] = [];
  for (let week = 0; week < HEATMAP_WEEKS; week += 1) {
    const column: HeatmapCell[] = [];
    for (let day = 0; day < 7; day += 1) {
      const dayStartMs = firstColumnStart + (week * 7 + day) * DAY_MS;
      const events = counts.get(dayStartMs) ?? 0;
      const inWindow = dayStartMs >= windowStart && dayStartMs <= todayStartMs;
      column.push({
        dayStartMs,
        events,
        inWindow,
        intensity:
          !inWindow || events === 0 ? 0 : Math.min(4, Math.ceil((events / busiest) * 4)),
      });
    }
    weeks.push(column);
  }
  return weeks;
}

export interface ActivityStreaks {
  totalEvents: number;
  activeDays: number;
  currentStreak: number;
  longestStreak: number;
}

// Streaks over the served window; the current streak tolerates a quiet
// today so an active week does not reset at midnight.
export function computeStreaks(days: ActivityDayDto[], todayStartMs: number): ActivityStreaks {
  const active = new Set(
    days.filter((day) => day.events > 0).map((day) => Math.floor(day.dayStartMs / DAY_MS)),
  );
  const today = Math.floor(todayStartMs / DAY_MS);
  let longest = 0;
  let run = 0;
  for (const day of [...active].sort((left, right) => left - right)) {
    run = active.has(day - 1) ? run + 1 : 1;
    longest = Math.max(longest, run);
  }
  let current = 0;
  let cursor = active.has(today) ? today : today - 1;
  while (active.has(cursor)) {
    current += 1;
    cursor -= 1;
  }
  return {
    totalEvents: days.reduce((sum, day) => sum + day.events, 0),
    activeDays: active.size,
    currentStreak: current,
    longestStreak: longest,
  };
}

function formatDay(dayStartMs: number): string {
  const date = new Date(dayStartMs);
  return Number.isNaN(date.valueOf())
    ? "Unknown day"
    : new Intl.DateTimeFormat(undefined, {
        month: "short",
        day: "numeric",
        timeZone: "UTC",
      }).format(date);
}

function StatTile({
  icon,
  label,
  value,
  hint,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <article className="project-stat group flex items-center gap-3">
      <span className="project-stat-icon">{icon}</span>
      <div>
        <small>{label}</small>
        <strong title={value}>{value}</strong>
        {hint && <small>{hint}</small>}
      </div>
    </article>
  );
}

// Daemon-wide activity stats: one fetch shared by every panel that needs
// them (overview heatmap, project usage), each rendering its own slice.
export interface DaemonStatsState {
  stats: DaemonStatsDto | null;
  loadError: string | null;
}

export function useDaemonStats(bridge: PamBridge, daemon: DaemonView, refreshTick = 0): DaemonStatsState {
  const [stats, setStats] = useState<DaemonStatsDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const offline = daemon.state === "stopped";

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setLoadError(null);
    try {
      // Activity statistics are daemon-global: always the daemon authority.
      const response = await bridge.daemonStats(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setStats(response);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    }
  }, [bridge]);

  useEffect(() => {
    if (offline) {
      setStats(null);
      setLoadError(null);
      return;
    }
    void load();
    return () => {
      requestSequence.current += 1;
    };
  }, [load, offline, refreshTick]);

  return { stats, loadError };
}

export interface OverviewPanelProps {
  daemon: DaemonView;
  stats: DaemonStatsDto | null;
  loadError: string | null;
}

export function OverviewPanel({ daemon, stats, loadError }: OverviewPanelProps) {
  const offline = daemon.state === "stopped";
  const todayStartMs = Math.floor(Date.now() / DAY_MS) * DAY_MS;
  const days = stats?.status === "ok" ? stats.days : [];
  const streaks = computeStreaks(days, todayStartMs);
  const weeks = buildHeatmapWeeks(days, todayStartMs);

  return (
    <>
      <section className="project-overview" aria-label="Daemon overview">
        <article className="project-stat group flex items-center gap-3">
          <span
            className={`project-stat-icon${daemon.state === "running" ? "" : " project-stat-icon--attention"}`}
          >
            <Pulse size={21} weight="bold" />
          </span>
          <div>
            <small>Watch status</small>
            <strong title={daemon.detail}>{daemon.detail}</strong>
          </div>
        </article>
        <StatTile
          icon={<Queue size={21} weight="bold" />}
          label="Queue depth"
          value={daemon.queueDepth === null ? "—" : String(daemon.queueDepth)}
        />
        <StatTile
          icon={<Lightning size={21} weight="bold" />}
          label="Events"
          value={streaks.totalEvents.toLocaleString()}
          hint="last 26 weeks"
        />
        <StatTile
          icon={<CalendarBlank size={21} weight="bold" />}
          label="Active days"
          value={String(streaks.activeDays)}
        />
        <StatTile
          icon={<Lightning size={21} weight="bold" />}
          label="Streak"
          value={`${streaks.currentStreak}d`}
          hint={`longest ${streaks.longestStreak}d`}
        />
      </section>
      <section className="panel heatmap-panel" aria-labelledby="overview-heatmap-heading">
        <div className="panel-title">
          <div>
            <span className="eyebrow">Daemon activity</span>
            <h2 id="overview-heatmap-heading">The last 26 weeks</h2>
          </div>
        </div>
        {loadError ? (
          <p className="panel-empty" role="alert">
            {loadError}
          </p>
        ) : offline ? (
          <p className="panel-empty">The activity picture returns when PAM is back on watch.</p>
        ) : stats && stats.status !== "ok" ? (
          <p className="panel-empty">
            {[stats.failure.detail, stats.failure.recovery].filter(Boolean).join(" ")}
          </p>
        ) : (
          <div className="heatmap-scroll">
            <div className="heatmap" role="img" aria-label="Daily daemon activity for the last 26 weeks">
              {weeks.map((week) => (
                <div className="heatmap-week" key={week[0].dayStartMs}>
                  {week.map((cell) => (
                    <span
                      key={cell.dayStartMs}
                      className={`heatmap-cell heat-${cell.intensity}${cell.inWindow ? "" : " heatmap-cell--outside"}`}
                      title={`${formatDay(cell.dayStartMs)} · ${cell.events} event${cell.events === 1 ? "" : "s"}`}
                    />
                  ))}
                </div>
              ))}
            </div>
            <div className="heatmap-legend" aria-hidden="true">
              <small>Less</small>
              {[0, 1, 2, 3, 4].map((level) => (
                <span className={`heatmap-cell heat-${level}`} key={level} />
              ))}
              <small>More</small>
            </div>
          </div>
        )}
      </section>
    </>
  );
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

// null means "not known yet" (still loading host_memory); the caller treats
// unknown as neither a pass nor a fail.
export function fitsMemory(minMemoryBytes: number, hostTotalBytes: number | null): boolean | null {
  return hostTotalBytes === null ? null : hostTotalBytes >= minMemoryBytes;
}

// The curated path: pick a preset, review its license, download it. PAM
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
  const [hostMemory, setHostMemory] = useState<HostMemoryDto | null>(null);
  const hostMemoryBytes = hostMemory?.totalBytes ?? null;
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
        setHostMemory(memoryResponse);
        if (downloadStatus.presetId) {
          setSelectedId(downloadStatus.presetId);
          setAccepted(true);
        }
        if (downloadStatus.status === "running") {
          setProgress({ receivedBytes: downloadStatus.receivedBytes, totalBytes: downloadStatus.totalBytes });
          setPhase("running");
        } else if (downloadStatus.status === "complete" && downloadStatus.presetId) {
          setProgress({ receivedBytes: downloadStatus.receivedBytes, totalBytes: downloadStatus.totalBytes });
          setPhase("complete");
          if (!completedRef.current) {
            completedRef.current = true;
            onImported();
          }
        } else if (downloadStatus.status === "failed" && downloadStatus.presetId) {
          setProgress({ receivedBytes: downloadStatus.receivedBytes, totalBytes: downloadStatus.totalBytes });
          setPhase("failed");
          setDownloadError(
            [downloadStatus.failure?.detail, downloadStatus.failure?.recovery].filter(Boolean).join(" ") ||
              "The download failed partway through.",
          );
        } else if (downloadStatus.status === "cancelled" && downloadStatus.presetId) {
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
  const fits = selected ? fitsMemory(selected.minMemoryBytes, hostMemoryBytes) : null;
  const busy = phase === "running";

  const startDownload = async () => {
    if (!selected || !accepted || busy) return;
    completedRef.current = false;
    setDownloadError(null);
    setProgress({ receivedBytes: 0, totalBytes: selected.expectedSizeBytes });
    setPhase("running");
    try {
      const response = await bridge.modelDownload(withDaemonOperation(), selected.id);
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
        <p className="model-note">Looking at the curated models PAM can fetch for you…</p>
      ) : (
        <>
          {hostMemory !== null && hostMemory.totalBytes < hostMemory.supportedMinimumBytes && (
            <p className="model-fit-warn model-host-notice">
              PAM's local model is built for machines with {formatHostMemory(hostMemory.supportedMinimumBytes)} of
              memory or more; this Mac reports {formatHostMemory(hostMemory.totalBytes)}.
            </p>
          )}
          <DropdownMenu.Root open={pickerOpen && !busy} onOpenChange={(open) => !busy && setPickerOpen(open)}>
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                className="button button--secondary button--small model-preset-trigger"
                disabled={busy}
              >
                {selected ? selected.label : "Choose a model"}
                <CaretDown size={14} weight="bold" aria-hidden="true" />
              </button>
            </DropdownMenu.Trigger>
            {busy && <small className="model-note">A download is already running — wait for it to finish before choosing another model.</small>}
            <DropdownMenu.Portal>
              <DropdownMenu.Content className="project-menu-popover model-preset-popover" align="start" sideOffset={8}>
                <DropdownMenu.RadioGroup
                  value={selectedId ?? ""}
                  onValueChange={(value) => {
                    if (busy) return;
                    setSelectedId(value);
                    setAccepted(false);
                    setPhase("idle");
                    setDownloadError(null);
                    setProgress(null);
                  }}
                >
                  {presets.map((preset) => {
                    const presetFits = fitsMemory(preset.minMemoryBytes, hostMemoryBytes);
                    return (
                      <DropdownMenu.RadioItem
                        key={preset.id}
                        className="project-menu-item model-preset-item"
                        value={preset.id}
                        textValue={preset.label}
                      >
                        <span>
                          <strong>{preset.label}</strong>
                          <small>
                            {preset.paramsLabel} · {preset.quantLabel} · {formatModelSize(preset.expectedSizeBytes)} ·{" "}
                            {preset.licenseId}
                          </small>
                          <small className={presetFits === false ? "model-fit-warn" : undefined}>
                            {presetFits === null
                              ? "Checking this Mac's memory…"
                              : presetFits
                                ? "Runs on this Mac"
                                : `Needs ~${formatHostMemory(preset.minMemoryBytes)} memory; this Mac has ${formatHostMemory(hostMemoryBytes ?? 0)}`}
                          </small>
                        </span>
                        <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
                      </DropdownMenu.RadioItem>
                    );
                  })}
                </DropdownMenu.RadioGroup>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>

          {selected && (
            <div className="model-preset-summary">
              <div className="model-identity">
                <strong title={selected.model}>{selected.model}</strong>
                <small>{formatModelSize(selected.expectedSizeBytes)} download · {selected.licenseId}</small>
              </div>
              <p className="model-note">{selected.licenseUrl}</p>
              <p className="model-note">{selected.licenseNoticeText}</p>
              {fits === false && (
                <p className="model-fit-warn" role="status">
                  Needs ~{formatHostMemory(selected.minMemoryBytes)} memory; this Mac has {formatHostMemory(hostMemoryBytes ?? 0)}.
                </p>
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
                {busy && <small>PAM fetches, hashes, and registers this model, all from this screen.</small>}
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

// The manual path: point PAM at an already-downloaded GGUF. License fields
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
  // Set when an import completes against an artifact outside PAM's calibrated
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
        detail: `Fill in the ${missingLicenseFields.join(", ")} under Advanced — PAM records exactly which license you accept.`,
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
              Below PAM's recommended minimum of {formatModelSize(inspect.floorBytes)} — override this
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
              Allow a model below PAM's recommended minimum size (results will fall short in real flows).
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
        {busy && <small>PAM reads and hashes the whole file in the background; the rest of the app stays responsive.</small>}
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
          Registered, but this artifact is not in PAM's calibrated set — it may fail to load under
          this Mac's runtime profile. The calibrated presets in the download list above are the
          tested path.
        </p>
      )}
    </form>
  );
}

// The whole setup happens here: a curated preset PAM downloads for you, or
// point PAM at a GGUF you already have — no terminal round-trip either way.
function ModelImportForm({
  bridge,
  onImported,
  refreshTick = 0,
}: {
  bridge: PamBridge;
  onImported: () => void;
  refreshTick?: number;
}) {
  return (
    <div className="model-runtime model-setup">
      <p className="model-note">
        No local model is registered yet. Choose a curated model for PAM to download, or import one
        you already have.
      </p>
      <PresetDownload bridge={bridge} onImported={onImported} refreshTick={refreshTick} />
      <div className="model-setup-divider" role="separator">
        <span>or import a downloaded GGUF</span>
      </div>
      <ManualImport bridge={bridge} onImported={onImported} refreshTick={refreshTick} />
    </div>
  );
}

export interface ModelPanelProps {
  bridge: PamBridge;
  daemon: DaemonView;
  modelStatus: ModelStatusDto | null;
  /** True while a daemon lifecycle command is in flight. */
  modelBusy: boolean;
  onOpenModelChat: (modelId: string, returnFocusTarget?: HTMLElement) => void;
  onStartWithModel: (modelId: string) => void;
  onModelImported: () => void;
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
  onModelImported,
  refreshTick = 0,
}: ModelPanelProps) {
  const [verify, setVerify] = useState<VerifyState>({ state: "idle" });
  const verifySequence = useRef(0);
  const offline = daemon.state === "stopped";
  const loaded = modelStatus?.status === "ok" ? modelStatus.loaded : null;
  const registered = modelStatus?.status === "ok" ? modelStatus.registered : [];

  // A verify result only ever applies to the model it measured; once the
  // loaded model changes (e.g. a restart with a different one), the stale
  // pass/fail line must not keep rendering next to the new identity.
  useEffect(() => {
    verifySequence.current += 1;
    setVerify({ state: "idle" });
  }, [loaded?.modelId]);
  const restartLabel = offline ? "Start PAM with this model" : "Restart PAM with this model";

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

  const pill = offline || !modelStatus || modelStatus.status !== "ok"
    ? { label: offline ? "unreachable" : !modelStatus ? "checking" : "unreachable", tone: offline || modelStatus ? "attention" : "not-reported" }
    : loaded
      ? { label: "loaded", tone: "healthy" }
      : registered.length > 0
        ? { label: "on deck", tone: "observed" }
        : { label: "none", tone: "not-reported" };

  const restartRow = (model: { modelId: string; sizeBytes: number }) => (
    <article key={model.modelId}>
      <span className="access-icon"><Brain size={21} /></span>
      <div>
        <strong title={model.modelId}>{model.modelId}</strong>
        <p>{formatModelSize(model.sizeBytes)} on disk</p>
      </div>
      <button
        type="button"
        className="button button--secondary button--small"
        disabled={modelBusy}
        onClick={() => onStartWithModel(model.modelId)}
      >
        <Power size={17} /> {restartLabel}
      </button>
    </article>
  );

  return (
    <section className="panel model-panel" aria-labelledby="model-panel-heading">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Local model</span>
          <h2 id="model-panel-heading">Model runtime</h2>
        </div>
        <span className={`state-pill state-pill--${pill.tone}`}>{pill.label}</span>
      </div>
      {offline ? (
        <p className="panel-empty">PAM is paused, so the local model runtime is not reachable. Start PAM to check on it.</p>
      ) : !modelStatus ? (
        <p className="panel-empty">Checking the local model…</p>
      ) : modelStatus.status !== "ok" ? (
        <p className="panel-empty">
          {[modelStatus.failure.detail, modelStatus.failure.recovery].filter(Boolean).join(" ")}
        </p>
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
          </div>
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
              {registered.filter((model) => model.modelId !== loaded.modelId).map(restartRow)}
            </div>
          )}
        </div>
      ) : registered.length > 0 ? (
        <div className="model-runtime">
          <p className="model-note">A model is registered but not loaded. Bring it into memory to chat and verify.</p>
          <div className="access-list model-rows">{registered.map(restartRow)}</div>
        </div>
      ) : (
        <ModelImportForm bridge={bridge} onImported={onModelImported} refreshTick={refreshTick} />
      )}
    </section>
  );
}

export interface CallerRequestRow {
  callerId: string;
  requests: number;
  revoked: boolean;
  /** Self-declared local caller surface; null for legacy or unregistered callers. */
  kind: string | null;
}

// The whole project story on this screen: recent daemon requests, grouped by
// caller. Registered-but-quiet callers stay visible with a zero count.
export function aggregateCallerRequests(
  callers: CallerDto[],
  events: ActivityEventDto[],
): CallerRequestRow[] {
  const rows = new Map<string, CallerRequestRow>();
  for (const caller of callers) {
    rows.set(caller.callerId, {
      callerId: caller.callerId,
      requests: 0,
      revoked: caller.revokedAtMs !== null,
      kind: caller.kind,
    });
  }
  for (const event of events) {
    const row = rows.get(event.callerId) ?? {
      callerId: event.callerId,
      requests: 0,
      revoked: false,
      kind: null,
    };
    row.requests += 1;
    rows.set(event.callerId, row);
  }
  return [...rows.values()].sort(
    (left, right) => right.requests - left.requests || left.callerId.localeCompare(right.callerId),
  );
}

// e.g. "GUI · bfb1974c…" for a caller with a declared kind — production
// caller IDs are UUIDs, so only the first 8 characters are shown, with the
// full ID as a tooltip. Legacy callers with no recorded kind render exactly
// as before: the full ID, no badge.
function CallerLabel({ callerId, kind }: { callerId: string; kind: string | null }) {
  if (!kind) return <strong>{callerId}</strong>;
  return (
    <strong title={callerId}>
      <span className="state-pill state-pill--observed">{kind.toUpperCase()}</span> {callerId.slice(0, 8)}…
    </strong>
  );
}

interface CallerRequestsPanelProps {
  bridge: PamBridge;
  daemon: DaemonView;
  /** True when the desktop's own GUI caller still needs registration. */
  registrationNeeded: boolean;
  registrationBusy: boolean;
  onRegisterCaller: () => void;
  /** Bumped by ⌘R; re-runs the mount-time fetch without a remount. */
  refreshTick?: number;
}

function CallerRequestsPanel({
  bridge,
  daemon,
  registrationNeeded,
  registrationBusy,
  onRegisterCaller,
  refreshTick = 0,
}: CallerRequestsPanelProps) {
  const [rows, setRows] = useState<CallerRequestRow[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const offline = daemon.state === "stopped";

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setLoadError(null);
    try {
      // Both slices are daemon-global: always the daemon authority.
      const [callers, activity] = await Promise.all([
        bridge.callerRegistry(withDaemonOperation()),
        bridge.daemonActivity(withDaemonOperation()),
      ]);
      if (sequence !== requestSequence.current) return;
      if (callers.status !== "ok") {
        setLoadError([callers.failure.detail, callers.failure.recovery].filter(Boolean).join(" "));
        return;
      }
      if (activity.status !== "ok") {
        setLoadError([activity.failure.detail, activity.failure.recovery].filter(Boolean).join(" "));
        return;
      }
      setRows(aggregateCallerRequests(callers.callers, activity.events));
      setTruncated(activity.truncated);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    }
  }, [bridge]);

  useEffect(() => {
    if (offline) {
      setRows(null);
      setLoadError(null);
      return;
    }
    void load();
    return () => {
      requestSequence.current += 1;
    };
  }, [load, offline, refreshTick]);

  return (
    <section className="panel" aria-labelledby="caller-requests-heading">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Daemon requests</span>
          <h2 id="caller-requests-heading">Requests per caller</h2>
        </div>
        {truncated && <small>recent window</small>}
      </div>
      {registrationNeeded && !offline && (
        <div className="access-list">
          <article>
            <span className="access-icon" aria-hidden="true"><UserCircle size={21} /></span>
            <div>
              <strong>This desktop</strong>
              <p>PAM has not registered this GUI as a caller yet.</p>
            </div>
            <button
              type="button"
              className="button button--primary button--small"
              disabled={registrationBusy}
              onClick={onRegisterCaller}
            >
              {registrationBusy ? "Registering…" : "Register GUI caller"}
            </button>
          </article>
        </div>
      )}
      {offline ? (
        <p className="panel-empty">PAM is paused, so no requests are being served.</p>
      ) : loadError ? (
        <p className="panel-empty" role="alert">{loadError}</p>
      ) : !rows ? (
        <p className="panel-empty" aria-busy="true" aria-live="polite">Loading the recent requests…</p>
      ) : rows.length === 0 ? (
        <p className="panel-empty">No caller has talked to the daemon yet.</p>
      ) : (
        <div className="access-list">
          {rows.map((row) => (
            <article key={row.callerId}>
              <span className="access-icon" aria-hidden="true"><UserCircle size={21} /></span>
              <div>
                <CallerLabel callerId={row.callerId} kind={row.kind} />
                <p>{row.requests.toLocaleString()} request{row.requests === 1 ? "" : "s"} recently</p>
              </div>
              <span className={`state-pill state-pill--${row.revoked ? "attention" : "observed"}`}>
                {row.revoked ? "revoked" : "active"}
              </span>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function formatLastActive(lastEventMs: number | null): string {
  if (lastEventMs === null) return "No activity yet";
  const date = new Date(lastEventMs);
  return Number.isNaN(date.valueOf())
    ? "Date unavailable"
    : new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(date);
}

export interface ProjectUsageRow {
  projectId: string;
  name: string;
  location: string | null;
  events: number;
  lastEventMs: number | null;
}

// The whole known fleet, not just the active project: every catalog project
// appears (zero-usage ones included), plus any usage row PAM reports for a
// project the catalog does not (yet) know about.
export function aggregateProjectUsage(
  catalog: ProjectSummaryDto[],
  usage: ProjectUsageDto[],
): ProjectUsageRow[] {
  const rows = new Map<string, ProjectUsageRow>();
  for (const project of catalog) {
    rows.set(project.handle, {
      projectId: project.handle,
      name: project.name,
      location: project.location,
      events: 0,
      lastEventMs: null,
    });
  }
  for (const row of usage) {
    const known = catalog.find((project) => project.handle === row.projectId);
    rows.set(row.projectId, {
      projectId: row.projectId,
      name: known ? known.name : row.root ? basename(row.root) : `${row.projectId.slice(0, 8)}…`,
      location: known ? known.location : row.root,
      events: row.events,
      lastEventMs: row.lastEventMs,
    });
  }
  return [...rows.values()].sort(
    (left, right) => right.events - left.events || left.name.localeCompare(right.name),
  );
}

// RAM reads in binary GiB, the way machines are sold: 34_359_738_368 -> "32 GB".
function formatHostMemory(bytes: number): string {
  return `${Math.round(bytes / (1 << 30))} GB`;
}

interface ProjectsPanelProps {
  daemon: DaemonView;
  catalog: ProjectSummaryDto[];
  stats: DaemonStatsDto | null;
  loadError: string | null;
}

// The fleet overview: every project PAM knows about, at a glance, with no
// selector — this screen never scopes to one project.
function ProjectsPanel({ daemon, catalog, stats, loadError }: ProjectsPanelProps) {
  const offline = daemon.state === "stopped";

  // Defensive: an older daemon may not report `projects` yet.
  const usage = stats?.status === "ok" ? stats.projects ?? [] : [];
  const rows = aggregateProjectUsage(catalog, usage);
  const maxEvents = Math.max(1, ...rows.map((row) => row.events));

  return (
    <section className="panel projects-panel" aria-labelledby="projects-panel-heading">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Projects</span>
          <h2 id="projects-panel-heading">Usage by project</h2>
        </div>
      </div>
      {loadError ? (
        <p className="panel-empty" role="alert">
          {loadError}
        </p>
      ) : offline ? (
        <p className="panel-empty">Project usage returns when PAM is back on watch.</p>
      ) : stats && stats.status !== "ok" ? (
        <p className="panel-empty">
          {[stats.failure.detail, stats.failure.recovery].filter(Boolean).join(" ")}
        </p>
      ) : rows.length === 0 ? (
        <p className="panel-empty">No projects are known to PAM yet.</p>
      ) : (
        <div className="project-usage-list">
          {rows.map((row) => (
            <article className="project-usage-row" key={row.projectId}>
              <div className="project-usage-identity">
                <strong title={row.name}>{row.name}</strong>
                {row.location && <small title={row.location}>{row.location}</small>}
              </div>
              <div className="project-usage-bar-track" aria-hidden="true">
                <div
                  className="project-usage-bar-fill"
                  style={{ width: `${Math.round((row.events / maxEvents) * 100)}%` }}
                />
              </div>
              <div className="project-usage-meta">
                <strong>
                  {row.events.toLocaleString()} event{row.events === 1 ? "" : "s"}
                </strong>
                <small>{formatLastActive(row.lastEventMs)}</small>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

export interface ControlCenterViewProps {
  bridge: PamBridge;
  daemon: DaemonView;
  catalog?: ProjectSummaryDto[];
  modelStatus: ModelStatusDto | null;
  modelBusy: boolean;
  onOpenModelChat: (modelId: string, returnFocusTarget?: HTMLElement) => void;
  onStartWithModel: (modelId: string) => void;
  onModelImported: () => void;
  registrationNeeded?: boolean;
  registrationBusy?: boolean;
  onRegisterCaller?: () => void;
  /** Bumped by ⌘R; re-runs the mount-time loaders without remounting, so
   * in-progress form state (e.g. a manual import) survives a refresh. */
  refreshTick?: number;
}

export function ControlCenterView({
  bridge,
  daemon,
  catalog = [],
  modelStatus,
  modelBusy,
  onOpenModelChat,
  onStartWithModel,
  onModelImported,
  registrationNeeded = false,
  registrationBusy = false,
  onRegisterCaller = () => {},
  refreshTick = 0,
}: ControlCenterViewProps) {
  const { stats, loadError } = useDaemonStats(bridge, daemon, refreshTick);
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div>
          <h1>Control center</h1>
          <p>Everything PAM watches, at a glance.</p>
        </div>
      </header>
      <OverviewPanel daemon={daemon} stats={stats} loadError={loadError} />
      <ProjectsPanel daemon={daemon} catalog={catalog} stats={stats} loadError={loadError} />
      <ModelPanel
        bridge={bridge}
        daemon={daemon}
        modelStatus={modelStatus}
        modelBusy={modelBusy}
        onOpenModelChat={onOpenModelChat}
        onStartWithModel={onStartWithModel}
        onModelImported={onModelImported}
        refreshTick={refreshTick}
      />
      <CallerRequestsPanel
        bridge={bridge}
        daemon={daemon}
        registrationNeeded={registrationNeeded}
        registrationBusy={registrationBusy}
        onRegisterCaller={onRegisterCaller}
        refreshTick={refreshTick}
      />
    </main>
  );
}
