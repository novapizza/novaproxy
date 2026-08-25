import { useMemo, useState } from "react";
import { Icon } from "../icons";
import { formatChord, shortcut } from "../shortcuts";
import {
  sameScope,
  type AppNode,
  type HostNode,
  type PathNode,
  type Scope,
  type ScopeTree as Tree,
} from "../scope";

/**
 * The left tree: which slice of the capture the table is looking at.
 *
 * Rows set a *scope*, which ANDs with the chips and the search box rather than
 * replacing them — picking "Google Chrome" leaves the status chips exactly where
 * they were. Counts are of the whole capture, not of the current filter: a count
 * that moved whenever a chip changed would stop being a landmark.
 */
export function ScopeTree({
  tree,
  scope,
  setScope,
  pinnedCount,
  savedCount,
  filterRef,
}: {
  tree: Tree;
  scope: Scope;
  setScope: (s: Scope) => void;
  pinnedCount: number;
  savedCount: number;
  /** Focus target for the tree filter (⌘⇧F). */
  filterRef?: React.RefObject<HTMLInputElement | null>;
}) {
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const [needle, setNeedle] = useState("");

  const toggle = (key: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (!next.delete(key)) next.add(key);
      return next;
    });

  // The tree filter matches names only, and matches app *and* host names, so
  // typing a domain narrows both sections rather than emptying one of them.
  const q = needle.trim().toLowerCase();
  const apps = useMemo(
    () => (q ? tree.apps.filter((a) => appLabel(a).toLowerCase().includes(q)) : tree.apps),
    [tree.apps, q],
  );
  const domains = useMemo(
    () => (q ? tree.domains.filter((d) => d.host.toLowerCase().includes(q)) : tree.domains),
    [tree.domains, q],
  );

  const row = (s: Scope) => ({
    className: `tree-row ${sameScope(scope, s) ? "on" : ""}`,
    onClick: () => setScope(s),
  });

  return (
    <div className="tree">
      <div className="tree-body">
        {/* Favourites appear only once there is something in them: a permanent
            empty "Pinned" row is a control that does nothing. */}
        {(pinnedCount > 0 || savedCount > 0) && (
          <>
            <div className="tree-eyebrow">Favorites</div>
            {pinnedCount > 0 && (
              <div {...row({ kind: "pinned" })}>
                <Icon name="pin" size={14} />
                <span className="t">Pinned</span>
                <span className="n">{pinnedCount}</span>
              </div>
            )}
          </>
        )}

        <div {...row({ kind: "all" })} style={{ marginTop: 6 }}>
          <Icon name="layers" size={14} />
          <span className="t">All traffic</span>
          <span className="n">{tree.total}</span>
        </div>

        <TreeHead icon="app-window" label="Apps" count={apps.length} />
        {apps.map((a) => (
          <AppRow
            key={a.name}
            app={a}
            scope={scope}
            setScope={setScope}
            open={open.has(`app:${a.name}`)}
            toggle={() => toggle(`app:${a.name}`)}
          />
        ))}
        {apps.length === 0 && <div className="tree-none">no match</div>}

        <TreeHead icon="globe" label="Domains" count={domains.length} />
        {domains.map((d) => (
          <HostRow
            key={d.host}
            host={d}
            scope={scope}
            setScope={setScope}
            openKeys={open}
            toggle={toggle}
          />
        ))}
        {domains.length === 0 && <div className="tree-none">no match</div>}
      </div>

      <div className="tree-foot">
        <Icon name="filter" size={13} />
        <input
          ref={filterRef}
          value={needle}
          onChange={(e) => setNeedle(e.target.value)}
          placeholder="Filter tree"
          aria-label="Filter the scope tree"
        />
        <span className="kbd">{formatChord(shortcut("filter.tree").chord).join("")}</span>
      </div>
    </div>
  );
}

/** `unknown` rather than a blank: the socket was seen, the process was not named. */
function appLabel(a: AppNode): string {
  return a.name === "" ? "unknown" : a.name;
}

