import { describe, expect, it } from "vitest";
import type { BodyPreview, Flow, Header } from "../api";
import { paneText } from "./copy";

function mkFlow(over: Partial<Flow> = {}): Flow {
  return {
    id: "f0", seq: 0, method: "GET", scheme: "https", host: "example.com",
    path: "/", url: "https://example.com/", client_addr: "127.0.0.1:1",
    pid: null, process: null, http_version: "HTTP/1.1", state: "Completed",
    status: 200, request_headers: [], response_headers: [], request_body: null,
    response_body: null, request_size: 0, response_size: 0, content_type: null,
    started_at: 0, duration_ms: null, error: null, resent: false,
    mapped_from: null, is_websocket: false, tunneled: false, mcp: null, internal: false,
    edits: { rule: false, script: false, breakpoint: false },
    ...over,
  } as Flow;
}
const h = (name: string, value: string): Header => ({ name, value });
const body = (text: string | null, media_type = "text/plain"): BodyPreview =>
  ({ size: 10, truncated: false, media_type, decoded_from: null, text, base64: null }) as BodyPreview;

describe("paneText", () => {
  it("copies each side's headers as name: value lines", () => {
    const f = mkFlow({
      request_headers: [h("host", "example.com"), h("accept", "*/*")],
      response_headers: [h("server", "x")],
    });
    expect(paneText(f, "request", "Header", null)).toBe("host: example.com\naccept: */*");
    expect(paneText(f, "response", "Header", null)).toBe("server: x");
  });

  it("copies query parameters as name=value, decoded", () => {
    const f = mkFlow({ url: "https://x.com/a?id=1&q=a%20b" });
    expect(paneText(f, "request", "Query", null)).toBe("id=1\nq=a b");
  });

  it("pretty-prints a JSON body, as the Body tab shows it", () => {
    const f = mkFlow();
    expect(paneText(f, "response", "Body", body('{"a":1}', "application/json"))).toBe('{\n  "a": 1\n}');
    expect(paneText(f, "response", "Treeview", body("plain"))).toBe("plain");
  });

  it("is null when there is nothing to copy as text", () => {
    const f = mkFlow();
    expect(paneText(f, "request", "Header", null)).toBeNull();
    expect(paneText(f, "request", "Query", null)).toBeNull();
    expect(paneText(f, "response", "Body", body(null, "application/ocsp-response"))).toBeNull();
    expect(paneText(f, "response", "Timing", null)).toBeNull();
  });

  it("copies Raw as the reconstructed HTTP text", () => {
    const f = mkFlow({ request_headers: [h("host", "example.com")] });
    expect(paneText(f, "request", "Raw", null)).toContain("GET / HTTP/1.1");
  });
});
