import { useState } from "react";
import { Icon } from "../icons";
import { describeFilter, suggestFilterName, type FlowFilter, type SavedFilter } from "../filter";

/**
 * Name a filter before keeping it.
 *
 * The name used to be derived silently, which produced correct and unmemorable
 * chips ("HTTPS · JSON · 4xx · 5xx"). A name the user chose is the point of
 * saving — "tenant 401s" is findable next week in a way the parts list is not —
 * so this asks, with a short suggestion already filled in and selected, so
 * accepting the default is still one keystroke.
 */
export function SaveFilterDialog({
  filter,
  saved,
  onSave,
  onClose,
}: {
  filter: FlowFilter;
  saved: SavedFilter[];
  onSave: (label: string) => void;
  onClose: () => void;
}) {
  const suggestion = suggestFilterName(filter);
  const [name, setName] = useState(suggestion);

  const label = name.trim() === "" ? suggestion : name.trim();
  // Saving over a name that already exists replaces that filter rather than
  // making a second chip with the same face: typing an existing name is how a
  // person says "update this one".
  const replacing = saved.some((s) => s.label.toLowerCase() === label.toLowerCase());

  const commit = () => {
    onSave(label);
    onClose();
  };

  return (
    <div className="scrim modal-scrim" onClick={onClose}>
      <div className="modal sf-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Save filter</h2>
          <span className="modal-x" onClick={onClose}><Icon name="x" size={16} /></span>
        </div>

        <div className="sf-body">
          <label className="sf-label" htmlFor="sf-name">Name</label>
          <input
            id="sf-name"
            autoFocus
            value={name}
            placeholder={suggestion}
            onChange={(e) => setName(e.target.value)}
            onFocus={(e) => e.currentTarget.select()}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commit();
              }
              // Escape belongs to the dialog, not to the app's global handler.
              if (e.key === "Escape") e.stopPropagation();
            }}
          />
          {/* What is actually being saved, spelled out — the name is free text and
              says nothing about the filter behind it. */}
          <div className="sf-what">
            <span className="k">Filter</span>
            <span className="v">{describeFilter(filter)}</span>
          </div>
          {replacing && (
            <div className="sf-note">
              A filter called “{label}” already exists — saving replaces it.
            </div>
          )}
        </div>

        <div className="sf-foot">
          <button className="tool-btn" onClick={onClose}>Cancel</button>
          <button className="btn-primary sm" onClick={commit}>
            {replacing ? "Replace" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
