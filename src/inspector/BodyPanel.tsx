import { useEffect, useState } from "react";
import { api, type BodyPreview, type Flow } from "../api";
import { formatCellBytes } from "../format";
import { bytesMissing } from "./curl";

/**
 * A body preview with its bytes, fetched when the list is not holding them.
 *
 * The fetched copy is tagged with the flow and side it belongs to, so switching
 * flows can never show one flow's body under another's headers while the next
 * fetch is in flight.
 */
function useBodyBytes(
  flowId: string,
  side: "request" | "response",
  body: Flow["request_body"],
  onError?: (m: string) => void,
) {
  const [fetched, setFetched] = useState<{ key: string; body: BodyPreview } | null>(null);
  const [loading, setLoading] = useState(false);
  const key = `${flowId}:${side}`;
  const current = fetched?.key === key ? fetched.body : null;
  const missing = bytesMissing(body);

  useEffect(() => {
    if (!missing) return;
    let alive = true;
    setLoading(true);
    api
      .readBody(flowId, side, body!.media_type, body!.decoded_from)
      .then((p) => alive && setFetched({ key, body: p }))
      .catch((e) => alive && onError?.(String(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
    // `key` covers flowId and side; the rest of `body` is metadata for the fetch.
  }, [key, missing]);

  return {
    shown: current ?? body,
    fetched: current,
    loading,
    put: (p: BodyPreview) => setFetched({ key, body: p }),
    setLoading,
  };
}

export function bodyToText(body: Flow["request_body"]): string | null {
  if (!body || body.text == null) return null;
  const ct = (body.media_type ?? "").toLowerCase();
  if (ct.includes("json")) {
    try {
      return JSON.stringify(JSON.parse(body.text), null, 2);
    } catch {
      return body.text;
    }
  }
  return body.text;
}

export function BodyPanel({
  body, kind, status, flowId, showToast,
}: {
  body: Flow["request_body"];
  kind: "req" | "res";
  status?: number | null;
  flowId: string;
  showToast?: (t: string) => void;
}) {
  // Bodies are not held in the list. The bytes of the one on screen are fetched
  // here — from the on-disk store when the body was too large to preview in
  // full, otherwise from the flow the engine retains.
  const side = kind === "req" ? "request" : "response";
  const { shown, fetched: full, loading, put, setLoading } = useBodyBytes(flowId, side, body, showToast);

  if (!shown) {
    return <pre className={`code ${kind}`}>{status === 204 ? "— no content (204) —" : "— no body —"}</pre>;
  }
  if (loading && !full) {
    return <pre className={`code ${kind}`}>Loading body ({formatCellBytes(shown.size)})…</pre>;
  }

  async function loadFull() {
    if (!body) return;
    setLoading(true);
    try {
      put(await api.readBody(flowId, side, body.media_type, body.decoded_from));
    } catch (e) {
      showToast?.(String(e));
    } finally {
      setLoading(false);
    }
  }

  const loadMore =
    body?.spilled && !full ? (
      <div className="body-more" onClick={() => !loading && loadFull()}>
        {loading ? "Loading…" : `Load full body (${formatCellBytes(body.size)})`}
      </div>
    ) : null;

  const ct = (shown.media_type ?? "").toLowerCase();
  if (shown.base64 && ct.startsWith("image/")) {
    return (
      <div className={`code ${kind}`}>
        <img src={`data:${shown.media_type};base64,${shown.base64}`} alt="body preview" />
        {loadMore}
      </div>
    );
  }
  if (shown.base64) {
    return (
      <>
        <pre className={`code ${kind}`}>Binary body — {formatCellBytes(shown.size)} ({shown.media_type ?? "unknown"}){shown.truncated ? ", truncated" : ""}</pre>
        {loadMore}
      </>
    );
  }
  const text = bodyToText(shown);
  return (
    <>
      <pre className={`code ${kind}`}>
        {text ?? "— empty body —"}
        {shown.truncated
          ? shown.spilled && !full
            ? "\n… preview truncated — the full body is stored on disk"
            : "\n… truncated at the capture cap"
          : ""}
      </pre>
      {loadMore}
    </>
  );
}
