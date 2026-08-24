import { describe, expect, it } from "vitest";
import type { UpdateStatus } from "./bindings/UpdateStatus";
import {
  INITIAL_UPDATE_STATE,
  afterCheck,
  afterProgress,
  canAct,
  formatBytes,
  progressPercent,
  updateSummary,
} from "./update";

const status = (over: Partial<UpdateStatus> = {}): UpdateStatus => ({
  current_version: "0.1.0",
  configured: true,
  available: false,
  version: null,
  notes: null,
  date: null,
  ...over,
});

describe("update", () => {
  it("separates 'up to date' from 'cannot update at all'", () => {
    // A development build has no endpoint. Saying "up to date" there would be a
    // lie the user acts on, so it gets its own phase and its own sentence.
    expect(afterCheck(status()).phase).toBe("current");
    expect(afterCheck(status({ configured: false })).phase).toBe("unconfigured");
    expect(updateSummary(afterCheck(status({ configured: false })))).toContain("manually");
    expect(updateSummary(afterCheck(status()))).toBe("Up to date — version 0.1.0");
  });

  it("names the version on offer", () => {
    const state = afterCheck(status({ available: true, version: "0.2.0" }));
    expect(state.phase).toBe("available");
    expect(updateSummary(state)).toBe("Version 0.2.0 is available");
  });

  it("reports a percentage only when the server sent a length", () => {
    expect(progressPercent(null)).toBeNull();
    expect(progressPercent({ downloaded: 50n, total: null, done: false })).toBeNull();
    // A zero total would divide by zero rather than mean "nothing to download".
    expect(progressPercent({ downloaded: 0n, total: 0n, done: false })).toBeNull();
    expect(progressPercent({ downloaded: 512n, total: 1024n, done: false })).toBe(50);
    // Bytes past the advertised length (a server that under-reported) clamp.
    expect(progressPercent({ downloaded: 2048n, total: 1024n, done: false })).toBe(100);
  });

  it("falls back to bytes downloaded when there is no total", () => {
    const state = afterProgress(afterCheck(status({ available: true, version: "0.2.0" })), {
      downloaded: 2_097_152n,
      total: null,
      done: false,
    });
    expect(state.phase).toBe("downloading");
    expect(updateSummary(state)).toBe("Downloading — 2.0 MB");
  });

  it("switches to installing on the done event and keeps the last progress", () => {
    const downloading = afterProgress(afterCheck(status({ available: true })), {
      downloaded: 900n,
      total: 1000n,
      done: false,
    });
    // The done event carries no counts; overwriting progress with its zeros
    // would make a finished bar jump back to empty.
    const installing = afterProgress(downloading, { downloaded: 0n, total: null, done: true });
    expect(installing.phase).toBe("installing");
    expect(progressPercent(installing.progress)).toBe(90);
    expect(updateSummary(installing)).toContain("restart");
  });

  it("locks the button only while work is in flight", () => {
    expect(canAct(INITIAL_UPDATE_STATE)).toBe(true);
    expect(canAct({ ...INITIAL_UPDATE_STATE, phase: "checking" })).toBe(false);
    expect(canAct({ ...INITIAL_UPDATE_STATE, phase: "downloading" })).toBe(false);
    expect(canAct({ ...INITIAL_UPDATE_STATE, phase: "installing" })).toBe(false);
    // An error must stay retryable — the usual cause is a dropped network.
    expect(canAct({ ...INITIAL_UPDATE_STATE, phase: "error" })).toBe(true);
  });

  it("surfaces the backend's error text rather than a generic one", () => {
    const state = { ...INITIAL_UPDATE_STATE, phase: "error" as const, error: "dns error" };
    expect(updateSummary(state)).toBe("dns error");
    expect(updateSummary({ ...state, error: null })).toBe("Update check failed");
  });

  it("formats sizes the way a download line reads", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(9_869_928)).toBe("9.4 MB");
    expect(formatBytes(15_728_640)).toBe("15 MB");
    expect(formatBytes(null)).toBe("0 B");
    expect(formatBytes(-1)).toBe("—");
  });
});
