import { api, type Flow } from "../api";

/**
 * cURL is an action, not a view.
 *
 * Proxyman has no cURL tab either — it is a context-menu command — and a panel
 * whose only content is a string you immediately copy is a tab spent on
 * nothing. So this lives here, called from the summary bar, the command palette
 * and the keyboard, and there is no cURL panel at all (issues/0003 §6 decision 9).
 */
export function buildCurl(f: Flow): string {
  let s = `curl -X ${f.method} '${f.url}'`;
  for (const h of f.request_headers) s += ` \\\n  -H '${h.name}: ${h.value}'`;
  if (f.request_body?.text) s += ` \\\n  --data '${f.request_body.text}'`;
  return s;
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
