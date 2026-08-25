import { useState } from "react";

/**
 * A JSON body as a collapsible tree.
 *
 * The Body panel already shows the same bytes pretty-printed; this exists for
 * the case pretty-printing does not help — a 6,000-line response where the
 * question is "what keys are in here", not "what does line 4,000 say".
 *
 * Non-JSON is not an error state: plenty of bodies are not JSON, and the panel
 * says which one this is rather than looking broken.
 */
export function TreeviewPanel({ text }: { text: string | null }) {
  if (text == null || text.trim() === "") {
    return <div className="kv-empty">— no body to expand —</div>;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return <div className="kv-empty">— not JSON; read it in the Body panel —</div>;
  }
  return (
    <div className="jtree">
      <Node label={null} value={parsed} depth={0} />
    </div>
  );
}

/** Objects and arrays open two levels deep: enough to see the shape, not the whole document. */
const OPEN_TO_DEPTH = 2;

function Node({ label, value, depth }: { label: string | null; value: unknown; depth: number }) {
  const [open, setOpen] = useState(depth < OPEN_TO_DEPTH);
  const branch = value !== null && typeof value === "object";

  if (!branch) {
    return (
      <div className="jrow" style={{ paddingLeft: depth * 14 }}>
        {label != null && <span className="jkey">{label}</span>}
        <span className={`jval ${leafClass(value)}`}>{leafText(value)}</span>
      </div>
    );
  }

  const entries = Array.isArray(value)
    ? value.map((v, i) => [String(i), v] as const)
    : Object.entries(value as Record<string, unknown>);
  const brackets = Array.isArray(value) ? ["[", "]"] : ["{", "}"];

  return (
    <>
      <div className="jrow branch" style={{ paddingLeft: depth * 14 }} onClick={() => setOpen(!open)}>
        <span className="jtwist">{open ? "▾" : "▸"}</span>
        {label != null && <span className="jkey">{label}</span>}
        <span className="jbracket">
          {brackets[0]}
          {open ? "" : `… ${entries.length}`}
          {open ? "" : brackets[1]}
        </span>
      </div>
      {open && (
        <>
          {entries.map(([k, v]) => (
            <Node key={k} label={k} value={v} depth={depth + 1} />
          ))}
          <div className="jrow" style={{ paddingLeft: depth * 14 }}>
            <span className="jbracket">{brackets[1]}</span>
          </div>
        </>
      )}
    </>
  );
}

function leafClass(v: unknown): string {
  if (v === null) return "null";
  return typeof v;
}

function leafText(v: unknown): string {
  if (v === null) return "null";
  return typeof v === "string" ? `"${v}"` : String(v);
}
