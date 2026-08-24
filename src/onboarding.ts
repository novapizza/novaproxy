/**
 * The first-run walkthrough, as data.
 *
 * A new install cannot capture anything until three separate pieces of OS
 * plumbing are in place — the privileged helper, a trusted root CA, and the
 * system proxy — and each lives behind a different corner of the UI. This
 * module holds the order they go in and how to tell, from the statuses the app
 * already fetches, which ones are done. Keeping it out of the view tree is what
 * makes "would this machine see the wizard, and at which step?" a unit test
 * rather than a manual reinstall.
 *
 * Nothing here performs an action. Every step is something the user clicks:
 * two of the three raise an OS trust or password prompt, and the app's standing
 * rule is that those never appear unasked.
 */

import type { CaStatus, HelperStatus, ProxyStatus } from "./api";
import type { IconName } from "./icons";
import type { Prefs } from "./prefs";

export type StepId = "helper" | "certificate" | "proxy" | "capture";

export interface StepMeta {
  id: StepId;
  title: string;
  icon: IconName;
  /** What the step buys, in one sentence. */
  blurb: string;
  /** Label of the button that performs it. */
  action: string;
}

/**
 * Order matters and is not arbitrary: the helper comes first because having it
 * is what makes step 3 free of a password prompt, and the certificate comes
 * before the proxy because traffic captured without it is opaque.
 */
export const STEPS: StepMeta[] = [
  {
    id: "helper",
    title: "Privileged helper",
    icon: "lock",
    blurb:
      "Changing the macOS system proxy needs root. A small helper daemon does it for you, so NovaProxy never has to raise a password dialog again.",
    action: "Install helper",
  },
  {
    id: "certificate",
    title: "Root certificate",
    icon: "shield-check",
    blurb:
      "NovaProxy decrypts HTTPS with a root CA generated on this machine. Until your OS trusts it, secure traffic stays opaque.",
    action: "Install & trust",
  },
  {
    id: "proxy",
    title: "System proxy",
    icon: "power",
    blurb:
      "Point the OS at NovaProxy so your apps' requests come through it. Turning this off puts your previous settings back.",
    action: "Enable system proxy",
  },
  {
    id: "capture",
    title: "Capture something",
    icon: "circle-dot",
    blurb:
      "The list starts paused. Press Record and the requests your apps make land here — click one to read its headers, body and timings.",
    action: "Start recording",
  },
];

/**
 * The steps this machine actually has to do. Only macOS needs a privileged
 * helper; elsewhere the proxy settings are per-user, and showing a step that
 * cannot be completed reads as a broken install.
 */
export function visibleSteps(helper: HelperStatus | null): StepMeta[] {
  if (helper && !helper.supported) return STEPS.filter((s) => s.id !== "helper");
  return STEPS;
}

export interface StepState {
  done: boolean;
  /** Present when the step is done but wants qualifying — e.g. a stale helper. */
  note?: string;
}

export interface Statuses {
  helper: HelperStatus | null;
  ca: CaStatus | null;
  proxy: ProxyStatus;
  recording: boolean;
  flowCount: number;
}

/**
 * Whether a step is satisfied, read from live status rather than from anything
 * the walkthrough remembers — a cert removed between launches has to un-tick.
 */
export function stepState(id: StepId, s: Statuses): StepState {
  switch (id) {
    case "helper": {
      const h = s.helper;
      if (!h) return { done: false };
      // A helper speaking an older protocol answers pings but cannot be trusted
      // to apply this build's requests, so it counts as unfinished.
      if (h.running && h.version !== h.expected_version) {
        return { done: false, note: "An older helper is installed — reinstall it to match this build." };
      }
      if (h.running) return { done: true };
      if (!h.installable) {
        return { done: false, note: "No helper binary shipped with this build (cargo build -p nova-helper)." };
      }
      return { done: false };
    }
    case "certificate": {
      const ca = s.ca;
      if (!ca?.trusted) return { done: false };
      return ca.trusted_system
        ? { done: true }
        : { done: true, note: "Trusted for your login. Other accounts and root-owned daemons still reject it." };
    }
    case "proxy":
      return { done: s.proxy.system_proxy };
    case "capture":
      // Recording alone is a promise; a flow in the list is the proof.
      return s.flowCount > 0
        ? { done: true }
        : { done: false, note: s.recording ? "Recording — make a request and it will appear." : undefined };
  }
}

/**
 * Index into `visibleSteps` of the first unfinished step, so the wizard opens
 * where there is work rather than always at one. Everything done → the last
 * step, which is the one that says so.
 */
export function firstIncompleteStep(steps: StepMeta[], s: Statuses): number {
  const at = steps.findIndex((step) => !stepState(step.id, s).done);
  return at === -1 ? Math.max(0, steps.length - 1) : at;
}

/** Whether every visible step is satisfied. */
export function allComplete(steps: StepMeta[], s: Statuses): boolean {
  return steps.every((step) => stepState(step.id, s).done);
}

export type LaunchDecision = "open" | "mark-done" | "nothing";

/**
 * What to do about the walkthrough at launch.
 *
 * `mark-done` is the upgrade path: someone who has been running NovaProxy since
 * before this flag existed has a trusted CA and does not need to be taught the
 * app. Recording the flag silently is what keeps the wizard from appearing once
 * for every existing user.
 *
 * Call only once `caStatus` has resolved — deciding from a null CA would open
 * the wizard on top of a working install for the split second before the status
 * lands.
 */
export function launchDecision(prefs: Prefs, ca: CaStatus | null): LaunchDecision {
  if (prefs.onboardingDone) return "nothing";
  return ca?.trusted ? "mark-done" : "open";
}
