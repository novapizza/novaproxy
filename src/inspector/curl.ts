import { api, type Flow } from "../api";

/**
 * Headers that describe the connection a request arrived on, not the request.
 *
 * Exporting them is what made a copied cURL fail where the original succeeded:
 * every client regenerates its own `Host` and `Connection`, so passing the
 * captured ones through produces a *second* set. CloudFront answers a duplicated
 * `Host` with its own 403 "Bad request" page, so the replay died in front of the
 * origin and looked like an auth problem. RFC 9110 §7.6.1 calls these
 * connection-specific; a request is what is left once they are gone.
 *
 * `content-length` is here for the same reason from the other direction: curl
 * counts the bytes of `--data-raw` itself, and a stale count is a broken body.
 */
const CONNECTION_HEADERS = new Set([
  "host",
  "connection",
  "proxy-connection",
  "keep-alive",
  "transfer-encoding",
  "upgrade",
  "te",
  "trailer",
  "content-length",
]);

/** Single-quoted for a POSIX shell — the only character that matters is `'`. */
function quote(s: string): string {
  return `'${s.split("'").join(`'\\''`)}'`;
}

/**
 * Drops a port that is already the scheme's default.
 *
 * The captured URL is absolute and carries `:443` because that is what the
 * connection used, but a `:443` pasted into Postman shows up in its URL bar and
 * invites exactly the Host confusion this file is trying to avoid. Rewritten by
 * hand rather than through `new URL()`, which would re-encode the path and query
 * and so change the request being replayed.
 */
function tidyUrl(url: string): string {
  const authority = String.raw`(?:\[[^\]]*\]|[^:/?#]*)`;
  return url
    .replace(new RegExp(String.raw`^(https://${authority}):443(?=[/?#]|$)`, "i"), "$1")
    .replace(new RegExp(String.raw`^(http://${authority}):80(?=[/?#]|$)`, "i"), "$1");
}

/**
 * cURL is an action, not a view.
 *
 * Proxyman has no cURL tab either — it is a context-menu command — and a panel
 * whose only content is a string you immediately copy is a tab spent on
 * nothing. So this lives here, called from the summary bar, the command palette
 * and the keyboard, and there is no cURL panel at all (issues/0003 §6 decision 9).
 */
export function buildCurl(f: Flow): string {
  const headers = f.request_headers.filter(
    (h) => !h.name.startsWith(":") && !CONNECTION_HEADERS.has(h.name.toLowerCase()),
  );
  const parts = [`curl -X ${f.method} ${quote(tidyUrl(f.url))}`];
  // The captured `accept-encoding` is kept, so the origin sees the request it
  // saw the first time — which means asking curl to undo the encoding it gets
  // back, or the reply is a screenful of binary.
  if (headers.some((h) => h.name.toLowerCase() === "accept-encoding")) parts.push("--compressed");
  for (const h of headers) parts.push(`-H ${quote(`${h.name}: ${h.value}`)}`);
  // `--data-raw`, not `--data`: a body that happens to begin with `@` would
  // otherwise send the contents of a file of that name.
  if (f.request_body?.text) parts.push(`--data-raw ${quote(f.request_body.text)}`);
  return parts.join(" \\\n  ");
}

/**
 * Whether a preview describes bytes the list is not holding: the metadata says
 * there is content, but neither the text nor the base64 came with it. The store
 * drops body bytes on ingest (see `withoutBodies`); the engine keeps them.
 */
export function bytesMissing(body: Flow["request_body"]): boolean {
  return !!body && body.text == null && body.base64 == null && Number(body.size) > 0;
}

/** Put a flow's request body back, for the paths that need the bytes themselves. */
export async function withRequestBody(flow: Flow): Promise<Flow> {
  const body = flow.request_body;
  if (!bytesMissing(body)) return flow;
  try {
    return {
      ...flow,
      request_body: await api.readBody(flow.id, "request", body!.media_type, body!.decoded_from),
    };
  } catch {
    return flow; // a cURL without its body still beats no cURL
  }
}