function TreeHead({ icon, label, count }: { icon: "app-window" | "globe"; label: string; count: number }) {
  return (
    <div className="tree-head">
      <Icon name={icon} size={13} />
      <span>{label}</span>
      <span className="spacer" />
      <span className="n">{count}</span>
    </div>
  );
}

function AppRow({
  app,
  scope,
  setScope,
  open,
  toggle,
}: {
  app: AppNode;
  scope: Scope;
  setScope: (s: Scope) => void;
  open: boolean;
  toggle: () => void;
}) {
  const self: Scope = { kind: "app", name: app.name };
  return (
    <>
      <div className={`tree-row indent ${sameScope(scope, self) ? "on" : ""}`}>
        <span
          className={`twist ${app.hosts.length > 1 ? "" : "hidden"}`}
          onClick={(e) => {
            e.stopPropagation();
            toggle();
          }}
        >
          <Icon name={open ? "chevron-down" : "chevron-right"} size={12} />
        </span>
        <span className="t" onClick={() => setScope(self)}>
          {appLabel(app)}
        </span>
        <span className="n">{app.count}</span>
      </div>
      {open &&
        app.hosts.map((h) => {
          const hostScope: Scope = { kind: "host", host: h.host };
          return (
            <div
              key={h.host}
              className={`tree-row indent2 mono ${sameScope(scope, hostScope) ? "on" : ""}`}
              onClick={() => setScope(hostScope)}
            >
              <span className="t">{h.host}</span>
              <span className="n">{h.count}</span>
            </div>
          );
        })}
    </>
  );
}

function HostRow({
  host,
  scope,
  setScope,
  openKeys,
  toggle,
}: {
  host: HostNode;
  scope: Scope;
  setScope: (s: Scope) => void;
  openKeys: ReadonlySet<string>;
  toggle: (key: string) => void;
}) {
  const key = `host:${host.host}`;
  const open = openKeys.has(key);
  const self: Scope = { kind: "host", host: host.host };
  return (
    <>
      <div className={`tree-row indent mono ${sameScope(scope, self) ? "on" : ""}`}>
        <span
          className={`twist ${host.children.length ? "" : "hidden"}`}
          onClick={(e) => {
            e.stopPropagation();
            toggle(key);
          }}
        >
          <Icon name={open ? "chevron-down" : "chevron-right"} size={12} />
        </span>
        <span className="t" onClick={() => setScope(self)}>
          {host.host}
        </span>
        {host.tls && <span className="tls-chip">TLS</span>}
        <span className="n">{host.count}</span>
      </div>
      {open &&
        host.children.map((p) => (
          <PathRow key={p.prefix} host={host.host} node={p} scope={scope} setScope={setScope} />
        ))}
    </>
  );
}

/**
 * Path rows are always expanded to their (capped) depth.
 *
 * `MAX_PATH_DEPTH` is 2, so there is nothing worth a second twist control: the
 * whole subtree is three rows at most.
 */
function PathRow({
  host,
  node,
  scope,
  setScope,
}: {
  host: string;
  node: PathNode;
  scope: Scope;
  setScope: (s: Scope) => void;
}) {
  const self: Scope = { kind: "path", host, prefix: node.prefix };
  return (
    <>
      <div
        className={`tree-row indent2 mono ${sameScope(scope, self) ? "on" : ""}`}
        onClick={() => setScope(self)}
      >
        <span className="t">/{node.segment}</span>
        <span className="n">{node.count}</span>
      </div>
      {node.children.map((c) => (
        <div
          key={c.prefix}
          className={`tree-row indent3 mono ${sameScope(scope, { kind: "path", host, prefix: c.prefix }) ? "on" : ""}`}
          onClick={() => setScope({ kind: "path", host, prefix: c.prefix })}
        >
          <span className="t">/{c.segment}</span>
          <span className="n">{c.count}</span>
        </div>
      ))}
    </>
  );
}
