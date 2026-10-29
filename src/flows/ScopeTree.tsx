import { useMemo, useState } from "react";
import { Icon } from "../icons";
import { formatChord, shortcut } from "../shortcuts";
import { ScopePicker } from "./ScopePicker";
import { ManageSaved } from "./ManageSaved";
import {
  ALL_TRAFFIC,
  describeScope,
  picksOf,
  sameScope,
  type AppNode,
  type SavedScope,
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
  saved,
  setSaved,
  filterRef,
  width,
}: {
  tree: Tree;
  scope: Scope;
  setScope: (s: Scope) => void;
  pinnedCount: number;
  /** Selections kept from the picker, under Favorites › Saved. */
  saved: SavedScope[];
  setSaved: (s: SavedScope[]) => void;
  /** Focus target for the tree filter (⌘⇧F). */
  filterRef?: React.RefObject<HTMLInputElement | null>;
  /** Set by the divider; the CSS token is only the first-paint fallback. */
  width: number;
}) {
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const [needle, setNeedle] = useState("");
  // The picker opens on the current scope, or on one saved filter to edit it.
  const [picking, setPicking] = useState<{ editing?: SavedScope } | null>(null);
  const [managing, setManaging] = useState(false);
  const [allVia, setAllVia] = useState<"apps" | "domains">("apps");
  // How many apps and domains the active pick holds, for the filter button.
  const pickCount = scope.kind === "set" ? picksOf(scope).apps.length + picksOf(scope).hosts.length : 0;

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
    <div className="tree" style={{ width }}>
      <div className="tree-body">
        {/* Favourites and Saved appear only once there is something in them: a
            permanent empty section is a heading that does nothing. */}
        {pinnedCount > 0 && (
          <>
            <div className="tree-eyebrow">Favorites</div>
            <div {...row({ kind: "pinned" })}>
              <Icon name="pin" size={14} />
              <span className="t">Pinned</span>
              <span className="n">{pinnedCount}</span>
            </div>
          </>
        )}

        {saved.length > 0 && (
          <>
            <div className={`tree-eyebrow saved ${pinnedCount > 0 ? "gap" : ""}`}>
              <Icon name="bookmark" size={13} />
              <span>Saved</span>
              <span className="spacer" />
              <button
                type="button"
                className="tree-x"
                aria-label="Manage saved filters"
                title="Manage saved filters"
                onClick={() => setManaging(true)}
              >
                <Icon name="settings" size={12} />
              </button>
              <span className="n">{saved.length}</span>
            </div>
            {saved.map((sv) => (
              <div
                key={sv.id}
                className={`tree-row ${sameScope(scope, sv.scope) ? "on" : ""}`}
                title={describeScope(sv.scope)}
                onClick={() => setScope(sv.scope)}
              >
                <Icon name="filter" size={14} />
                <span className="t">{sv.label}</span>
              </div>
            ))}
          </>
        )}

        {/* No "All traffic" row: a section head is the way back to everything.
            Which head was used is only for which one lights up. */}
        <TreeHead
          icon="app-window"
          label="Apps"
          count={apps.length}
          on={scope.kind === "all" && allVia === "apps"}
          onClick={() => {
            setAllVia("apps");
            setScope(ALL_TRAFFIC);
          }}
        />
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

        <TreeHead
          icon="globe"
          label="Domains"
          count={domains.length}
          on={scope.kind === "all" && allVia === "domains"}
          onClick={() => {
            setAllVia("domains");
            setScope(ALL_TRAFFIC);
          }}
        />
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
        <button
          type="button"
          className={`tree-pick ${pickCount ? "on" : ""}`}
          title={pickCount ? `Filtering by ${describeScope(scope)}` : "Filter by several apps and domains"}
          aria-label="Filter by several apps and domains"
          onClick={() => setPicking({})}
        >
          <Icon name="filter" size={13} />
          {pickCount > 0 && <span className="c">{pickCount}</span>}
        </button>
        <input
          ref={filterRef}
          value={needle}
          onChange={(e) => setNeedle(e.target.value)}
          placeholder="Filter tree"
          aria-label="Filter the scope tree"
        />
        <span className="kbd">{formatChord(shortcut("filter.tree").chord).join("")}</span>
      </div>
      {picking && (
        <ScopePicker
          tree={tree}
          scope={scope}
          editing={picking.editing}
          saved={saved}
          setSaved={setSaved}
          onApply={setScope}
          onClose={() => setPicking(null)}
        />
      )}
      {managing && (
        <ManageSaved
          saved={saved}
          setSaved={setSaved}
          onEdit={(sv) => {
            setManaging(false);
            setPicking({ editing: sv });
          }}
          onClose={() => setManaging(false)}
        />
      )}
    </div>
  );
}

/** `unknown` rather than a blank: the socket was seen, the process was not named. */
function appLabel(a: AppNode): string {
  return a.name === "" ? "unknown" : a.name;
}

/** A section head, which is also the way back to all traffic. */
function TreeHead({
  icon,
  label,
  count,
  on,
  onClick,
}: {
  icon: "app-window" | "globe";
  label: string;
  count: number;
  on: boolean;
  onClick: () => void;
}) {
  return (
    <div
      className={`tree-head ${on ? "on" : ""}`}
      role="button"
      tabIndex={0}
      title="Show all traffic"
      onClick={onClick}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onClick())}
    >
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
