import { describe, expect, it } from "vitest";
import pkg from "../package.json";
import { CHANGELOG, compareVersions, shouldShowWhatsNew } from "./changelog";

describe("changelog", () => {
  it("has an entry for the version being built", () => {
    // A release bumps package.json; this fails until What's New says what it brought.
    expect(CHANGELOG[0].version).toBe(pkg.version);
  });

  it("is newest first, with no duplicate versions and real dates", () => {
    for (let i = 1; i < CHANGELOG.length; i++) {
      expect(compareVersions(CHANGELOG[i - 1].version, CHANGELOG[i].version)).toBeGreaterThan(0);
    }
    for (const r of CHANGELOG) {
      expect(r.date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      expect((r.added?.length ?? 0) + (r.improved?.length ?? 0) + (r.fixed?.length ?? 0)).toBeGreaterThan(0);
    }
  });

  it("compares versions numerically", () => {
    expect(compareVersions("0.10.0", "0.9.0")).toBeGreaterThan(0);
    expect(compareVersions("0.8.0", "0.8.0")).toBe(0);
    expect(compareVersions("0.7.4", "0.8.0")).toBeLessThan(0);
    expect(compareVersions("1.0", "1.0.0")).toBe(0);
  });
});

describe("shouldShowWhatsNew", () => {
  const log = [
    { version: "0.9.0", date: "2026-12-01", added: ["x"] },
    { version: "0.8.0", date: "2026-11-12", fixed: ["y"] },
  ];

  it("opens after an update", () => {
    expect(shouldShowWhatsNew("0.8.0", "0.9.0", true, log)).toBe(true);
  });

  it("stays shut on the same version, and on a downgrade", () => {
    expect(shouldShowWhatsNew("0.9.0", "0.9.0", true, log)).toBe(false);
    expect(shouldShowWhatsNew("0.9.0", "0.8.0", true, log)).toBe(false);
  });

  it("leaves a fresh install to the walkthrough", () => {
    expect(shouldShowWhatsNew(null, "0.9.0", false, log)).toBe(false);
  });

  it("shows an existing user the first build that has the check", () => {
    expect(shouldShowWhatsNew(null, "0.9.0", true, log)).toBe(true);
  });

  it("says nothing for a version the changelog does not know", () => {
    expect(shouldShowWhatsNew("0.8.0", "0.9.1", true, log)).toBe(false);
  });
});
