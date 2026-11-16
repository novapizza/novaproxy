import { Icon } from "./icons";
import { CHANGELOG, compareVersions, type Release } from "./changelog";

const GROUPS: { key: "added" | "improved" | "fixed"; label: string }[] = [
  { key: "added", label: "New" },
  { key: "improved", label: "Improved" },
  { key: "fixed", label: "Fixed" },
];

function formatDate(iso: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString("en-US", { year: "numeric", month: "long", day: "numeric" });
}

/**
 * Help › What's New, and the same dialog opened by itself after an update.
 *
 * Every release, newest first, from `src/changelog.ts`. `since` is the version
 * the user ran before this update: releases newer than it are marked, so an
 * update that skipped a few versions shows everything it brought.
 */
export function WhatsNewDialog({
  onClose,
  since = null,
  current,
  releases = CHANGELOG,
}: {
  onClose: () => void;
  since?: string | null;
  current: string;
  releases?: Release[];
}) {
  const isNew = (r: Release) =>
    since != null ? compareVersions(r.version, since) > 0 && compareVersions(r.version, current) <= 0 : false;

  return (
    <div className="scrim modal-scrim" onClick={onClose}>
      <div className="modal wn-modal" role="dialog" aria-label="What's new" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>What’s new</h2>
          <span className="modal-x" onClick={onClose}><Icon name="x" size={16} /></span>
        </div>
        <div className="wn-body">
          {releases.map((r) => (
            <section className={`wn-release ${isNew(r) ? "fresh" : ""}`} key={r.version}>
              <div className="wn-head">
                <span className="wn-date">{formatDate(r.date)}</span>
                {r.version === current && <span className="wn-tag">installed</span>}
                {isNew(r) && r.version !== current && <span className="wn-tag">new</span>}
                <span className="wn-ver">{r.version}</span>
              </div>
              {GROUPS.map(({ key, label }) => {
                const items = r[key];
                if (!items?.length) return null;
                return (
                  <div className="wn-group" key={key}>
                    <div className="wn-eyebrow">{label}</div>
                    <ul>
                      {items.map((t, i) => <li key={i}>{t}</li>)}
                    </ul>
                  </div>
                );
              })}
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
