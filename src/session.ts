import { save, open } from "@tauri-apps/plugin-dialog";
import { api, type Flow } from "./api";

/**
 * Which flows an export writes: their ids in list order, plus the flows the
 * engine never held (an imported session lives only in the list).
 */
export interface ExportSet {
  ids: string[];
  extra: Flow[];
}

/**
 * Save flows as a `.nova` session file. The backend writes it flow by flow,
 * bodies included, so a long capture never sits in the webview as one string.
 * Returns the number written, or null if the dialog was cancelled.
 */
export async function exportSession(set: ExportSet): Promise<number | null> {
  const path = await save({
    title: "Save NovaProxy session",
    defaultPath: "session.nova",
    filters: [{ name: "NovaProxy session", extensions: ["nova"] }],
  });
  if (!path) return null;
  return api.exportFlows(path, "nova", set.ids, set.extra, false);
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

/**
 * Export flows as an HTTP Archive (HAR 1.2), written by the backend like a
 * session (`src-tauri/src/export.rs` builds the entries); `redact` hides
 * credentials. Returns the number written, or null if cancelled.
 */
export async function exportHar(set: ExportSet, redact: boolean): Promise<number | null> {
  const path = await save({
    title: "Export as HAR",
    defaultPath: "novaproxy.har",
    filters: [{ name: "HTTP Archive", extensions: ["har"] }],
  });
  if (!path) return null;
  return api.exportFlows(path, "har", set.ids, set.extra, redact);
}

/** Save the Insights report as Markdown. Returns true if saved. */
export async function exportReport(markdown: string): Promise<boolean> {
  const path = await save({
    title: "Export Insights report",
    defaultPath: "novaproxy-report.md",
    filters: [{ name: "Markdown", extensions: ["md"] }],
  });
  if (!path) return false;
  await api.writeFile(path, markdown);
  return true;
}
