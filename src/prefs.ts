import { filterFromJson, filterToJson, type SavedFilter } from "./filter";
import {
  DEFAULT_COLUMNS,
  normalizeColumns,
  normalizeWidths,
  type ColumnId,
  type ColumnWidths,
} from "./flows/columns";

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

/**
 * What to do with the OS proxy at launch. `none` leaves the machine alone —
 * the default, because turning it on rewrites a setting the user depends on
 * for working internet.
 */
export type LaunchProxyMode = "none" | "system";

export interface Prefs {
  systemProxyAtLaunch: LaunchProxyMode;
  /**
   * Which table columns are shown, in display order. Eleven exist and seven fit
   * a small window, so this is a real choice rather than a cosmetic one — see
   * `src/flows/columns.tsx`.
   */
  columns: ColumnId[];
  /** Dragged column widths, in px. Absent columns keep their declared track. */
  columnWidths: ColumnWidths;
  /**
   * Follow the tail: keep the newest row selected as it arrives. Off by
   * default — a selection that moves while you are reading a body is worse than
   * one click.
   */
  autoSelect: boolean;
  /** Sidebar hidden (⌘0). Persisted because a small screen stays small. */
  treeHidden: boolean;
  /**
   * The three draggable dividers.
   *
   * Persisted for the same reason as `treeHidden`: how much room the inspector
   * deserves is a property of the screen and of the work, not of the session.
   * The sidebar is px because its content is names of a knowable width; the other
   * two are percentages because they divide whatever height and width the window
   * happens to have.
   */
  treeWidth: number;
  inspectorPct: number;
  requestPct: number;
  /**
   * Filters the user kept. Unlike pins — which die with the session because a
   * flow id does — a filter is worth keeping across launches: it describes the
   * work, not one exchange.
   */
  savedFilters: SavedFilter[];
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
  systemProxyAtLaunch: "none",
  columns: [...DEFAULT_COLUMNS],
  columnWidths: {},
  autoSelect: false,
  treeHidden: false,
  treeWidth: 252,
  inspectorPct: 42,
  requestPct: 50,
  savedFilters: [],
  autoCheckUpdates: true,
  onboardingDone: false,
};

const KEY = "novaproxy.prefs";

/**
 * Keep stored filters usable: a label, and a filter that survives being rebuilt.
 *
 * Passed through `filterFromJson` → `filterToJson` so a blob written by an older
 * build loses only the parts that no longer exist, rather than sitting in the
 * chip row matching nothing.
 */
function normalizeSaved(raw: unknown): SavedFilter[] {
  if (!Array.isArray(raw)) return [];
  const out: SavedFilter[] = [];
  for (const item of raw) {
    if (!item || typeof item !== "object") continue;
    const { id, label, filter } = item as Partial<SavedFilter>;
    if (typeof id !== "string" || typeof label !== "string" || label.trim() === "") continue;
    out.push({ id, label, filter: filterToJson(filterFromJson(filter)) });
  }
  return out;
}

/**
 * Bounds for the dividers, so a stale or hand-edited value cannot hide a pane.
 *
 * The floors are what a pane needs to be worth showing, not the smallest number
 * that renders: a 40px sidebar is a column of ellipses, and a 10% inspector is a
 * tab strip with nothing under it.
 */
export const TREE_W = { min: 180, max: 480 } as const;
export const INSPECTOR_PCT = { min: 20, max: 70 } as const;
export const REQUEST_PCT = { min: 20, max: 80 } as const;

/** Clamp a stored number into range, rejecting NaN — which `Math.min` lets through. */
function clampIn(raw: unknown, range: { min: number; max: number }, fallback: number): number {
  const v = typeof raw === "number" ? raw : Number.NaN;
  if (!Number.isFinite(v)) return fallback;
  return Math.min(range.max, Math.max(range.min, Math.round(v)));
}

/** Coerce arbitrary stored JSON into a complete, in-range `Prefs`. */
export function normalizePrefs(raw: unknown): Prefs {
  const v = (raw ?? {}) as Partial<Record<keyof Prefs, unknown>>;
  return {
    systemProxyAtLaunch: v.systemProxyAtLaunch === "system" ? "system" : "none",
    // A pref blob written by a build that had the list view carries
    // `flowGrouping` and `flowListWidth` and no `columns`; both are dropped and
    // the default column set stands in. Nothing to migrate *from* — grouping is
    // not a column choice — so this is a reset, not a translation.
    columns: normalizeColumns(Array.isArray(v.columns) ? (v.columns as string[]) : null),
    columnWidths: normalizeWidths(v.columnWidths),
    autoSelect: v.autoSelect === true,
    treeHidden: v.treeHidden === true,
    treeWidth: clampIn(v.treeWidth, TREE_W, DEFAULT_PREFS.treeWidth),
    inspectorPct: clampIn(v.inspectorPct, INSPECTOR_PCT, DEFAULT_PREFS.inspectorPct),
    requestPct: clampIn(v.requestPct, REQUEST_PCT, DEFAULT_PREFS.requestPct),
    savedFilters: normalizeSaved(v.savedFilters),
    // Only an explicit `false` opts out, so a pref file written by an older
    // build keeps the safer default.
    autoCheckUpdates: v.autoCheckUpdates !== false,
    // The mirror image of the line above: only an explicit `true` counts as
    // done, so anything missing or corrupt shows the walkthrough rather than
    // silently swallowing it.
    onboardingDone: v.onboardingDone === true,
  };
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
