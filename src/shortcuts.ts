/**
 * Every keyboard shortcut, in one registry.
 *
 * The dispatcher (`useShortcuts`) and the Shortcuts dialog both read this array,
 * which is the point: a table of shortcuts written by hand next to a dispatcher
 * written by hand is a table that is wrong by the second commit. See
 * `issues/0003-keyboard-shortcuts.md` for the decisions behind the assignments.
 */

/** Who is allowed to act on a chord. */
export type ShortcutScope =
  /** Anywhere, unless a modal is open. */
  | "global"
  /** Only while the flows table has focus; handled by the table itself. */
  | "table"
  /** Acts on the inspector panes, so only with a flow selected. */
  | "inspector"
  /** Only while a modal is open. Documented, not dispatched. */
  | "modal";

export type ShortcutId =
  | "palette"
  | "clear"
  | "shortcuts"
  | "settings"
  | "section.flows"
  | "section.rules"
  | "section.break"
  | "section.scripts"
  | "section.certs"
  | "record"
  | "session.save"
  | "session.open"
  | "session.har"
  | "filter.search"
  | "filter.tree"
  | "filter.toggle"
  | "tree.toggle"
  | "filter.newClause"
  | "filter.removeClause"
  | "row.prev"
  | "row.next"
  | "row.first"
  | "row.last"
  | "row.pageUp"
  | "row.pageDown"
  | "row.inspect"
  | "row.selectAll"
  | "flow.resend"
  | "flow.curl"
  | "pane.prevTab"
  | "pane.nextTab"
  | "pane.switch"
  | "pane.collapse"
  | "modal.close";

export interface Shortcut {
  id: ShortcutId;
  scope: ShortcutScope;
  /** `Mod+Shift+F`, `ArrowDown`, `Escape`. `Mod` is ⌘ on macOS, Ctrl elsewhere. */
  chord: string;
  /** Heading it appears under in the dialog. */
  group: string;
  label: string;
  /** Extra condition, shown as a note. */
  when?: string;
  /**
   * True when the native menu owns the chord, so the webview never sees it (on
   * macOS a menu accelerator is consumed before the page). Documented here,
   * dispatched by the OS.
   */
  native?: boolean;
}

export const SHORTCUTS: Shortcut[] = [
  // ---- global ----
  { id: "palette", scope: "global", chord: "Mod+P", group: "Global", label: "Command palette" },
  {
    id: "clear",
    scope: "global",
    chord: "Mod+K",
    group: "Global",
    label: "Clear all flows",
    when: "cannot be undone — ⌘S saves the session first",
  },
  {
    id: "shortcuts",
    scope: "global",
    chord: "Mod+/",
    group: "Global",
    label: "Keyboard shortcuts",
    native: true,
  },
  { id: "settings", scope: "global", chord: "Mod+,", group: "Global", label: "Settings" },
  { id: "record", scope: "global", chord: "Mod+Shift+R", group: "Global", label: "Pause or resume capture" },
  { id: "section.flows", scope: "global", chord: "Mod+1", group: "Global", label: "Go to Flows" },
  { id: "section.rules", scope: "global", chord: "Mod+2", group: "Global", label: "Go to Rules" },
  { id: "section.break", scope: "global", chord: "Mod+3", group: "Global", label: "Go to Breakpoints" },
  { id: "section.scripts", scope: "global", chord: "Mod+4", group: "Global", label: "Go to Scripts" },
  { id: "section.certs", scope: "global", chord: "Mod+5", group: "Global", label: "Go to Certificate" },
  { id: "session.save", scope: "global", chord: "Mod+S", group: "Global", label: "Save session (.nova)" },
  { id: "session.open", scope: "global", chord: "Mod+O", group: "Global", label: "Open session (.nova)" },
  { id: "session.har", scope: "global", chord: "Mod+Shift+E", group: "Global", label: "Export as HAR" },

  // ---- filter ----
  { id: "filter.search", scope: "global", chord: "Mod+F", group: "Filter", label: "Focus the search box" },
  { id: "filter.tree", scope: "global", chord: "Mod+Shift+F", group: "Filter", label: "Focus the tree filter" },
  {
    id: "filter.toggle",
    scope: "global",
    chord: "Mod+B",
    group: "Filter",
    label: "Turn the filters off and on",
    when: "keeps the filters — it does not clear them",
  },
  {
    id: "filter.newClause",
    scope: "global",
    chord: "Mod+N",
    group: "Filter",
    label: "Add a filter condition",
  },
  {
    id: "filter.removeClause",
    scope: "global",
    chord: "Mod+Shift+N",
    group: "Filter",
    label: "Remove the last condition",
  },
  { id: "tree.toggle", scope: "global", chord: "Mod+0", group: "Filter", label: "Show or hide the sidebar" },

  // ---- the table ----
  { id: "row.prev", scope: "table", chord: "ArrowUp", group: "Flows table", label: "Previous row" },
  { id: "row.next", scope: "table", chord: "ArrowDown", group: "Flows table", label: "Next row" },
  { id: "row.first", scope: "table", chord: "Home", group: "Flows table", label: "Newest row" },
  { id: "row.last", scope: "table", chord: "End", group: "Flows table", label: "Oldest row" },
  { id: "row.pageUp", scope: "table", chord: "PageUp", group: "Flows table", label: "Up one page" },
  { id: "row.pageDown", scope: "table", chord: "PageDown", group: "Flows table", label: "Down one page" },
  { id: "row.inspect", scope: "table", chord: "Enter", group: "Flows table", label: "Focus the inspector" },
  {
    id: "row.selectAll",
    scope: "global",
    chord: "Mod+Shift+A",
    group: "Flows table",
    label: "Select every visible row",
    // ⌘A belongs to the Edit menu's Select All, which macOS consumes before the
    // webview; taking it would mean dropping that item and breaking Select All
    // in every text field (issues/0003 §9.3).
    when: "⌘A is the Edit menu's, so this takes Shift",
  },
  { id: "flow.resend", scope: "global", chord: "Mod+Enter", group: "Flows table", label: "Resend the selected flow" },
  { id: "flow.curl", scope: "global", chord: "Mod+Shift+C", group: "Flows table", label: "Copy as cURL" },

  // ---- inspector ----
  { id: "pane.prevTab", scope: "inspector", chord: "Mod+[", group: "Inspector", label: "Previous panel" },
  { id: "pane.nextTab", scope: "inspector", chord: "Mod+]", group: "Inspector", label: "Next panel" },
  { id: "pane.switch", scope: "inspector", chord: "Mod+Shift+ArrowRight", group: "Inspector", label: "Switch pane" },
  { id: "pane.collapse", scope: "inspector", chord: "Mod+E", group: "Inspector", label: "Collapse or expand the pane" },

  // ---- modal ----
  { id: "modal.close", scope: "modal", chord: "Escape", group: "Dialogs", label: "Close" },
];

