/**
 * Pure helpers for the Rules page: grouping rules into scenarios, the Fault
 * mode catalogue, and reading / writing rule files. Kept out of the component
 * so it is testable without a DOM.
 */
import type { Rule } from "./bindings/Rule";
import type { RuleKind } from "./bindings/RuleKind";
import type { FaultMode } from "./bindings/FaultMode";
import type { Fault } from "./bindings/Fault";

export const RULE_KINDS: RuleKind[] = ["MapRemote", "MapLocal", "Block", "Rewrite", "Fault"];
export const RULE_KIND_LABEL: Record<RuleKind, string> = {
  MapRemote: "Map Remote",
  MapLocal: "Map Local",
  Block: "Block",
  Rewrite: "Rewrite",
  Fault: "Fault",
};

export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/** Which of a Fault's fields a mode reads. */
export type FaultField = "delay_ms" | "status" | "body" | "content_type" | "cut_after_bytes";

export interface FaultModeInfo {
  mode: FaultMode;
  label: string;
  /** What the app sees, in one line. */
  hint: string;
  fields: FaultField[];
  /** Forwards the request: the server acts on it. */
  forwards: boolean;
}

export const FAULT_MODES: FaultModeInfo[] = [
  { mode: "DropBefore", label: "Drop before", hint: "Closes the connection without forwarding. The app sees a network error (status 0).", fields: [], forwards: false },
  { mode: "HoldClose", label: "Hold, then close", hint: "Holds the request, then closes without forwarding. Status 0, or the app's own timeout first.", fields: ["delay_ms"], forwards: false },
  { mode: "HoldStatus", label: "Hold, then status", hint: "Holds the request, then answers with a status (504 by default). Not forwarded.", fields: ["delay_ms", "status", "body", "content_type"], forwards: false },
  { mode: "FakeResponse", label: "Fake response", hint: "Answers at once with this status and body. Not forwarded.", fields: ["status", "body", "content_type"], forwards: false },
  { mode: "Waf403", label: "WAF 403", hint: "Answers 403 with an HTML page, like a web application firewall block.", fields: [], forwards: false },
  { mode: "ProxyAuth407", label: "Proxy auth 407", hint: "Answers 407 Proxy Authentication Required; for HTTPS the CONNECT is refused. Matches on host.", fields: [], forwards: false },
  { mode: "PassCut", label: "Forward, drop answer", hint: "Forwards (the server does the work), then drops the response. The app sees status 0.", fields: [], forwards: true },
  { mode: "CutBody", label: "Cut body", hint: "Forwards, passes the head and the first bytes of the body, then closes.", fields: ["cut_after_bytes"], forwards: true },
  { mode: "Delay", label: "Delay response", hint: "Forwards and passes the response intact, after a hold.", fields: ["delay_ms"], forwards: true },
];

export function faultModeInfo(mode: FaultMode): FaultModeInfo {
  return FAULT_MODES.find((m) => m.mode === mode) ?? FAULT_MODES[0];
}

export const FAULT_MAX_DELAY_MS = 600_000;

export function defaultFault(mode: FaultMode = "DropBefore"): Fault {
  return { mode, delay_ms: null, status: null, body: null, content_type: null, cut_after_bytes: null };
}

export function newRule(id: string, group: string | null = null): Rule {
  return {
    id,
    enabled: true,
    kind: "MapRemote",
    name: "new rule",
    pattern: "https://api.example.com/*",
    target: "https://staging.example.com",
    header_name: null,
    header_value: null,
    method: null,
    group,
    times: null,
    fault: null,
  };
}

/** A rule switched to `kind`, with a fault filled in when it becomes a Fault. */
export function withKind(rule: Rule, kind: RuleKind): Rule {
  return { ...rule, kind, fault: kind === "Fault" ? (rule.fault ?? defaultFault()) : rule.fault };
}

export interface RuleGroup {
  /** `null` for rules without a group. */
  name: string | null;
  rules: Rule[];
}

/** Rules without a group first, then each group in order of first appearance. */
export function groupRules(rules: Rule[]): RuleGroup[] {
  const ungrouped: Rule[] = [];
  const groups = new Map<string, Rule[]>();
  for (const r of rules) {
    const g = r.group?.trim();
    if (!g) ungrouped.push(r);
    else groups.set(g, [...(groups.get(g) ?? []), r]);
  }
  const out: RuleGroup[] = [...groups].map(([name, rs]) => ({ name, rules: rs }));
  return ungrouped.length ? [{ name: null, rules: ungrouped }, ...out] : out;
}

/** "on", "off" or "mixed" for a group's switch. */
export function groupState(rules: Rule[]): "on" | "off" | "mixed" {
  const on = rules.filter((r) => r.enabled).length;
  return on === 0 ? "off" : on === rules.length ? "on" : "mixed";
}

