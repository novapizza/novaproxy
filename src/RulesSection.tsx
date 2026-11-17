import { useEffect, useMemo, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, type Rule, type RuleHit } from "./api";
import type { Fault } from "./bindings/Fault";
import type { FaultMode } from "./bindings/FaultMode";
import type { RuleKind } from "./bindings/RuleKind";
import type { ShowToast } from "./filter";
import { Icon } from "./icons";
import {
  FAULT_MAX_DELAY_MS,
  FAULT_MODES,
  METHODS,
  RULE_KINDS,
  RULE_KIND_LABEL,
  activeFaults,
  faultBlocker,
  faultModeInfo,
  groupRules,
  groupState,
  matchesFilter,
  mergeRules,
  newRule,
  rulesFromFile,
  rulesToFile,
  withKind,
} from "./rules";

/** How often the page re-reads hit counts while it is open. */
const HITS_POLL_MS = 1000;
/** Show the filter box once the list is this long. */
const FILTER_FROM = 8;

const newId = () => "r" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);

/** A number field's value, or null when empty / not a number. */
function intOrNull(v: string, max?: number): number | null {
  if (v.trim() === "") return null;
  const n = Math.floor(Number(v));
  if (!Number.isFinite(n) || n < 0) return null;
  return max != null ? Math.min(n, max) : n;
}

