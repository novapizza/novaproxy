import { describe, expect, it } from "vitest";
import type { Rule } from "./api";
import {
  activeFaults,
  defaultFault,
  faultBlocker,
  globMatch,
  groupRules,
  groupState,
  matchesFilter,
  mergeRules,
  newRule,
  patternHost,
  rulesFromFile,
  rulesToFile,
  withKind,
} from "./rules";

const r = (over: Partial<Rule> = {}): Rule => ({ ...newRule("r1"), ...over });
let n = 0;
const nextId = () => `gen${++n}`;

describe("groupRules", () => {
  it("puts ungrouped first, then groups in order of first appearance", () => {
    const rules = [r({ id: "a", group: "B" }), r({ id: "b" }), r({ id: "c", group: "A" }), r({ id: "d", group: "B" })];
    expect(groupRules(rules).map((g) => [g.name, g.rules.map((x) => x.id)])).toEqual([
      [null, ["b"]],
      ["B", ["a", "d"]],
      ["A", ["c"]],
    ]);
  });
  it("treats a blank group as no group", () => {
    expect(groupRules([r({ group: "  " })])[0].name).toBeNull();
  });
});

describe("groupState", () => {
  it("is on, off or mixed", () => {
    expect(groupState([r(), r()])).toBe("on");
    expect(groupState([r({ enabled: false })])).toBe("off");
    expect(groupState([r(), r({ enabled: false })])).toBe("mixed");
  });
});

describe("matchesFilter", () => {
  it("looks at group, name and pattern", () => {
    const rule = r({ group: "LH A3", name: "submit", pattern: "https://x/save" });
    expect(matchesFilter(rule, "a3")).toBe(true);
    expect(matchesFilter(rule, "SUBMIT")).toBe(true);
    expect(matchesFilter(rule, "/save")).toBe(true);
    expect(matchesFilter(rule, "nope")).toBe(false);
    expect(matchesFilter(rule, "  ")).toBe(true);
  });
});

describe("globMatch / patternHost", () => {
  it("matches like the engine", () => {
    expect(globMatch("*.example.com", "api.example.com")).toBe(true);
    expect(globMatch("*.example.com", "example.org")).toBe(false);
    expect(globMatch("a.b", "aXb")).toBe(false);
  });
  it("finds a literal host only", () => {
    expect(patternHost("https://api.dev.example.com/v1/*")).toBe("api.dev.example.com");
    expect(patternHost("https://*.example.com/*")).toBeNull();
    expect(patternHost("*/graphql")).toBeNull();
  });
});

describe("faultBlocker", () => {
  const fault = r({ kind: "Fault", fault: defaultFault(), pattern: "https://api.dev.example.com/*" });
  it("explains an empty allowlist", () => {
    expect(faultBlocker(fault, [])).toMatch(/No Fault host/);
  });
  it("names a host that is not allowed", () => {
    expect(faultBlocker(fault, ["*.prod.example.com"])).toMatch(/api.dev.example.com/);
  });
  it("is quiet when the host is allowed or the rule is not a fault", () => {
    expect(faultBlocker(fault, ["*.dev.example.com"])).toBeNull();
    expect(faultBlocker(r(), [])).toBeNull();
  });
});

describe("withKind", () => {
  it("adds a fault when switching to Fault and keeps it otherwise", () => {
    const f = withKind(r(), "Fault");
    expect(f.fault?.mode).toBe("DropBefore");
    expect(withKind(f, "Block").fault).toEqual(f.fault);
  });
});

describe("rule files", () => {
  it("round-trips", () => {
    const rules = [r({ kind: "Fault", group: "G", times: 1, method: "POST", fault: { ...defaultFault("PassCut") } })];
    expect(rulesFromFile(rulesToFile(rules), nextId)).toEqual(rules);
  });
  it("accepts a bare array and fills what is missing", () => {
    const [rule] = rulesFromFile(JSON.stringify([{ kind: "Block", pattern: "https://x/*", method: "post" }]), nextId);
    expect(rule.id).toMatch(/^gen/);
    expect(rule.enabled).toBe(true);
    expect(rule.method).toBe("POST");
    expect(rule.name).toBe("https://x/*");
  });
  it("drops what is not a rule and caps delays", () => {
    const rules = rulesFromFile(
      JSON.stringify({
        rules: [
          { kind: "Nope", pattern: "x" },
          { kind: "Fault", pattern: "x" },
          { kind: "Fault", pattern: "x", fault: { mode: "HoldClose", delay_ms: 9_999_999 } },
          { kind: "Block" },
        ],
      }),
      nextId,
    );
    expect(rules).toHaveLength(1);
    expect(rules[0].fault?.delay_ms).toBe(600_000);
  });
  it("throws when nothing is a rule", () => {
    expect(() => rulesFromFile("{}", nextId)).toThrow(/No rules/);
  });
});

describe("mergeRules / activeFaults", () => {
  it("replaces by id in place and appends the rest", () => {
    const merged = mergeRules([r({ id: "a", name: "old" }), r({ id: "b" })], [r({ id: "a", name: "new" }), r({ id: "c" })]);
    expect(merged.map((x) => [x.id, x.name])).toEqual([
      ["a", "new"],
      ["b", "new rule"],
      ["c", "new rule"],
    ]);
  });
  it("counts enabled faults", () => {
    expect(activeFaults([r({ kind: "Fault" }), r({ kind: "Fault", enabled: false }), r()])).toBe(1);
  });
});
