import { describe, expect, it } from "vitest";
import type { Insight } from "./bindings/Insight";
import type { InsightReport } from "./bindings/InsightReport";
import { badge, filterForInsights, insightScope, insightsInView, partition, severityCounts, toggleMuted } from "./insights";
import { matchScope, describeScope, scopeKey } from "./scope";
import { filterFromJson, filterToJson, EMPTY_FILTER } from "./filter";
import { normalizePrefs } from "./prefs";
import type { Flow } from "./api";

function insight(id: string, severity: Insight["severity"], ids: string[] = []): Insight {
  return {
    id,
    rule: id.split(":")[0],
    severity,
    title: `title ${id}`,
    detail: "",
    advice: "",
    app: null,
    host: "api.test",
    flow_ids: ids,
    count: ids.length,
    last_seen: 0,
  };
}

const report = (...insights: Insight[]): InsightReport => ({ insights, analyzed: 10, generation: 1 });

describe("partition", () => {
  it("separates muted findings and keeps the engine's order", () => {
    const r = report(insight("a:1", "High"), insight("b:1", "Warning"), insight("c:1", "Info"));
    const { shown, muted } = partition(r, new Set(["b:1"]));
    expect(shown.map((i) => i.id)).toEqual(["a:1", "c:1"]);
    expect(muted.map((i) => i.id)).toEqual(["b:1"]);
  });

  it("treats no report as nothing found", () => {
    expect(partition(null, new Set())).toEqual({ shown: [], muted: [] });
  });
});

describe("badge", () => {
  it("counts unmuted findings and reports the worst severity", () => {
    const r = report(insight("a:1", "Info"), insight("b:1", "High"), insight("c:1", "Warning"));
    expect(badge(r, new Set())).toEqual({ count: 3, severity: "High" });
    expect(badge(r, new Set(["b:1"]))).toEqual({ count: 2, severity: "Warning" });
  });

  it("disappears rather than showing zero", () => {
    expect(badge(report(insight("a:1", "High")), new Set(["a:1"]))).toBeNull();
    expect(badge(null, new Set())).toBeNull();
  });
});

describe("severityCounts", () => {
  it("counts per level", () => {
    expect(severityCounts([insight("a", "High"), insight("b", "High"), insight("c", "Info")])).toEqual({
      High: 2,
      Warning: 0,
      Info: 1,
    });
  });
});

describe("insight scope", () => {
  const scope = insightScope(insight("burst:x", "Warning", ["f1", "f3"]));

  it("matches exactly the finding's flows", () => {
    expect(matchScope({ id: "f1" } as Flow, scope)).toBe(true);
    expect(matchScope({ id: "f2" } as Flow, scope)).toBe(false);
  });

  it("is named and keyed by the finding", () => {
    expect(describeScope(scope)).toBe("title burst:x");
    expect(scopeKey(scope)).toBe("insight:burst:x");
  });

  it("is not saved with a filter, like a pin's flows", () => {
    const saved = filterFromJson(JSON.parse(JSON.stringify(filterToJson({ ...EMPTY_FILTER, scope }))));
    expect(saved.scope).toEqual({ kind: "all" });
  });
});

describe("muting", () => {
  it("toggles an id in and out", () => {
    expect(toggleMuted([], "a")).toEqual(["a"]);
    expect(toggleMuted(["a", "b"], "a")).toEqual(["b"]);
  });

  it("survives in prefs, dropping junk and duplicates", () => {
    expect(normalizePrefs({ mutedInsights: ["a", 3, "a", "b"] }).mutedInsights).toEqual(["a", "b"]);
    expect(normalizePrefs({}).mutedInsights).toEqual([]);
  });
});

describe("following the flow filter", () => {
  const flows = [
    { id: "c1", process: "Chrome" },
    { id: "c2", process: "Chrome" },
    { id: "s1", process: "Slack" },
  ] as Flow[];
  const chromeOnly = (f: Flow) => f.process === "Chrome";

  it("keeps findings with at least one flow in view and counts them", () => {
    const r = report(insight("a:1", "High", ["c1", "s1"]), insight("b:1", "Info", ["s1"]), insight("c:1", "Info", ["c1", "c2"]));
    const { report: kept, inView, hidden } = insightsInView(r, flows, chromeOnly);
    expect(kept?.insights.map((i) => i.id)).toEqual(["a:1", "c:1"]);
    expect(inView.get("a:1")).toBe(1);
    expect(inView.get("c:1")).toBe(2);
    expect(hidden).toBe(1);
  });

  it("drops findings whose flows have all left the list", () => {
    const r = report(insight("a:1", "High", ["gone"]));
    expect(insightsInView(r, flows, () => true).report?.insights).toEqual([]);
  });

  it("ignores the scope this page itself set with Show flows", () => {
    const scope = insightScope(insight("a:1", "High", ["c1"]));
    expect(filterForInsights({ ...EMPTY_FILTER, scope }).scope).toEqual({ kind: "all" });
    const app = { kind: "app", name: "Chrome" } as const;
    expect(filterForInsights({ ...EMPTY_FILTER, scope: app }).scope).toBe(app);
  });
});