/**
 * Chords the OS or the webview already owns.
 *
 * Not a suggestion: a shortcut here either never arrives or breaks something the
 * user needs. `Mod+P` is deliberately absent — Print is a menu item on macOS and
 * this app has none (issues/0003 §5).
 */
export const RESERVED: string[] = [
  "Mod+Q", "Mod+W", "Mod+M", "Mod+H", "Mod+Shift+H",
  "Mod+C", "Mod+V", "Mod+X", "Mod+Z", "Mod+Shift+Z", "Mod+A",
  "Mod+R", // reloads the webview
  "F5",
  "Mod+Tab",
];

/** macOS, read from the UA so nothing has to be injected into the app to test it. */
export function isMac(ua: string = navigator.userAgent): boolean {
  return ua.includes("Macintosh") || ua.includes("Mac OS X");
}

interface Parsed {
  mod: boolean;
  shift: boolean;
  key: string;
}

/**
 * Split a chord into its parts.
 *
 * Alt is not part of the grammar at all, and that is deliberate: on macOS
 * `Option+o` produces `ø`, so `e.key` for an Alt chord is a character the user
 * never typed and cannot predict (issues/0003 §2).
 */
export function parseChord(chord: string): Parsed {
  const parts = chord.split("+");
  const key = parts[parts.length - 1];
  return {
    mod: parts.includes("Mod"),
    shift: parts.includes("Shift"),
    key,
  };
}

const NAMED = new Set([
  "Enter", "Escape", "Tab", "Home", "End", "PageUp", "PageDown",
  "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
]);

/** Keys whose event value differs from how it is written in a chord. */
const GLYPH: Record<string, string> = {
  Enter: "↵",
  Escape: "Esc",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  PageUp: "PgUp",
  PageDown: "PgDn",
};

/**
 * A chord as badges: `["⌘", "⇧", "F"]`.
 *
 * An array rather than a string because the dialog renders one keycap per key,
 * and joining them here would make the caller split them apart again.
 */
export function formatChord(chord: string, mac = isMac()): string[] {
  const { mod, shift, key } = parseChord(chord);
  const out: string[] = [];
  if (mod) out.push(mac ? "⌘" : "Ctrl");
  if (shift) out.push(mac ? "⇧" : "Shift");
  out.push(GLYPH[key] ?? (key.length === 1 ? key.toUpperCase() : key));
  return out;
}

/** Does this keyboard event mean this chord? */
export function matchChord(
  e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">,
  chord: string,
  mac = isMac(),
): boolean {
  const { mod, shift, key } = parseChord(chord);
  // Alt is never part of a chord, so an event carrying it is never a match —
  // otherwise ⌥⌘F would fire ⌘F.
  if (e.altKey) return false;
  const modDown = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (mod !== modDown) return false;
  // A chord that does not ask for Ctrl on macOS must not accept it: Ctrl+K is
  // kill-line in every AppKit text field.
  if (mac && e.ctrlKey) return false;
  if (shift !== e.shiftKey) return false;
  return NAMED.has(key) ? e.key === key : e.key.toLowerCase() === key.toLowerCase();
}

/**
 * Should a chord be ignored because the user is typing?
 *
 * Single-key shortcuts must not fire inside a text field — typing "j" in the
 * search box would jump a row — but `Escape` and the arrow keys still have to
 * work there, since they are how you leave the field or move through a list.
 */
export function suppressedWhileTyping(chord: string, target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  const tag = el?.tagName?.toLowerCase();
  const typing = tag === "input" || tag === "textarea" || el?.isContentEditable === true;
  if (!typing) return false;
  const { mod, key } = parseChord(chord);
  if (mod) return false;
  return !NAMED.has(key);
}

export function shortcut(id: ShortcutId): Shortcut {
  const found = SHORTCUTS.find((s) => s.id === id);
  if (!found) throw new Error(`no shortcut ${id}`);
  return found;
}

/** The dialog's rows, in the order the groups were declared. */
export function shortcutGroups(): { group: string; items: Shortcut[] }[] {
  const out: { group: string; items: Shortcut[] }[] = [];
  for (const s of SHORTCUTS) {
    const last = out.find((g) => g.group === s.group);
    if (last) last.items.push(s);
    else out.push({ group: s.group, items: [s] });
  }
  return out;
}