/** Case-insensitive match on group name, rule name or pattern. */
export function matchesFilter(rule: Rule, q: string): boolean {
  const needle = q.trim().toLowerCase();
  if (!needle) return true;
  return [rule.group, rule.name, rule.pattern].some((s) => (s ?? "").toLowerCase().includes(needle));
}

/** Same glob as the engine: `*` = any run, case-insensitive, anchored. */
export function globMatch(pattern: string, text: string): boolean {
  const esc = pattern.toLowerCase().split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&"));
  return new RegExp(`^${esc.join(".*")}$`).test(text.toLowerCase());
}

/** The literal host a URL pattern names, or null when the host has a wildcard. */
export function patternHost(pattern: string): string | null {
  const m = /^[a-z*]+:\/\/([^/:?]+)/i.exec(pattern.trim());
  if (!m || m[1].includes("*")) return null;
  return m[1].toLowerCase();
}

/** Why a Fault rule will not act, or null when it can. */
export function faultBlocker(rule: Rule, allowHosts: string[]): string | null {
  if (rule.kind !== "Fault") return null;
  const allow = allowHosts.map((h) => h.trim()).filter(Boolean);
  if (allow.length === 0) return "No Fault host is allowed yet: this rule does nothing until you add its host above.";
  const host = patternHost(rule.pattern);
  if (host && !allow.some((g) => globMatch(g, host))) return `${host} is not in the Fault hosts: this rule does nothing there.`;
  return null;
}

const FILE_TAG = "novaproxy-rules";

/** A rule file: a tagged object so a file of something else is not taken for rules. */
export function rulesToFile(rules: Rule[]): string {
  return JSON.stringify({ format: FILE_TAG, version: 1, rules }, null, 2) + "\n";
}

const KINDS = new Set<string>(RULE_KINDS);
const MODES = new Set<string>(FAULT_MODES.map((m) => m.mode));

function str(v: unknown): string | null {
  return typeof v === "string" && v.trim() !== "" ? v : null;
}
function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) && v >= 0 ? Math.floor(v) : null;
}

/**
 * Rules from a file: a tagged object, a bare array, or `{ rules: [...] }`.
 * Fills what is missing, drops what is not a rule, and gives a rule without
 * an id a fresh one. Throws when nothing in it is a rule.
 */
export function rulesFromFile(text: string, newId: () => string): Rule[] {
  const doc: unknown = JSON.parse(text);
  const list: unknown[] = Array.isArray(doc)
    ? doc
    : doc && typeof doc === "object" && Array.isArray((doc as { rules?: unknown }).rules)
      ? (doc as { rules: unknown[] }).rules
      : [];
  const rules: Rule[] = [];
  for (const raw of list) {
    if (!raw || typeof raw !== "object") continue;
    const r = raw as Record<string, unknown>;
    const kind = typeof r.kind === "string" && KINDS.has(r.kind) ? (r.kind as RuleKind) : null;
    const pattern = str(r.pattern);
    if (!kind || !pattern) continue;
    let fault: Fault | null = null;
    if (r.fault && typeof r.fault === "object") {
      const f = r.fault as Record<string, unknown>;
      const mode = typeof f.mode === "string" && MODES.has(f.mode) ? (f.mode as FaultMode) : null;
      if (mode) {
        fault = {
          mode,
          delay_ms: num(f.delay_ms) === null ? null : Math.min(num(f.delay_ms)!, FAULT_MAX_DELAY_MS),
          status: num(f.status),
          body: typeof f.body === "string" ? f.body : null,
          content_type: str(f.content_type),
          cut_after_bytes: num(f.cut_after_bytes),
        };
      }
    }
    if (kind === "Fault" && !fault) continue;
    const times = num(r.times);
    rules.push({
      id: str(r.id) ?? newId(),
      enabled: r.enabled !== false,
      kind,
      name: str(r.name) ?? pattern,
      pattern,
      target: str(r.target),
      header_name: str(r.header_name),
      header_value: typeof r.header_value === "string" ? r.header_value : null,
      method: str(r.method)?.toUpperCase() ?? null,
      group: str(r.group)?.trim() ?? null,
      times: times && times > 0 ? times : null,
      fault,
    });
  }
  if (rules.length === 0) throw new Error("No rules found in this file");
  return rules;
}

/** `current` with `incoming` added; an incoming rule with an existing id replaces it in place. */
export function mergeRules(current: Rule[], incoming: Rule[]): Rule[] {
  const byId = new Map(incoming.map((r) => [r.id, r]));
  const out = current.map((r) => byId.get(r.id) ?? r);
  const have = new Set(current.map((r) => r.id));
  return [...out, ...incoming.filter((r) => !have.has(r.id))];
}

/** Enabled Fault rules: what the "faults are on" banner counts. */
export function activeFaults(rules: Rule[]): number {
  return rules.filter((r) => r.enabled && r.kind === "Fault").length;
}
