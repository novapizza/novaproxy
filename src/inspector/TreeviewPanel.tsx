import { useEffect, useRef, useState } from "react";
import { api, type Flow } from "../api";
import { formatCellBytes } from "../format";
import { Icon } from "../icons";
import { bodyToText, useBodyBytes } from "./BodyPanel";
import { childPath, nodeValueText } from "./jsonCopy";

/** How long a copy button shows the check after copying. */
const COPIED_MS = 1200;

/** Which button was clicked last, so only that one shows the check. */
type Copied = { path: string; what: "value" | "path" } | null;

interface CopyProps {
  copied: Copied;
  onCopy: (path: string, what: "value" | "path", text: string) => void;
}

/**
 * The response Treeview of a flow, with the body's bytes fetched first.
 *
 * The flow list keeps no body bytes, so the flow's own preview has only
 * metadata. The Body tab already fetched them; this does the same, or every
 * captured flow read "no body to expand" and only imported sessions had a tree.
 */
export function ResponseTreeview({
  flowId, body, showToast,
}: {
  flowId: string;
  body: Flow["response_body"];
  showToast?: (t: string) => void;
}) {
  const { shown, fetched, loading } = useBodyBytes(flowId, "response", body, showToast);
  if (loading && !fetched && shown) {
    return <div className="kv-empty">Loading body ({formatCellBytes(shown.size)})…</div>;
  }
  return <TreeviewPanel text={bodyToText(shown)} />;
}

/**
 * A JSON body as a collapsible tree.
 *
 * The Body panel already shows the same bytes pretty-printed; this exists for
 * the case pretty-printing does not help — a 6,000-line response where the
 * question is "what keys are in here", not "what does line 4,000 say".
 *
 * Non-JSON is not an error state: plenty of bodies are not JSON, and the panel
 * says which one this is rather than looking broken.
 *
 * Every node copies its value (a string without quotes, a branch as JSON) and
 * its path (`data.items[0].id`) from buttons shown on hover — picking one field
 * out of a large response is most of what this panel is opened for.
 */
export function TreeviewPanel({ text }: { text: string | null }) {
  const [copied, setCopied] = useState<Copied>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  if (text == null || text.trim() === "") {
    return <div className="kv-empty">— no body to expand —</div>;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return <div className="kv-empty">— not JSON; read it in the Body panel —</div>;
  }
  const onCopy = (path: string, what: "value" | "path", value: string) => {
    void api.copyText(value).then(() => {
      setCopied({ path, what });
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(null), COPIED_MS);
    }, () => {});
  };

  return (
    <div className="jtree">
      <Node label={null} path="" value={parsed} depth={0} copied={copied} onCopy={onCopy} />
    </div>
  );
}

/**
 * The copy buttons at the end of a row. The root has no path, so it offers
 * only its value. Clicks stop here: on a branch row a click also toggles it.
 */
function CopyButtons({ path, isRoot, value, copied, onCopy }: CopyProps & {
  path: string;
  isRoot: boolean;
  value: unknown;
}) {
  const done = (what: "value" | "path") => copied?.path === path && copied.what === what;
  return (
    <span className="jcopy" onClick={(e) => e.stopPropagation()}>
      <button
        type="button"
        className={done("value") ? "done" : undefined}
        title={done("value") ? "Copied" : "Copy value"}
        aria-label={isRoot ? "Copy the whole body" : `Copy value of ${path}`}
        onClick={() => onCopy(path, "value", nodeValueText(value))}
      >
        <Icon name={done("value") ? "check" : "copy"} size={12} />
      </button>
      {!isRoot && (
        <button
          type="button"
          className={done("path") ? "done path" : "path"}
          title={done("path") ? "Copied" : `Copy path: ${path}`}
          aria-label={`Copy path ${path}`}
          onClick={() => onCopy(path, "path", path)}
        >
          {done("path") ? <Icon name="check" size={12} /> : "path"}
        </button>
      )}
    </span>
  );
}

/** Objects and arrays open two levels deep: enough to see the shape, not the whole document. */
const OPEN_TO_DEPTH = 2;

function Node({ label, path, value, depth, copied, onCopy }: CopyProps & {
  label: string | null;
  /** Where this node sits, as `childPath` writes it; empty for the root. */
  path: string;
  value: unknown;
  depth: number;
}) {
  const [open, setOpen] = useState(depth < OPEN_TO_DEPTH);
  const branch = value !== null && typeof value === "object";

  if (!branch) {
    return (
      <div className="jrow" style={{ paddingLeft: depth * 14 }}>
        {label != null && <span className="jkey">{label}</span>}
        <span className={`jval ${leafClass(value)}`}>{leafText(value)}</span>
        <CopyButtons path={path} isRoot={label == null} value={value} copied={copied} onCopy={onCopy} />
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
        <CopyButtons path={path} isRoot={label == null} value={value} copied={copied} onCopy={onCopy} />
      </div>
      {open && (
        <>
          {entries.map(([k, v]) => (
            <Node
              key={k}
              label={k}
              path={childPath(path, k, Array.isArray(value))}
              value={v}
              depth={depth + 1}
              copied={copied}
              onCopy={onCopy}
            />
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
