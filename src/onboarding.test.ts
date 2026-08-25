import { describe, expect, it } from "vitest";
import type { CaStatus, HelperStatus, ProxyStatus } from "./api";
import { DEFAULT_PREFS } from "./prefs";
import {
  STEPS,
  allComplete,
  firstIncompleteStep,
  launchDecision,
  stepState,
  visibleSteps,
  type Statuses,
} from "./onboarding";

const macHelper: HelperStatus = {
  supported: true,
  running: true,
  version: 1,
  expected_version: 1,
  installable: true,
};

const trustedCa: CaStatus = {
  cert_path: "/tmp/ca.pem",
  fingerprint: "AA:BB",
  trusted: true,
  trusted_user: true,
  trusted_system: true,
  subject: "NovaProxy Root CA",
  platform: "macos",
};

const proxyOn: ProxyStatus = {
  running: true,
  host: "127.0.0.1",
  port: 9090,
  flows_captured: 0n,
  system_proxy: true,
  pending_restore: false,
};

/** Everything done, so each test can spoil exactly one thing. */
function ready(patch: Partial<Statuses> = {}): Statuses {
  return {
    helper: macHelper,
    ca: trustedCa,
    proxy: proxyOn,
    recording: true,
    flowCount: 3,
    ...patch,
  };
}

describe("visibleSteps", () => {
  it("keeps the helper step on macOS", () => {
    expect(visibleSteps(macHelper)).toHaveLength(STEPS.length);
  });

  it("drops the helper step where the platform needs no helper", () => {
    // Windows and Linux set proxy settings per user; a step that cannot be
    // completed reads as a broken install.
    const ids = visibleSteps({ ...macHelper, supported: false, running: false }).map((s) => s.id);
    expect(ids).not.toContain("helper");
    expect(ids).toEqual(["certificate", "proxy", "capture"]);
  });

  it("keeps every step while the status is still loading", () => {
    expect(visibleSteps(null)).toHaveLength(STEPS.length);
  });
});

describe("stepState", () => {
  it("counts a running, current helper as done", () => {
    expect(stepState("helper", ready()).done).toBe(true);
  });

  it("counts a protocol-mismatched helper as unfinished", () => {
    // It answers pings, so `running` is true — but it cannot be trusted to
    // apply this build's requests.
    const s = stepState("helper", ready({ helper: { ...macHelper, version: 0 } }));
    expect(s.done).toBe(false);
    expect(s.note).toMatch(/older helper/i);
  });

  it("says so when no helper binary shipped", () => {
    const s = stepState(
      "helper",
      ready({ helper: { ...macHelper, running: false, version: null, installable: false } }),
    );
    expect(s.done).toBe(false);
    expect(s.note).toMatch(/nova-helper/);
  });

  it("treats a user-only cert as done but qualified", () => {
    const s = stepState(
      "certificate",
      ready({ ca: { ...trustedCa, trusted_system: false } }),
    );
    expect(s.done).toBe(true);
    expect(s.note).toMatch(/your login/i);
  });

  it("treats an untrusted or missing cert as unfinished", () => {
    expect(stepState("certificate", ready({ ca: { ...trustedCa, trusted: false } })).done).toBe(false);
    expect(stepState("certificate", ready({ ca: null })).done).toBe(false);
  });

  it("follows the live system-proxy flag", () => {
    expect(stepState("proxy", ready()).done).toBe(true);
    expect(stepState("proxy", ready({ proxy: { ...proxyOn, system_proxy: false } })).done).toBe(false);
  });

  it("needs a captured flow, not just an armed recorder", () => {
    expect(stepState("capture", ready({ flowCount: 0, recording: true })).done).toBe(false);
    expect(stepState("capture", ready({ flowCount: 0, recording: true })).note).toMatch(/make a request/i);
    expect(stepState("capture", ready({ flowCount: 0, recording: false })).note).toBeUndefined();
    expect(stepState("capture", ready({ flowCount: 1 })).done).toBe(true);
  });
});

describe("firstIncompleteStep", () => {
  const steps = visibleSteps(macHelper);

  it("opens on the first thing left to do", () => {
    const s = ready({ ca: { ...trustedCa, trusted: false } });
    expect(firstIncompleteStep(steps, s)).toBe(1);
  });

  it("skips past work already done on a half-set-up machine", () => {
    const s = ready({ proxy: { ...proxyOn, system_proxy: false } });
    expect(firstIncompleteStep(steps, s)).toBe(2);
  });

  it("lands on the last step when nothing is outstanding", () => {
    expect(firstIncompleteStep(steps, ready())).toBe(steps.length - 1);
    expect(allComplete(steps, ready())).toBe(true);
  });

  it("indexes the visible steps, not the full list", () => {
    // Without the helper step, "certificate outstanding" is index 0.
    const noHelper = visibleSteps({ ...macHelper, supported: false });
    const s = ready({
      helper: { ...macHelper, supported: false, running: false },
      ca: { ...trustedCa, trusted: false },
    });
    expect(firstIncompleteStep(noHelper, s)).toBe(0);
  });
});

describe("launchDecision", () => {
  it("opens the wizard on a fresh install", () => {
    expect(launchDecision(DEFAULT_PREFS, { ...trustedCa, trusted: false })).toBe("open");
    expect(launchDecision(DEFAULT_PREFS, null)).toBe("open");
  });

  it("marks an existing install done without showing anything", () => {
    // The upgrade path: a 0.2.1 pref blob has no flag, but a trusted CA proves
    // this user already found their way around.
    expect(launchDecision(DEFAULT_PREFS, trustedCa)).toBe("mark-done");
  });

  it("stays out of the way once the flag is set", () => {
    const done = { ...DEFAULT_PREFS, onboardingDone: true };
    expect(launchDecision(done, null)).toBe("nothing");
    expect(launchDecision(done, { ...trustedCa, trusted: false })).toBe("nothing");
  });
});
