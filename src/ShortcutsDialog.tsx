import { useState } from "react";
import { Icon } from "./icons";
import { formatChord, isMac, shortcutGroups } from "./shortcuts";

/**
 * Help → Keyboard Shortcuts.
 *
 * Every row comes from the registry in `src/shortcuts.ts`; nothing in this panel
 * is written out by hand, which is the whole reason the registry exists. The
 * search box matches the label *and* the chord, because half the time the
 * question is "what does ⌘B do".
 */
export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  const [q, setQ] = useState("");
  const mac = isMac();
  const needle = q.trim().toLowerCase();

  const groups = shortcutGroups()
    .map((g) => ({
      group: g.group,
      items: g.items.filter(
        (s) =>
          !needle ||
          s.label.toLowerCase().includes(needle) ||
          formatChord(s.chord, mac).join("").toLowerCase().includes(needle),
      ),
    }))
    .filter((g) => g.items.length > 0);

  return (
    <div className="scrim modal-scrim" onClick={onClose}>
      <div className="modal sc-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Keyboard Shortcuts</h2>
          <span className="modal-x" onClick={onClose}><Icon name="x" size={16} /></span>
        </div>
        <div className="sc-search">
          <span className="mag"><Icon name="search" size={14} /></span>
          <input
            autoFocus
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Search shortcuts…"
            aria-label="Search shortcuts"
          />
        </div>
        <div className="sc-body">
          {groups.length === 0 && <div className="sc-none">Nothing matches “{q}”.</div>}
          {groups.map((g) => (
            <div className="sc-group" key={g.group}>
              <div className="sc-eyebrow">{g.group}</div>
              {g.items.map((s) => (
                <div className="sc-row" key={s.id}>
                  <span className="l">
                    {s.label}
                    {s.when && <span className="when">{s.when}</span>}
                  </span>
                  <span className="keys">
                    {formatChord(s.chord, mac).map((k, i) => (
                      <span className="kbd" key={i}>{k}</span>
                    ))}
                  </span>
                </div>
              ))}
            </div>
          ))}
        </div>
        <div className="sc-foot">
          Shortcuts are fixed in this version. Clipboard and window chords belong to the OS.
        </div>
      </div>
    </div>
  );
}