export function RulesSection({
  rules,
  saveRules,
  showToast,
}: {
  rules: Rule[];
  saveRules: (r: Rule[]) => void;
  showToast: ShowToast;
}) {
  const [hits, setHits] = useState<Record<string, number>>({});
  const [allowHosts, setAllowHosts] = useState<string[]>([]);
  const [hostsDraft, setHostsDraft] = useState("");
  const [filter, setFilter] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  useEffect(() => {
    api
      .getFaultSettings()
      .then((s) => {
        setAllowHosts(s.allow_hosts);
        setHostsDraft(s.allow_hosts.join(", "));
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    let live = true;
    const poll = () =>
      api
        .getRuleHits()
        .then((list: RuleHit[]) => live && setHits(Object.fromEntries(list.map((h) => [h.rule_id, h.hits]))))
        .catch(() => {});
    poll();
    const t = window.setInterval(poll, HITS_POLL_MS);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, []);

  const groups = useMemo(() => groupRules(rules.filter((r) => matchesFilter(r, filter))), [rules, filter]);
  const groupNames = useMemo(() => groupRules(rules).flatMap((g) => (g.name ? [g.name] : [])), [rules]);
  const faultsOn = activeFaults(rules);
  const hasFaults = rules.some((r) => r.kind === "Fault");

  const update = (id: string, patch: Partial<Rule>) =>
    saveRules(rules.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  const del = (id: string) => saveRules(rules.filter((r) => r.id !== id));
  const addRule = (group: string | null = null) => saveRules([...rules, newRule(newId(), group)]);

  const resetHits = (ids?: string[]) => {
    api
      .resetRuleHits(ids)
      .then(() =>
        setHits((h) => (ids ? Object.fromEntries(Object.entries(h).filter(([k]) => !ids.includes(k))) : {})),
      )
      .catch((e) => showToast(String(e), { kind: "error" }));
  };

  const saveHosts = () => {
    const list = hostsDraft
      .split(/[\s,]+/)
      .map((h) => h.trim())
      .filter(Boolean);
    if (list.join(",") === allowHosts.join(",")) return;
    api
      .setFaultSettings({ allow_hosts: list })
      .then(() => {
        setAllowHosts(list);
        setHostsDraft(list.join(", "));
        showToast(list.length ? `Fault hosts saved (${list.length})` : "Fault hosts cleared: Fault rules are off everywhere");
      })
      .catch((e) => showToast(String(e), { kind: "error" }));
  };

  const setGroupEnabled = (name: string, enabled: boolean) =>
    saveRules(rules.map((r) => (r.group === name ? { ...r, enabled } : r)));

  async function importRules() {
    try {
      const picked = await open({
        title: "Import rules",
        multiple: false,
        filters: [{ name: "NovaProxy rules", extensions: ["json"] }],
      });
      const path = Array.isArray(picked) ? picked[0] : picked;
      if (!path) return;
      const incoming = rulesFromFile(await api.readFile(path), newId);
      saveRules(mergeRules(rules, incoming));
      showToast(`Imported ${incoming.length} rule${incoming.length === 1 ? "" : "s"}`);
    } catch (e) {
      showToast(`Import failed: ${String(e)}`, { kind: "error" });
    }
  }

  async function exportRules(subset: Rule[], name: string) {
    try {
      const path = await save({
        title: "Export rules",
        defaultPath: `${name.replace(/[^\w.-]+/g, "-").replace(/^-+|-+$/g, "") || "rules"}.json`,
        filters: [{ name: "NovaProxy rules", extensions: ["json"] }],
      });
      if (!path) return;
      await api.writeFile(path, rulesToFile(subset));
      showToast(`Exported ${subset.length} rule${subset.length === 1 ? "" : "s"}`);
    } catch (e) {
      showToast(String(e), { kind: "error" });
    }
  }

  const toggleCollapsed = (key: string) =>
    setCollapsed((s) => {
      const next = new Set(s);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  return (
    <div className="page">
      <div className="page-inner w720">
        <div className="page-head rules-head">
          <h2 className="page-title">Rules</h2>
          <span className="spacer" />
          <div className="btn-neutral" onClick={() => void importRules()} title="Add rules from a JSON file">
            <Icon name="file-code" />
            Import
          </div>
          {rules.length > 0 && (
            <div className="btn-neutral" onClick={() => void exportRules(rules, "novaproxy-rules")} title="Save every rule to a JSON file">
              <Icon name="download" />
              Export
            </div>
          )}
          <div className="btn-primary" onClick={() => addRule()}>
            <Icon name="plus" />
            New rule
          </div>
        </div>
        <p className="page-sub">
          Map Remote, Map Local, Block and header Rewrite are applied to matching live traffic; Fault rules
          inject timeouts and network errors to test how an app copes. Patterns match{" "}
          <span className="mono">scheme://host/path</span> with <span className="mono">*</span> wildcards. Rules
          sharing a group form a scenario you can switch, count and export together.
        </p>

        {(hasFaults || allowHosts.length > 0) && (
          <div className={`fault-hosts ${allowHosts.length === 0 ? "empty" : ""}`}>
            <div className="fh-head">
              <Icon name="shield-check" size={15} />
              <span className="fh-title">Fault hosts</span>
              {faultsOn > 0 && (
                <span className="fh-on">
                  {faultsOn} Fault rule{faultsOn === 1 ? "" : "s"} on
                </span>
              )}
            </div>
            <p className="fh-sub">
              Fault rules act only on these hosts (globs such as <span className="mono">*.dev.example.com</span>),
              however their pattern reads. Empty means Fault rules act nowhere: keep it to test environments.
            </p>
            <input
              className="rule-input"
              value={hostsDraft}
              placeholder="*.dev.example.com, api.staging.example.com"
              onChange={(e) => setHostsDraft(e.target.value)}
              onBlur={saveHosts}
              onKeyDown={(e) => e.key === "Enter" && saveHosts()}
            />
          </div>
        )}

        {rules.length >= FILTER_FROM && (
          <input
            className="rule-input rules-filter"
            value={filter}
            placeholder="Filter by group, name or pattern"
            onChange={(e) => setFilter(e.target.value)}
          />
        )}

        {rules.length === 0 && <p className="page-sub">No rules yet — add one to reshape traffic.</p>}
        {rules.length > 0 && groups.length === 0 && <p className="page-sub">No rule matches the filter.</p>}

        {groups.map((g) => {
          const key = g.name ?? "";
          const ids = g.rules.map((r) => r.id);
          const groupHits = ids.reduce((n, id) => n + (hits[id] ?? 0), 0);
          const isCollapsed = collapsed.has(key);
          const state = groupState(g.rules);
          return (
            <div className="rule-group" key={key}>
              {g.name !== null && (
                <div className="rg-head">
                  <span className="rg-chev" onClick={() => toggleCollapsed(key)} title={isCollapsed ? "Expand" : "Collapse"}>
                    <Icon name={isCollapsed ? "chevron-right" : "chevron-down"} size={15} />
                  </span>
                  <span
                    className={`switch sm ${state === "on" ? "on" : ""} ${state === "mixed" ? "mixed" : ""}`}
                    title={state === "on" ? "Switch the scenario off" : "Switch the whole scenario on"}
                    onClick={() => setGroupEnabled(g.name!, state !== "on")}
                  >
                    <span className="knob" />
                  </span>
                  <span className="rg-name" onClick={() => toggleCollapsed(key)}>
                    {g.name}
                  </span>
                  <span className="rg-count">
                    {g.rules.length} rule{g.rules.length === 1 ? "" : "s"}
                  </span>
                  <span className="spacer" />
                  <span className={`hits ${groupHits ? "hit" : ""}`} title="Requests these rules acted on since the last reset">
                    hits {groupHits}
                  </span>
                  <span className="rg-act" title="Reset this scenario's hits (re-arms first-only rules)" onClick={() => resetHits(ids)}>
                    <Icon name="refresh-cw" size={14} />
                  </span>
                  <span className="rg-act" title="Add a rule to this scenario" onClick={() => addRule(g.name)}>
                    <Icon name="plus" size={14} />
                  </span>
                  <span className="rg-act" title="Export this scenario" onClick={() => void exportRules(g.rules, g.name!)}>
                    <Icon name="download" size={14} />
                  </span>
                </div>
              )}
              {!isCollapsed &&
                g.rules.map((r) => (
                  <RuleCard
                    key={r.id}
                    rule={r}
                    hits={hits[r.id] ?? 0}
                    groups={groupNames}
                    blocker={faultBlocker(r, allowHosts)}
                    update={(patch) => update(r.id, patch)}
                    onDelete={() => del(r.id)}
                    onResetHits={() => resetHits([r.id])}
                  />
                ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function RuleCard({
  rule: r,
  hits,
  groups,
  blocker,
  update,
  onDelete,
  onResetHits,
}: {
  rule: Rule;
  hits: number;
  groups: string[];
  blocker: string | null;
  update: (patch: Partial<Rule>) => void;
  onDelete: () => void;
  onResetHits: () => void;
}) {
  const fault = r.fault;
  const info = fault ? faultModeInfo(fault.mode) : null;
  const setFault = (patch: Partial<Fault>) => fault && update({ fault: { ...fault, ...patch } });
  const used = r.times != null && hits >= r.times;
  const listId = `groups-${r.id}`;

  return (
    <div className={`rule ${r.kind === "Fault" ? "fault" : ""}`}>
      <div className="rule-head">
        <span className={`switch ${r.enabled ? "on" : ""}`} onClick={() => update({ enabled: !r.enabled })}>
          <span className="knob" />
        </span>
        <select
          className="rule-kind"
          value={r.kind}
          onChange={(e) => update(withKind(r, e.target.value as RuleKind))}
        >
          {RULE_KINDS.map((k) => (
            <option key={k} value={k}>
              {RULE_KIND_LABEL[k]}
            </option>
          ))}
        </select>
        <select
          className="rule-method"
          value={r.method ?? ""}
          title="Match only this method"
          onChange={(e) => update({ method: e.target.value || null })}
        >
          <option value="">Any method</option>
          {METHODS.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <input
          className="rule-name-input"
          value={r.name}
          onChange={(e) => update({ name: e.target.value })}
          placeholder="rule name"
        />
        <span
          className={`hits ${hits ? "hit" : ""} ${used ? "used" : ""}`}
          title={used ? "Used up: reset to arm it again" : "Requests this rule acted on since the last reset. Click to reset."}
          onClick={onResetHits}
        >
          {r.times != null ? `hits ${hits}/${r.times}` : `hits ${hits}`}
        </span>
        <span className="rule-del" title="Delete rule" onClick={onDelete}>
          <Icon name="trash-2" size={15} />
        </span>
      </div>
      <div className="rule-body">
        <div className="rule-field">
          <div className="k">Match URL</div>
          <input
            className="rule-input"
            value={r.pattern}
            onChange={(e) => update({ pattern: e.target.value })}
            placeholder="https://host/path/*"
          />
        </div>
        <div className="rule-field">
          <div className="k">Group</div>
          <input
            className="rule-input"
            list={listId}
            value={r.group ?? ""}
            onChange={(e) => update({ group: e.target.value.trim() === "" ? null : e.target.value })}
            placeholder="none"
          />
          <datalist id={listId}>
            {groups.map((g) => (
              <option key={g} value={g} />
            ))}
          </datalist>
        </div>
        {r.kind === "MapRemote" && (
          <div className="rule-field">
            <div className="k">Redirect to</div>
            <input className="rule-input" value={r.target ?? ""} onChange={(e) => update({ target: e.target.value })} placeholder="https://other-host" />
          </div>
        )}
        {r.kind === "MapLocal" && (
          <div className="rule-field">
            <div className="k">Serve file</div>
            <input className="rule-input" value={r.target ?? ""} onChange={(e) => update({ target: e.target.value })} placeholder="/absolute/path/response.json" />
          </div>
        )}
        {r.kind === "Block" && (
          <div className="rule-field">
            <div className="k">Action</div>
            <div className="v">
              Respond <span className="mono">403</span> to matching requests
            </div>
          </div>
        )}
        {r.kind === "Rewrite" && (
          <>
            <div className="rule-field">
              <div className="k">Header name</div>
              <input className="rule-input" value={r.header_name ?? ""} onChange={(e) => update({ header_name: e.target.value })} placeholder="x-debug" />
            </div>
            <div className="rule-field">
              <div className="k">Header value</div>
              <input className="rule-input" value={r.header_value ?? ""} onChange={(e) => update({ header_value: e.target.value })} placeholder="true" />
            </div>
          </>
        )}
        {r.kind === "Fault" && fault && info && (
          <>
            <div className="rule-field">
              <div className="k">Fault</div>
              <select className="rule-input" value={fault.mode} onChange={(e) => setFault({ mode: e.target.value as FaultMode })}>
                {FAULT_MODES.map((m) => (
                  <option key={m.mode} value={m.mode}>
                    {m.label}
                  </option>
                ))}
              </select>
            </div>
            <div className="rule-field">
              <div className="k">Apply</div>
              <div className="apply-row">
                <select
                  className="rule-input"
                  value={r.times == null ? "always" : r.times === 1 ? "first" : "n"}
                  onChange={(e) =>
                    update({ times: e.target.value === "always" ? null : e.target.value === "first" ? 1 : Math.max(2, r.times ?? 2) })
                  }
                >
                  <option value="always">Every match</option>
                  <option value="first">First match only</option>
                  <option value="n">First N matches</option>
                </select>
                {r.times != null && r.times !== 1 && (
                  <input
                    className="rule-input n"
                    type="number"
                    min={2}
                    value={r.times}
                    onChange={(e) => update({ times: Math.max(1, intOrNull(e.target.value) ?? 1) })}
                  />
                )}
              </div>
            </div>
            <p className="fault-hint full">
              {info.hint}
              {info.forwards ? " The request reaches the server." : " The request never reaches the server."}
            </p>
            {info.fields.includes("delay_ms") && (
              <div className="rule-field">
                <div className="k">Hold (ms)</div>
                <input
                  className="rule-input"
                  type="number"
                  min={0}
                  max={FAULT_MAX_DELAY_MS}
                  value={fault.delay_ms ?? ""}
                  placeholder="0"
                  onChange={(e) => setFault({ delay_ms: intOrNull(e.target.value, FAULT_MAX_DELAY_MS) })}
                />
              </div>
            )}
            {info.fields.includes("status") && (
              <div className="rule-field">
                <div className="k">Status</div>
                <input
                  className="rule-input"
                  type="number"
                  min={100}
                  max={999}
                  value={fault.status ?? ""}
                  placeholder={fault.mode === "HoldStatus" ? "504" : "200"}
                  onChange={(e) => setFault({ status: intOrNull(e.target.value, 999) })}
                />
              </div>
            )}
            {info.fields.includes("cut_after_bytes") && (
              <div className="rule-field">
                <div className="k">Cut after (bytes)</div>
                <input
                  className="rule-input"
                  type="number"
                  min={0}
                  value={fault.cut_after_bytes ?? ""}
                  placeholder="0"
                  onChange={(e) => setFault({ cut_after_bytes: intOrNull(e.target.value) })}
                />
              </div>
            )}
            {info.fields.includes("content_type") && (
              <div className="rule-field">
                <div className="k">Content type</div>
                <input
                  className="rule-input"
                  value={fault.content_type ?? ""}
                  placeholder={fault.mode === "HoldStatus" ? "text/html" : "application/json"}
                  onChange={(e) => setFault({ content_type: e.target.value || null })}
                />
              </div>
            )}
            {info.fields.includes("body") && (
              <div className="rule-field full">
                <div className="k">Body</div>
                <textarea
                  className="rule-input"
                  rows={3}
                  value={fault.body ?? ""}
                  placeholder={'{"error":"..."}'}
                  onChange={(e) => setFault({ body: e.target.value || null })}
                />
              </div>
            )}
            {blocker && (
              <p className="fault-warn full">
                <Icon name="triangle-alert" size={13} />
                {blocker}
              </p>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/** The bar shown on every page while Fault rules are on, so they are not forgotten. */
export function FaultBar({ count, onOpen, onAllOff }: { count: number; onOpen: () => void; onAllOff: () => void }) {
  if (count === 0) return null;
  return (
    <div className="restore-bar fault-bar">
      <span className="rb-icon">
        <Icon name="triangle-alert" size={15} />
      </span>
      <span>
        Fault injection is on: {count} rule{count === 1 ? "" : "s"} can fail matching requests.
      </span>
      <span className="spacer" />
      <button className="tool-btn" onClick={onOpen}>
        Open rules
      </button>
      <button className="tool-btn" onClick={onAllOff}>
        Turn all off
      </button>
    </div>
  );
}
