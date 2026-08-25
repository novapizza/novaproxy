import { describe, expect, it } from "vitest";
import {
  DEFAULT_PREFS,
  INSPECTOR_PCT,
  loadPrefs,
  normalizePrefs,
  REQUEST_PCT,
  savePrefs,
  TREE_W,
} from "./prefs";
import { DEFAULT_COLUMNS } from "./flows/columns";

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
  it("leaves the system proxy alone and does not follow the tail", () => {
    // The launch default matters: turning the OS proxy on rewrites a setting the
    // user needs for working internet, so it must be opt-in.
    expect(DEFAULT_PREFS.systemProxyAtLaunch).toBe("none");
    expect(DEFAULT_PREFS.autoSelect).toBe(false);
    expect(DEFAULT_PREFS.columns).toEqual(DEFAULT_COLUMNS);
  });

  it("round-trips a saved preference", () => {
    const store = fakeStore();
    savePrefs(
      { ...DEFAULT_PREFS, autoSelect: true, columns: ["url", "status"], systemProxyAtLaunch: "system" },
      store,
    );
    const loaded = loadPrefs(store);
    expect(loaded.autoSelect).toBe(true);
    expect(loaded.columns).toEqual(["url", "status"]);
    expect(loaded.systemProxyAtLaunch).toBe("system");
  });

  it("falls back to defaults on absent, malformed or foreign values", () => {
    expect(loadPrefs(fakeStore())).toEqual(DEFAULT_PREFS);
    expect(loadPrefs(fakeStore("{not json"))).toEqual(DEFAULT_PREFS);
    expect(loadPrefs(fakeStore('{"columns":["nope"]}')).columns).toEqual(DEFAULT_COLUMNS);
    expect(loadPrefs(fakeStore('{"systemProxyAtLaunch":true}')).systemProxyAtLaunch).toBe("none");
    expect(normalizePrefs(null)).toEqual(DEFAULT_PREFS);
  });

  it("drops what the list view stored, rather than trying to translate it", () => {
    // A blob from a build that had the list carries `flowGrouping` and
    // `flowListWidth` and no `columns`. Grouping is not a column choice, so
    // there is nothing to migrate *from*: the default set stands in and the two
    // dead keys go away.
    const old = '{"flowGrouping":"flat","flowListWidth":412,"systemProxyAtLaunch":"system"}';
    const loaded = loadPrefs(fakeStore(old));
    expect(loaded.columns).toEqual(DEFAULT_COLUMNS);
    expect(loaded.autoSelect).toBe(false);
    // …while the preferences that still exist survive the upgrade untouched.
    expect(loaded.systemProxyAtLaunch).toBe("system");
    expect(Object.keys(loaded)).not.toContain("flowGrouping");
    expect(Object.keys(loaded)).not.toContain("flowListWidth");
  });

  it("treats a pref blob without an onboarding flag as not yet onboarded", () => {
    // The inverse of `autoCheckUpdates`: a blob written by 0.2.1 has no such
    // field, and that has to mean "show the walkthrough", not "already done".
    expect(DEFAULT_PREFS.onboardingDone).toBe(false);
    expect(loadPrefs(fakeStore('{"flowGrouping":"flat"}')).onboardingDone).toBe(false);
    expect(normalizePrefs({ onboardingDone: "yes" }).onboardingDone).toBe(false);
    expect(normalizePrefs({ onboardingDone: true }).onboardingDone).toBe(true);
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

describe("divider sizes", () => {
  const load = (json: string) => loadPrefs(fakeStore(json));

  it("keeps every divider inside a range that leaves the pane usable", () => {
    // A 40px sidebar is a column of ellipses and a 5% inspector is a tab strip
    // with nothing under it, so the floors are about usefulness, not rendering.
    expect(load('{"treeWidth":10}').treeWidth).toBe(TREE_W.min);
    expect(load('{"treeWidth":9999}').treeWidth).toBe(TREE_W.max);
    expect(load('{"inspectorPct":1}').inspectorPct).toBe(INSPECTOR_PCT.min);
    expect(load('{"inspectorPct":95}').inspectorPct).toBe(INSPECTOR_PCT.max);
    expect(load('{"requestPct":0}').requestPct).toBe(REQUEST_PCT.min);
    expect(load('{"requestPct":100}').requestPct).toBe(REQUEST_PCT.max);
  });

  it("rounds a dragged fraction, and rejects what is not a number", () => {
    expect(load('{"treeWidth":252.7}').treeWidth).toBe(253);
    // NaN survives Math.min/Math.max, so it is rejected explicitly — a NaN width
    // would collapse the pane to nothing.
    expect(load('{"treeWidth":"wide"}').treeWidth).toBe(DEFAULT_PREFS.treeWidth);
    expect(normalizePrefs({ inspectorPct: null }).inspectorPct).toBe(DEFAULT_PREFS.inspectorPct);
  });

  it("round-trips a drag", () => {
    const store = fakeStore();
    savePrefs({ ...DEFAULT_PREFS, treeWidth: 320, inspectorPct: 55, requestPct: 35 }, store);
    const back = loadPrefs(store);
    expect([back.treeWidth, back.inspectorPct, back.requestPct]).toEqual([320, 55, 35]);
  });
});
