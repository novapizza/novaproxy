/**
 * What changed in each release, newest first. Help › What's New renders it,
 * and the app opens it by itself on the first launch after an update.
 *
 * Written for the people who use NovaProxy, not for the people who build it:
 * what they can now do, or what stopped going wrong, in one line each. CI,
 * refactors and version bumps stay out. A release with nothing worth telling
 * still gets an entry — `changelog.test.ts` checks that the newest one matches
 * `package.json`, so a version cannot ship without one.
 */
export interface Release {
  version: string;
  /** ISO date the tag was cut. */
  date: string;
  added?: string[];
  improved?: string[];
  fixed?: string[];
}

export const CHANGELOG: Release[] = [
  {
    version: "0.9.0",
    date: "2026-11-16",
    added: [
      "Help › What's New lists what changed in each release, and opens by itself after an update.",
      "A \"Filters off ⌘B\" label shows when the filters are switched off, and dims the settings it is not applying.",
      "MCP: clients sign in with a token, and three new tools — summarize_session, diff_flows and explain_failure.",
      "WebSocket messages show a preview, with the full frame on demand.",
    ],
    improved: [
      "Export HAR and Save session write the rows the table shows when a filter is on.",
      "Export can hide credentials, and runs in the background without holding up the app.",
      "HTTP/2 to upstream servers, and apps are kept on TCP (no HTTP/3) so their traffic stays visible.",
      "Insights: clearer wording for transport errors, and cookie findings grouped per site.",
      "Credentials are hidden from what MCP clients read, by default.",
    ],
    fixed: [
      "Changing a filter after ⌘B now applies it, instead of leaving the table unchanged.",
      "A selected flow hidden by the filter is marked as hidden, since Resend and cURL still act on it.",
      "Status codes in the flow table were unreadable on macOS.",
    ],
  },
  {
    version: "0.8.0",
    date: "2026-11-12",
    added: [
      "Settings › General: turn the system proxy on when NovaProxy opens.",
      "Settings › General: record while the system proxy is on — the switch starts and pauses recording with it. On by default.",
      "Quitting with the system proxy on shows a dialog while NovaProxy turns it off, instead of a window that looks frozen.",
    ],
    improved: [
      "The system proxy switch shows that it is working, and ignores clicks until it is done.",
      "Turning the system proxy on and off is logged step by step, so a slow switch can be explained from a log bundle.",
    ],
    fixed: [
      "Turning the system proxy off could leave the machine pointed at 127.0.0.1:9090, cutting it off from the internet.",
      "Quitting right after toggling the system proxy could restore the wrong settings.",
    ],
  },
  {
    version: "0.7.4",
    date: "2026-11-05",
    added: ["Copy a node's value or path from the response Treeview."],
    fixed: [
      "Long capture sessions no longer run out of file descriptors and stop proxying.",
      "The response Treeview loads the body when it was not kept in memory.",
    ],
  },
  {
    version: "0.7.3",
    date: "2026-11-04",
    added: [
      "Insights: NovaProxy finds anti-patterns in captured traffic — request bursts, polling, retries, slow servers — with a traffic report.",
    ],
  },
  {
    version: "0.7.2",
    date: "2026-11-04",
    added: ["Copy a single header, query parameter, cookie or summary value from the inspector."],
    fixed: ["WebSocket connections with many frames no longer grow the app's memory without bound."],
  },
  {
    version: "0.7.1",
    date: "2026-10-30",
    improved: ["Body previews are held compressed and capped at 64 KB, so long sessions use less memory."],
    fixed: [
      "Upstream TLS trusts the operating system's certificate store, and failed requests say why they failed.",
      "Network services that no longer exist no longer break turning the system proxy on.",
    ],
  },
  {
    version: "0.7.0",
    date: "2026-10-29",
    added: [
      "Filter by several apps and domains at once, and save the pick.",
      "Saved filters have their own sidebar section and a dialog to manage them.",
      "A Fetch/XHR chip in the type filter.",
      "App icons in the sidebar's Apps list.",
      "Copy the active tab of the request and response panes.",
    ],
    improved: ["The scope tree and flow stream stay smooth under heavy traffic."],
    fixed: ["Captured rows no longer squeeze the inspector."],
  },
  {
    version: "0.6.5",
    date: "2026-10-27",
    added: ["Windows support, with the system proxy restored on quit and on update."],
  },
  {
    version: "0.6.0",
    date: "2026-09-11",
    improved: ["Turning the system proxy on and off is about twice as fast on macOS."],
    fixed: [
      "Copy as cURL drops the connection header and default port and escapes quotes correctly.",
      "Copying to the clipboard works everywhere in the app.",
      "Dividers no longer jump when clicked.",
      "Toasts tell success and failure apart.",
      "Installing the helper checks that it really landed.",
    ],
  },
  {
    version: "0.5.0",
    date: "2026-08-26",
    added: [
      "The flow list is a table: sort, pick and resize columns, select several rows.",
      "A filter builder — field, operator and value, stacked.",
      "Pin flows, add a note to a flow, and save filters as chips.",
      "Request and Response side by side in the inspector.",
      "Keyboard shortcuts, listed under Help › Keyboard Shortcuts.",
      "Resizable sidebar, table and inspector.",
    ],
  },
  {
    version: "0.4.0",
    date: "2026-08-25",
    added: [
      "Help › Show Log Folder collects a support bundle.",
      "Check for updates from the menu bar.",
    ],
    improved: ["Crashes are recorded, so they can be reported and fixed."],
  },
  {
    version: "0.3.0",
    date: "2026-08-25",
    added: ["A walkthrough that guides a new install through its first capture."],
  },
  {
    version: "0.2.1",
    date: "2026-08-24",
    fixed: ["macOS updates install correctly."],
  },
  {
    version: "0.2.0",
    date: "2026-08-24",
    added: ["A new logo and app icon."],
    fixed: ["The updater only runs in builds that can update themselves."],
  },
  {
    version: "0.1.1",
    date: "2026-08-24",
    added: ["NovaProxy updates itself from signed releases."],
    improved: ["A new interface, and a flow list that stays fast with thousands of rows."],
  },
  {
    version: "0.1.0",
    date: "2026-08-24",
    added: ["First release: capture and inspect HTTP and HTTPS traffic from every app on the machine."],
  },
];

/** Compare dotted versions numerically: `0.10.0` is newer than `0.9.0`. */
export function compareVersions(a: string, b: string): number {
  const pa = a.split(/[.-]/).map((x) => parseInt(x, 10) || 0);
  const pb = b.split(/[.-]/).map((x) => parseInt(x, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/**
 * Whether to open What's New by itself at launch.
 *
 * Only after an update: a fresh install gets the walkthrough instead, and a
 * second launch of the same version gets nothing. `lastSeen` is null on the
 * first launch of a build that has this check; an install that already finished
 * onboarding is an existing user who just updated, so they see it once too.
 */
export function shouldShowWhatsNew(
  lastSeen: string | null,
  current: string,
  onboardingDone: boolean,
  log: Release[] = CHANGELOG,
): boolean {
  const known = log.some((r) => r.version === current);
  if (!known) return false;
  if (lastSeen == null) return onboardingDone;
  return compareVersions(current, lastSeen) > 0;
}

/** Releases newer than `lastSeen`, for marking what this update brought. */
export function isNewSince(release: Release, lastSeen: string | null): boolean {
  return lastSeen != null && compareVersions(release.version, lastSeen) > 0;
}
