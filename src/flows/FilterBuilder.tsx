import { Icon } from "../icons";
import {
  FIELDS,
  newClause,
  OP_LABEL,
  opsFor,
  withField,
  type Clause,
  type ClauseField,
  type ClauseOp,
} from "../builder";

/**
 * The structured filter rows.
 *
 * Modelled on Proxyman's (issues/0002 §8.1): field, operator, value, one row per
 * condition, each switchable on its own. The switch is the feature — "show me
 * everything for a moment" without losing the row you spent a minute building.
 *
 * Hidden until asked for. Most sessions never need it, and three empty selects
 * above the table would be permanent furniture for an occasional question.
 */
export function FilterBuilder({
  clauses,
  setClauses,
}: {
  clauses: Clause[];
  setClauses: (c: Clause[]) => void;
}) {
  const patch = (id: string, next: Partial<Clause>) =>
    setClauses(clauses.map((c) => (c.id === id ? { ...c, ...next } : c)));

  return (
    <div className="fbuild">
      {clauses.map((c) => (
        <div className={`fb-row ${c.enabled ? "" : "off"}`} key={c.id}>
          <span
            className={`fb-check ${c.enabled ? "on" : ""}`}
            title={c.enabled ? "Switch this row off" : "Switch this row on"}
            onClick={() => patch(c.id, { enabled: !c.enabled })}
          >
            {c.enabled && <Icon name="check" size={11} />}
          </span>

          <select
            className="fb-sel"
            value={c.field}
            aria-label="Field"
            onChange={(e) => patch(c.id, withField(c, e.target.value as ClauseField))}
          >
            {FIELDS.map((f) => (
              <option key={f.id} value={f.id}>{f.label}</option>
            ))}
          </select>

          <select
            className="fb-sel"
            value={c.op}
            aria-label="Operator"
            onChange={(e) => patch(c.id, { op: e.target.value as ClauseOp })}
          >
            {opsFor(c.field).map((op) => (
              <option key={op} value={op}>{OP_LABEL[op]}</option>
            ))}
          </select>

          <input
            className="fb-val"
            value={c.value}
            onChange={(e) => patch(c.id, { value: e.target.value })}
            placeholder={c.op === "matches" ? "regex" : "value"}
            aria-label="Value"
          />

          <span
            className="fb-x"
            title="Remove this row"
            onClick={() => setClauses(clauses.filter((x) => x.id !== c.id))}
          >
            <Icon name="x" size={12} />
          </span>
        </div>
      ))}

      <div className="fb-add" onClick={() => setClauses([...clauses, newClause()])}>
        <Icon name="plus" size={12} />
        Add condition
        <span className="kbd">⌘N</span>
      </div>
    </div>
  );
}
