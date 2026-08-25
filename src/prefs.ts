/**
 * Preferences that outlive a session.
 *
 * Kept in `localStorage` rather than in the backend's data dir because every one
 * of them is a *view* decision the frontend owns — the one exception,
 * `systemProxyAtLaunch`, is a decision about what the frontend should do on
 * mount, not state the engine needs to know about.
 *
 * Reads never throw: a corrupt or half-written value falls back to the default
 * rather than taking the window down with it, since nothing here is worth
 * failing a launch over.
 */

/** How the flow list is grouped when the app opens. */
export type FlowGrouping = "grouped" | "flat";

/**
 * What to do with the OS proxy at launch. `none` leaves the machine alone —
 * the default, because turning it on rewrites a setting the user depends on
 * for working internet.
 */
export type LaunchProxyMode = "none" | "system";

export interface Prefs {
  flowGrouping: FlowGrouping;
  systemProxyAtLaunch: LaunchProxyMode;
  /** Width of the flow list, in px, when the splitter has been dragged. */
  flowListWidth: number;
  /**
   * Look for a new version at launch. On by default: a debugging proxy holds a
   * root CA and a TLS stack, so running an old build is a security decision the
   * user should have to make deliberately. The check only ever *reports* —
   * nothing installs without a click.
   */
  autoCheckUpdates: boolean;
  /**
   * Whether the first-run walkthrough has been dealt with — finished *or*
   * skipped. Unlike the other flags this one defaults to `false` on an
   * unrecognised value rather than to the safe-looking `true`, because a pref
   * blob written before this field existed genuinely means "never onboarded".
   * The App tempers that for upgrades: a machine whose CA is already trusted is
   * marked done without ever seeing the wizard.
   */
  onboardingDone: boolean;
}

export const DEFAULT_PREFS: Prefs = {
  flowGrouping: "grouped",
  systemProxyAtLaunch: "none",
  flowListWidth: 412,
  autoCheckUpdates: true,
  onboardingDone: false,
};

/** Bounds for the flow list, so a stale or hand-edited width cannot hide a pane. */
export const MIN_LIST_WIDTH = 280;
export const MAX_LIST_WIDTH = 900;

const KEY = "novaproxy.prefs";

/** Coerce arbitrary stored JSON into a complete, in-range `Prefs`. */
export function normalizePrefs(raw: unknown): Prefs {
  const v = (raw ?? {}) as Partial<Record<keyof Prefs, unknown>>;
  return {
    flowGrouping: v.flowGrouping === "flat" ? "flat" : "grouped",
    systemProxyAtLaunch: v.systemProxyAtLaunch === "system" ? "system" : "none",
    flowListWidth: clampListWidth(
      typeof v.flowListWidth === "number" ? v.flowListWidth : DEFAULT_PREFS.flowListWidth,
    ),
    // Only an explicit `false` opts out, so a pref file written by an older
    // build keeps the safer default.
    autoCheckUpdates: v.autoCheckUpdates !== false,
    // The mirror image of the line above: only an explicit `true` counts as
    // done, so anything missing or corrupt shows the walkthrough rather than
    // silently swallowing it.
    onboardingDone: v.onboardingDone === true,
  };
}

/** Keep a width inside the usable range (and reject NaN, which `Math.min` lets through). */
export function clampListWidth(width: number): number {
  if (!Number.isFinite(width)) return DEFAULT_PREFS.flowListWidth;
  return Math.min(MAX_LIST_WIDTH, Math.max(MIN_LIST_WIDTH, Math.round(width)));
}

export function loadPrefs(store: Pick<Storage, "getItem"> = localStorage): Prefs {
  try {
    const text = store.getItem(KEY);
    return normalizePrefs(text ? JSON.parse(text) : null);
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export function savePrefs(prefs: Prefs, store: Pick<Storage, "setItem"> = localStorage): void {
  try {
    store.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // Private-mode storage quotas and disabled storage are not worth surfacing:
    // the app works, the choice just does not survive a restart.
  }
}
