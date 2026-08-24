/**
 * The state machine behind the Updates card, kept out of the component so it
 * can be tested without a webview or a running backend.
 *
 * The interesting cases are the ones that are *not* "an update is available":
 * a build with no updater endpoint, a check that failed on the network, and a
 * download whose server sent no `Content-Length`. Each has to read as a
 * distinct, honest sentence rather than as a stuck spinner.
 */
import type { UpdateProgress } from "./bindings/UpdateProgress";
import type { UpdateStatus } from "./bindings/UpdateStatus";

export type UpdatePhase =
  /** Nothing asked for yet. */
  | "idle"
  /** A check is in flight. */
  | "checking"
  /** Checked, and this build is the newest. */
  | "current"
  /** Checked, and a newer version is on offer. */
  | "available"
  /** Downloading the new bundle. */
  | "downloading"
  /** Bytes are in; the installer is running and a restart follows. */
  | "installing"
  /** This build has no updater endpoint (a development build). */
  | "unconfigured"
  /** The check or the download failed. */
  | "error";

export interface UpdateState {
  phase: UpdatePhase;
  status: UpdateStatus | null;
  progress: UpdateProgress | null;
  error: string | null;
}

export const INITIAL_UPDATE_STATE: UpdateState = {
  phase: "idle",
  status: null,
  progress: null,
  error: null,
};

/** Fold a completed check into the state the card renders from. */
export function afterCheck(status: UpdateStatus): UpdateState {
  return {
    phase: !status.configured ? "unconfigured" : status.available ? "available" : "current",
    status,
    progress: null,
    error: null,
  };
}

/**
 * Fold one progress event in.
 *
 * `done` arrives when the download finishes and the installer takes over, which
 * is a different sentence from "downloading" because it cannot be cancelled and
 * ends in a relaunch.
 */
export function afterProgress(state: UpdateState, progress: UpdateProgress): UpdateState {
  return {
    ...state,
    phase: progress.done ? "installing" : "downloading",
    progress: progress.done ? state.progress : progress,
  };
}

/**
 * Percentage downloaded, or null when it cannot be known.
 *
 * `total` is absent when the server sent no length, and the values arrive as
 * `bigint` from ts-rs, so both are coerced before dividing.
 */
export function progressPercent(progress: UpdateProgress | null): number | null {
  if (!progress) return null;
  const total = progress.total == null ? 0 : Number(progress.total);
  if (!Number.isFinite(total) || total <= 0) return null;
  const done = Number(progress.downloaded);
  return Math.min(100, Math.max(0, Math.round((done / total) * 100)));
}

/** Bytes as a short human string. Used in the sub-line under the progress bar. */
export function formatBytes(bytes: number | bigint | null | undefined): string {
  const n = bytes == null ? 0 : Number(bytes);
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let value = n / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** The one line of status text the card shows next to its dot. */
export function updateSummary(state: UpdateState): string {
  const current = state.status?.current_version;
  switch (state.phase) {
    case "checking":
      return "Checking for updates…";
    case "unconfigured":
      return "This build cannot update itself — download new versions manually";
    case "current":
      return current ? `Up to date — version ${current}` : "Up to date";
    case "available":
      return `Version ${state.status?.version ?? "?"} is available`;
    case "downloading": {
      const pct = progressPercent(state.progress);
      const done = formatBytes(state.progress?.downloaded ?? 0);
      return pct == null ? `Downloading — ${done}` : `Downloading — ${pct}%`;
    }
    case "installing":
      return "Installing — NovaProxy will restart";
    case "error":
      return state.error ?? "Update check failed";
    default:
      return current ? `Version ${current}` : "Check for updates";
  }
}

/** Whether the card's primary button should be clickable. */
export function canAct(state: UpdateState): boolean {
  return state.phase !== "checking" && state.phase !== "downloading" && state.phase !== "installing";
}
