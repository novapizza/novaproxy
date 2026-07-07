import { save, open } from "@tauri-apps/plugin-dialog";
import { api, type Flow } from "./api";

// Captured flow values arrive as JS numbers at runtime even though ts-rs types
// some as bigint; guard JSON.stringify against any stray bigint just in case.
const jsonReplacer = (_k: string, v: unknown) => (typeof v === "bigint" ? Number(v) : v);

const num = (n: number | bigint | null | undefined) => (n == null ? 0 : Number(n));

/** Save all captured flows as a `.nova` session file. Returns true if saved. */
export async function exportSession(flows: Flow[]): Promise<boolean> {
  const path = await save({
    title: "Save NovaProxy session",
    defaultPath: "session.nova",
    filters: [{ name: "NovaProxy session", extensions: ["nova"] }],
  });
  if (!path) return false;
  const doc = { format: "novaproxy-session", version: 1, flows };
  await api.writeFile(path, JSON.stringify(doc, jsonReplacer, 2));
  return true;
}

/** Load flows from a `.nova` session file. Returns null if cancelled. */
export async function importSession(): Promise<Flow[] | null> {
  const picked = await open({
    title: "Open NovaProxy session",
    multiple: false,
    filters: [{ name: "NovaProxy session", extensions: ["nova", "json"] }],
  });
  const path = Array.isArray(picked) ? picked[0] : picked;
  if (!path) return null;
  const text = await api.readFile(path);
  const doc = JSON.parse(text);
  const flows: Flow[] = Array.isArray(doc) ? doc : (doc.flows ?? []);
  return flows;
}

/** Export all flows as an HTTP Archive (HAR 1.2) file. Returns true if saved. */
export async function exportHar(flows: Flow[]): Promise<boolean> {
  const path = await save({
    title: "Export as HAR",
    defaultPath: "novaproxy.har",
    filters: [{ name: "HTTP Archive", extensions: ["har"] }],
  });
  if (!path) return false;

  const entries = flows.map((f) => {
    const reqHeaders = f.request_headers.map((h) => ({ name: h.name, value: h.value }));
    const resHeaders = f.response_headers.map((h) => ({ name: h.name, value: h.value }));
    let queryString: { name: string; value: string }[] = [];
    try {
      queryString = [...new URL(f.url).searchParams.entries()].map(([name, value]) => ({ name, value }));
    } catch {
      queryString = [];
    }
    const dur = f.duration_ms ?? 0;
    return {
      startedDateTime: new Date(f.started_at).toISOString(),
      time: dur,
      request: {
        method: f.method,
        url: f.url,
        httpVersion: f.http_version,
        cookies: [],
        headers: reqHeaders,
        queryString,
        headersSize: -1,
        bodySize: num(f.request_size),
        ...(f.request_body?.text
          ? { postData: { mimeType: f.request_body.media_type ?? "", text: f.request_body.text } }
          : {}),
      },
      response: {
        status: f.status ?? 0,
        statusText: "",
        httpVersion: f.http_version,
        cookies: [],
        headers: resHeaders,
        content: {
          size: num(f.response_body?.size ?? f.response_size),
          mimeType: f.content_type ?? "",
          text: f.response_body?.text ?? "",
        },
        redirectURL: "",
        headersSize: -1,
        bodySize: num(f.response_size),
      },
      cache: {},
      timings: { send: 0, wait: dur, receive: 0 },
    };
  });

  const har = {
    log: {
      version: "1.2",
      creator: { name: "NovaProxy", version: "0.1.0" },
      entries,
    },
  };
  await api.writeFile(path, JSON.stringify(har, jsonReplacer, 2));
  return true;
}
