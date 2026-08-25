import type { Flow, Header } from "../api";
import { headerValue } from "../classify";

/**
 * The parts of an exchange the engine does not send as its own field, derived
 * here rather than in Rust: query parameters, cookies and the raw wire text are
 * all re-readable from what a `Flow` already carries, and deriving them keeps
 * the capture payload (which is per-flow, times `MAX_FLOWS`) from growing to
 * hold three more copies of the same bytes.
 */

export interface Pair {
  k: string;
  v: string;
}

/**
 * Query parameters, in the order they appear, duplicates kept.
 *
 * `URLSearchParams` rather than a split: it decodes `%20` and `+` the way the
 * server will. Repeated keys are a real thing (`?id=1&id=2`) and collapsing them
 * into a map would hide half the request.
 */
export function parseQuery(url: string): Pair[] {
  const q = url.indexOf("?");
  if (q === -1) return [];
  const hash = url.indexOf("#", q);
  const raw = url.slice(q + 1, hash === -1 ? undefined : hash);
  if (raw === "") return [];
  return [...new URLSearchParams(raw)].map(([k, v]) => ({ k, v }));
}

/**
 * Cookies, from whichever side is asked for.
 *
 * The two sides are different formats, not one: a request sends every cookie in
 * a single `Cookie` header, while a response sends one `Set-Cookie` per cookie
 * with attributes attached. Attributes are kept on the value rather than parsed
 * apart — `Secure` and `SameSite=Lax` are what you are reading the panel for.
 */
export function parseCookies(f: Flow, side: "request" | "response"): Pair[] {
  if (side === "request") {
    const raw = headerValue(f.request_headers, "cookie");
    if (!raw) return [];
    return raw
      .split(";")
      .map((part) => part.trim())
      .filter((part) => part !== "")
      .map((part) => {
        const eq = part.indexOf("=");
        return eq === -1 ? { k: part, v: "" } : { k: part.slice(0, eq), v: part.slice(eq + 1) };
      });
  }
  return f.response_headers
    .filter((h) => h.name.toLowerCase() === "set-cookie")
    .map((h) => {
      const eq = h.value.indexOf("=");
      if (eq === -1) return { k: h.value, v: "" };
      return { k: h.value.slice(0, eq), v: h.value.slice(eq + 1) };
    });
}

/**
 * The exchange as HTTP text — **reconstructed, not captured**.
 *
 * The engine keeps a parsed flow, not the wire bytes, so this is assembled from
 * the request line, the headers it retained and the body preview. It is
 * therefore semantically right and byte-wise approximate: header order and
 * casing survive (`Flow` keeps headers as a list for exactly this reason), but
 * the original framing — chunk boundaries, HTTP/2 pseudo-headers, the exact
 * spacing — does not. The panel says so; a panel labelled `Raw` that quietly
 * invents bytes would be worse than no panel.
 */
export function rawHttp(f: Flow, side: "request" | "response"): string {
  const lines: string[] = [];
  if (side === "request") {
    const path = f.url.slice(f.url.indexOf(f.host) + f.host.length) || f.path;
    lines.push(`${f.method} ${path || "/"} ${f.http_version}`);
    lines.push(...headerLines(f.request_headers));
    return join(lines, f.request_body?.text ?? null);
  }
  const status = f.error
    ? `${f.http_version} — ${f.error}`
    : `${f.http_version} ${f.status ?? ""}`.trim();
  lines.push(status);
  lines.push(...headerLines(f.response_headers));
  return join(lines, f.response_body?.text ?? null);
}

function headerLines(headers: Header[]): string[] {
  return headers.map((h) => `${h.name}: ${h.value}`);
}

function join(lines: string[], body: string | null): string {
  const head = lines.join("\n");
  return body != null && body !== "" ? `${head}\n\n${body}` : head;
}

/** Which of NovaProxy's own mechanisms changed this exchange, spelled out. */
function editedBy(f: Flow): string {
  const parts = [
    f.edits.rule && "a rule",
    f.edits.script && "the script",
    f.edits.breakpoint && "a breakpoint",
  ].filter((x): x is string => typeof x === "string");
  return parts.length === 0 ? "nothing — as sent" : parts.join(", ");
}

/** Facts about one side of the exchange, for its Summary panel. */
export function summaryOf(f: Flow, side: "request" | "response"): Pair[] {
  if (side === "request") {
    return [
      { k: "Method", v: f.method },
      { k: "URL", v: f.url },
      { k: "Protocol", v: f.http_version },
      { k: "Scheme", v: f.scheme.toUpperCase() + (f.tunneled ? " · tunneled" : "") },
      { k: "Client", v: f.client_addr },
      {
        k: "App",
        v: f.process ? `${f.process}${f.pid != null ? ` (${f.pid})` : ""}` : "unknown",
      },
      { k: "Headers", v: String(f.request_headers.length) },
      { k: "Content type", v: headerValue(f.request_headers, "content-type") ?? "—" },
    ];
  }
  return [
    { k: "Status", v: f.error ? `error — ${f.error}` : String(f.status ?? "in flight") },
    { k: "Content type", v: f.content_type ?? "—" },
    { k: "Headers", v: String(f.response_headers.length) },
    { k: "Encoding", v: f.response_body?.decoded_from ?? "identity" },
    { k: "Truncated", v: f.response_body?.truncated ? "yes — capped at the capture cap" : "no" },
    { k: "Mapped from", v: f.mapped_from ?? "—" },
    { k: "Resent", v: f.resent ? "yes" : "no" },
    { k: "Edited by", v: editedBy(f) },
    { k: "MCP", v: f.mcp ? `${f.mcp.method ?? "—"}${f.mcp.tool ? ` → ${f.mcp.tool}` : ""}` : "—" },
  ];
}
