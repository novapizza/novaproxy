import { useMemo, useState } from "react";
import { Icon } from "../icons";
import {
  picksOf,
  sameScope,
  scopeOf,
  type SavedScope,
  type Scope,
  type ScopeTree as Tree,
} from "../scope";

/**
 * Pick several apps and domains at once, and optionally keep the pick.
 *
 * A dialog rather than checkboxes in the tree: the tree stays a one-click
 * navigator, and choosing a combination is a deliberate act with its own
 * Apply. Apps OR, domains OR, and the two groups AND (`matchScope`), which the
 * summary line spells out before anything is applied.
 */
export function ScopePicker({
  tree,
  scope,
  saved,
  setSaved,
  onApply,
  onClose,
}: {
  tree: Tree;
  scope: Scope;
  saved: SavedScope[];
  setSaved: (s: SavedScope[]) => void;
  onApply: (s: Scope) => void;
  onClose: () => void;
}) {
  const start = picksOf(scope);
  const [apps, setApps] = useState<ReadonlySet<string>>(new Set(start.apps));
  const [hosts, setHosts] = useState<ReadonlySet<string>>(new Set(start.hosts));
  const [needle, setNeedle] = useState("");
  const [name, setName] = useState("");

  const q = needle.trim().toLowerCase();
  const appRows = useMemo(
    () => tree.apps.filter((a) => !q || (a.name || "unknown").toLowerCase().includes(q)),
    [tree.apps, q],
  );
  const hostRows = useMemo(
    () => tree.domains.filter((d) => !q || d.host.toLowerCase().includes(q)),
    [tree.domains, q],
  );

  // Picks keep the order the tree lists them in, so a saved selection reads the
  // way the sidebar does rather than in click order.
  const picked = scopeOf(
    tree.apps.map((a) => a.name).filter((n) => apps.has(n)),
    tree.domains.map((d) => d.host).filter((h) => hosts.has(h)),
  );
  const label = name.trim();
  const replacing = label !== "" && saved.some((s) => s.label.toLowerCase() === label.toLowerCase());

  const flip = (set: ReadonlySet<string>, v: string) => {
    const next = new Set(set);
    if (!next.delete(v)) next.add(v);
    return next;
  };

  const apply = () => {
    if (label !== "") {
      // Saving under a name that exists replaces it: typing the same name is
      // how a person says "update this one".
      const at = saved.findIndex((s) => s.label.toLowerCase() === label.toLowerCase());
      const entry: SavedScope = { id: at >= 0 ? saved[at].id : `ss${Date.now()}`, label, scope: picked };
      if (picked.kind !== "all") {
        setSaved(at >= 0 ? saved.map((s, i) => (i === at ? entry : s)) : [...saved, entry]);
      }
    }
    onApply(picked);
    onClose();
  };

  const already = saved.find((s) => sameScope(s.scope, picked));

  return (
    <div className="scrim modal-scrim" onClick={onClose}>
      <div
        className="modal sp-modal"
        role="dialog"
        aria-label="Filter apps and domains"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          // Keys belong to the dialog, not to the app's global shortcuts.
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
        }}
      >
        <div className="modal-head">
          <h2>Filter apps &amp; domains</h2>
          <span className="modal-x" onClick={onClose}><Icon name="x" size={16} /></span>
        </div>

        <div className="sp-search">
          <Icon name="search" size={13} />
          <input
            autoFocus
            value={needle}
            onChange={(e) => setNeedle(e.target.value)}
            placeholder="Search apps and domains"
            aria-label="Search apps and domains"
          />
        </div>

        <div className="sp-cols">
          <PickList
            title="Apps"
            icon="app-window"
            rows={appRows.map((a) => ({ id: a.name, label: a.name || "unknown", count: a.count }))}
            picked={apps}
            toggle={(id) => setApps(flip(apps, id))}
            clear={() => setApps(new Set())}
          />
          <PickList
            title="Domains"
            icon="globe"
            mono
            rows={hostRows.map((d) => ({ id: d.host, label: d.host, count: d.count }))}
            picked={hosts}
            toggle={(id) => setHosts(flip(hosts, id))}
            clear={() => setHosts(new Set())}
          />
        </div>

        <div className="sp-summary">
          <span className="k">Shows</span>
          <span className="v">
            {picked.kind === "all"
              ? "all traffic"
              : [
                  apps.size ? `requests from ${describeList(picked, "apps")}` : "requests from any app",
                  hosts.size ? `to ${describeList(picked, "hosts")}` : "to any domain",
                ].join(" ")}
          </span>
        </div>

        <div className="sp-foot">
          <input
            className="sp-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && apply()}
            placeholder={already ? `Saved as “${already.label}”` : "Name it to save (optional)"}
            aria-label="Name to save this selection under"
            disabled={picked.kind === "all"}
          />
          <span className="spacer" />
          <button className="tool-btn" onClick={onClose}>Cancel</button>
          <button className="btn-primary sm" onClick={apply}>
            {label === "" || picked.kind === "all" ? "Apply" : replacing ? "Replace & apply" : "Save & apply"}
          </button>
        </div>
      </div>
    </div>
  );
}

function describeList(scope: Scope, which: "apps" | "hosts"): string {
  const p = picksOf(scope);
  return (which === "apps" ? p.apps.map((a) => a || "unknown app") : p.hosts).join(", ");
}

function PickList({
  title,
  icon,
  rows,
  picked,
  toggle,
  clear,
  mono,
}: {
  title: string;
  icon: "app-window" | "globe";
  rows: { id: string; label: string; count: number }[];
  picked: ReadonlySet<string>;
  toggle: (id: string) => void;
  clear: () => void;
  mono?: boolean;
}) {
  return (
    <div className="sp-col">
      <div className="sp-col-head">
        <Icon name={icon} size={13} />
        <span>{title}</span>
        <span className="spacer" />
        {picked.size > 0 ? (
          <button type="button" className="sp-none" onClick={clear}>
            {picked.size} selected · clear
          </button>
        ) : (
          <span className="sp-any">any</span>
        )}
      </div>
      <div className="sp-list">
        {rows.map((r) => (
          <label key={r.id} className={`sp-row ${picked.has(r.id) ? "on" : ""} ${mono ? "mono" : ""}`}>
            <input type="checkbox" checked={picked.has(r.id)} onChange={() => toggle(r.id)} />
            <span className="t">{r.label}</span>
            <span className="n">{r.count}</span>
          </label>
        ))}
        {rows.length === 0 && <div className="tree-none">no match</div>}
      </div>
    </div>
  );
}
