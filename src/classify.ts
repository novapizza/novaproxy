import type { Flow, Header } from "./api";

/**
 * What a flow *is*, for the filter bar's three chip groups.
 *
 * Three independent axes, deliberately: "which protocol", "what kind of
 * payload" and "how did it end" are different questions, and the one query this
 * tool exists for — `JSON` **and** `4xx`, "which API is failing" — needs two of
 * them at once. A single mutually-exclusive chip row cannot express it.
 */

/** Transport. A tunneled CONNECT is still `https`; only the body is missing. */
export type Proto = "http" | "https" | "ws";

/**
 * Payload kind. Coarser than a media-type list on purpose: `JS` and `CSS` as
 * separate chips buy nothing in a proxy aimed at APIs, so everything a browser
 * renders collapses into `document`.
 */
export type FlowType =
  | "json"
  | "graphql"
  | "mcp"
  | "form"
  | "xml"
  | "document"
  | "media"
  | "other";

/**
 * How it ended. `err` is a transport failure — no status was ever received —
 * which is why it is a class of its own rather than a sixth hundred.
 */
export type StatusClass = "1xx" | "2xx" | "3xx" | "4xx" | "5xx" | "err";

export const PROTOS: { id: Proto; label: string }[] = [
  { id: "http", label: "HTTP" },
  { id: "https", label: "HTTPS" },
  { id: "ws", label: "WebSocket" },
];

export const FLOW_TYPES: { id: FlowType; label: string }[] = [
  { id: "json", label: "JSON" },
  { id: "graphql", label: "GraphQL" },
  { id: "mcp", label: "MCP" },
  { id: "form", label: "Form" },
  { id: "xml", label: "XML" },
  { id: "document", label: "Document" },
  { id: "media", label: "Media" },
  { id: "other", label: "Other" },
];

export const STATUS_CLASSES: { id: StatusClass; label: string }[] = [
  { id: "1xx", label: "1xx" },
  { id: "2xx", label: "2xx" },
  { id: "3xx", label: "3xx" },
  { id: "4xx", label: "4xx" },
  { id: "5xx", label: "5xx" },
  { id: "err", label: "ERR" },
];

/** First value for `name`, case-insensitively; `null` when the header is absent. */
export function headerValue(headers: Header[], name: string): string | null {
  const want = name.toLowerCase();
  for (const h of headers) if (h.name.toLowerCase() === want) return h.value;
  return null;
}

export function protoOf(f: Flow): Proto {
  if (f.is_websocket) return "ws";
  return f.scheme.toLowerCase() === "https" ? "https" : "http";
}

/** Media type only — parameters (`; charset=utf-8`, `; boundary=…`) dropped. */
function mediaType(raw: string | null): string {
  if (!raw) return "";
  const semi = raw.indexOf(";");
  return (semi === -1 ? raw : raw.slice(0, semi)).trim().toLowerCase();
}

/**
 * Classify a flow by payload.
 *
 * Both sides are consulted, not just `content_type` (which is the *response*):
 * a form POST is defined by what the client sent, and a GraphQL call is often
 * only recognisable from the request. Order matters — the earlier a rule sits,
 * the more specific it is:
 *
 * 1. `mcp`, because the engine already recognised it and it is JSON underneath;
 *    left to rule 3 every MCP call would hide inside `json`.
 * 2. `graphql`, likewise JSON on the wire, so it has to be claimed first.
 * 3. …then the ordinary media-type families.
 *
 * A tunneled CONNECT has no content type on either side and lands in `other`,
 * which is honest: nothing was decrypted, so nothing is known.
 */
export function typeOf(f: Flow): FlowType {
  if (f.mcp) return "mcp";

  const res = mediaType(f.content_type);
  const req = mediaType(headerValue(f.request_headers, "content-type"));
  const both = `${req} ${res}`;

  // The path is evidence too: a GraphQL endpoint answering `application/json`
  // is still GraphQL, and that is how nearly every server replies.
  if (both.includes("graphql") || /\/graphql\b/i.test(f.path)) return "graphql";
  if (both.includes("json")) return "json";
  if (req.includes("x-www-form-urlencoded") || req.includes("multipart/form-data")) return "form";
  if (both.includes("xml")) return "xml";

  const media = res || req;
  if (/^(image|video|audio|font)\//.test(media)) return "media";
  if (media === "application/font-woff" || media === "application/font-woff2") return "media";
  if (
    media.startsWith("text/") ||
    media.includes("html") ||
    media.includes("javascript") ||
    media.includes("ecmascript") ||
    media.includes("css")
  ) {
    return "document";
  }
  return "other";
}

/**
 * `null` means the flow is still in flight — no status, no error. Not an
 * absence to paper over: a pending request belongs in no status class, and
 * calling it `2xx` would be a guess the table then displays as fact.
 */
export function statusClassOf(f: Flow): StatusClass | null {
  if (f.error != null) return "err";
  if (f.status == null) return null;
  const hundred = Math.floor(f.status / 100);
  switch (hundred) {
    case 1: return "1xx";
    case 2: return "2xx";
    case 3: return "3xx";
    case 4: return "4xx";
    case 5: return "5xx";
    // A status outside 1xx–5xx is not a real HTTP status; treat it as a
    // failure rather than silently dropping the flow out of every class.
    default: return "err";
  }
}
