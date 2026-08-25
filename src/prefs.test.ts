import { describe, expect, it } from "vitest";
import { DEFAULT_PREFS, loadPrefs, normalizePrefs, savePrefs } from "./prefs";
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
