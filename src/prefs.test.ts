import { describe, expect, it } from "vitest";
import {
  DEFAULT_PREFS,
  MAX_LIST_WIDTH,
  MIN_LIST_WIDTH,
  clampListWidth,
  loadPrefs,
  normalizePrefs,
  savePrefs,
} from "./prefs";

/** A `localStorage` stand-in, plus one that fails the way a locked-down browser does. */
function fakeStore(seed?: string) {
  const state = { value: seed };
  return {
    getItem: () => state.value ?? null,
    setItem: (_k: string, v: string) => {
      state.value = v;
    },
    read: () => state.value,
  };
}

describe("prefs", () => {
  it("defaults to grouped flows and leaves the system proxy alone", () => {
    // The launch default matters: turning the OS proxy on rewrites a setting the
    // user needs for working internet, so it must be opt-in.
    expect(DEFAULT_PREFS.systemProxyAtLaunch).toBe("none");
    expect(DEFAULT_PREFS.flowGrouping).toBe("grouped");
  });

  it("round-trips a saved preference", () => {
    const store = fakeStore();
    savePrefs({ ...DEFAULT_PREFS, flowGrouping: "flat", systemProxyAtLaunch: "system" }, store);
    const loaded = loadPrefs(store);
    expect(loaded.flowGrouping).toBe("flat");
    expect(loaded.systemProxyAtLaunch).toBe("system");
  });

  it("falls back to defaults on absent, malformed or foreign values", () => {
    expect(loadPrefs(fakeStore())).toEqual(DEFAULT_PREFS);
    expect(loadPrefs(fakeStore("{not json"))).toEqual(DEFAULT_PREFS);
    expect(loadPrefs(fakeStore('{"flowGrouping":"sideways"}')).flowGrouping).toBe("grouped");
    expect(loadPrefs(fakeStore('{"systemProxyAtLaunch":true}')).systemProxyAtLaunch).toBe("none");
    expect(normalizePrefs(null)).toEqual(DEFAULT_PREFS);
  });

  it("keeps the flow list width usable whatever was stored", () => {
    expect(clampListWidth(10)).toBe(MIN_LIST_WIDTH);
    expect(clampListWidth(5000)).toBe(MAX_LIST_WIDTH);
    expect(clampListWidth(500.6)).toBe(501);
    // NaN survives Math.min/Math.max, so it is rejected explicitly — a NaN width
    // would collapse the pane to nothing.
    expect(clampListWidth(NaN)).toBe(DEFAULT_PREFS.flowListWidth);
    expect(loadPrefs(fakeStore('{"flowListWidth":"wide"}')).flowListWidth).toBe(
      DEFAULT_PREFS.flowListWidth,
    );
  });

  it("does not throw when storage refuses to write", () => {
    const hostile = {
      setItem: () => {
        throw new Error("QuotaExceededError");
      },
    };
    expect(() => savePrefs(DEFAULT_PREFS, hostile)).not.toThrow();
  });
});
