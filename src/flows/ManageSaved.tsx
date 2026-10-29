import { useState } from "react";
import { Icon } from "../icons";
import { describeScope, type SavedScope } from "../scope";

/**
 * The saved apps & domains filters, to rename, edit or delete.
 *
 * The sidebar rows are for applying; changing the list happens here so a
 * stray click in the tree can never delete or rename anything. Editing the
 * picks hands off to the picker, which already knows how to show them.
 */
export function ManageSaved({
  saved,
  setSaved,
  onEdit,
  onClose,
}: {
  saved: SavedScope[];
  setSaved: (s: SavedScope[]) => void;
  onEdit: (sv: SavedScope) => void;
  onClose: () => void;
}) {
  const [renaming, setRenaming] = useState<{ id: string; text: string } | null>(null);

  const rename = () => {
    if (!renaming) return;
    const text = renaming.text.trim();
    // A blank or clashing name keeps the old one rather than leaving two rows
    // nobody can tell apart.
    const clash = saved.some((s) => s.id !== renaming.id && s.label.toLowerCase() === text.toLowerCase());
    if (text && !clash) setSaved(saved.map((s) => (s.id === renaming.id ? { ...s, label: text } : s)));
    setRenaming(null);
  };

  const move = (i: number, by: -1 | 1) => {
    const j = i + by;
    if (j < 0 || j >= saved.length) return;
    const next = [...saved];
    [next[i], next[j]] = [next[j], next[i]];
    setSaved(next);
  };

  return (
    <div className="scrim modal-scrim" onClick={onClose}>
      <div
        className="modal ms-modal"
        role="dialog"
        aria-label="Saved filters"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            if (renaming) setRenaming(null);
            else onClose();
          }
        }}
      >
        <div className="modal-head">
          <h2>Saved filters</h2>
          <span className="modal-x" onClick={onClose}><Icon name="x" size={16} /></span>
        </div>

        <div className="ms-list">
          {saved.length === 0 && <div className="ms-empty">No saved filters.</div>}
          {saved.map((sv, i) => (
            <div key={sv.id} className="ms-row">
              <div className="ms-main">
                {renaming?.id === sv.id ? (
                  <input
                    autoFocus
                    aria-label="New name"
                    value={renaming.text}
                    onChange={(e) => setRenaming({ id: sv.id, text: e.target.value })}
                    onFocus={(e) => e.currentTarget.select()}
                    onBlur={rename}
                    onKeyDown={(e) => e.key === "Enter" && rename()}
                  />
                ) : (
                  <span className="ms-name">{sv.label}</span>
                )}
                <span className="ms-what">{describeScope(sv.scope)}</span>
              </div>
              <button type="button" className="ms-btn" title="Move up" aria-label={`Move ${sv.label} up`} disabled={i === 0} onClick={() => move(i, -1)}>
                <Icon name="arrow-up" size={13} />
              </button>
              <button type="button" className="ms-btn" title="Move down" aria-label={`Move ${sv.label} down`} disabled={i === saved.length - 1} onClick={() => move(i, 1)}>
                <Icon name="arrow-down" size={13} />
              </button>
              <button type="button" className="ms-btn" title="Rename" aria-label={`Rename ${sv.label}`} onClick={() => setRenaming({ id: sv.id, text: sv.label })}>
                <Icon name="pencil" size={13} />
              </button>
              <button type="button" className="ms-btn" title="Edit apps & domains" aria-label={`Edit ${sv.label}`} onClick={() => onEdit(sv)}>
                <Icon name="sliders" size={13} />
              </button>
              <button type="button" className="ms-btn danger" title="Delete" aria-label={`Delete ${sv.label}`} onClick={() => setSaved(saved.filter((x) => x.id !== sv.id))}>
                <Icon name="trash-2" size={13} />
              </button>
            </div>
          ))}
        </div>

        <div className="sf-foot">
          <button className="btn-primary sm" onClick={onClose}>Done</button>
        </div>
      </div>
    </div>
  );
}
