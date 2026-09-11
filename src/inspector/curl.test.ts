import { describe, expect, it } from "vitest";
import type { Flow, Header } from "../api";
import { buildCurl } from "./curl";

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
const body = (text: string) =>
  ({ text, base64: null, size: text.length, media_type: "text/plain", decoded_from: null }) as Flow["request_body"];

describe("buildCurl", () => {
  it("drops the headers a client regenerates for itself", () => {
    // The bug this exists for: `host` survived into the export, the replaying
    // client added its own, and CloudFront 403'd the duplicate.
    const out = buildCurl(
      mkFlow({
        request_headers: [
          h("host", "example.com"),
          h("Connection", "keep-alive"),
          h("proxy-connection", "keep-alive"),
          h("content-length", "9"),
          h("authorization", "Bearer t"),
        ],
      }),
    );
    expect(out).not.toMatch(/-H 'host/i);
    expect(out).not.toMatch(/-H 'Connection/i);
    expect(out).not.toMatch(/-H 'proxy-connection/i);
    expect(out).not.toMatch(/-H 'content-length/i);
    expect(out).toContain("-H 'authorization: Bearer t'");
  });

  it("drops HTTP/2 pseudo-headers", () => {
    const out = buildCurl(mkFlow({ request_headers: [h(":authority", "example.com"), h("x-a", "1")] }));
    expect(out).not.toContain(":authority");
    expect(out).toContain("-H 'x-a: 1'");
  });

  it("asks curl to decode what the kept accept-encoding invites", () => {
    const enc = mkFlow({ request_headers: [h("accept-encoding", "gzip, br, zstd")] });
    expect(buildCurl(enc)).toContain("--compressed");
    expect(buildCurl(enc)).toContain("-H 'accept-encoding: gzip, br, zstd'");
    expect(buildCurl(mkFlow())).not.toContain("--compressed");
  });

  it("drops a port that is the scheme's default, and keeps one that is not", () => {
    const url = (u: string, scheme = "https") => buildCurl(mkFlow({ url: u, scheme }));
    expect(url("https://example.com:443/a?b=1")).toContain("'https://example.com/a?b=1'");
    expect(url("http://example.com:80/a", "http")).toContain("'http://example.com/a'");
    expect(url("https://example.com:8443/a")).toContain("'https://example.com:8443/a'");
    expect(url("https://[::1]:443/a")).toContain("'https://[::1]/a'");
    // A `:443` anywhere but the authority is part of the request, not the port.
    expect(url("https://example.com/a?to=x:443")).toContain("'https://example.com/a?to=x:443'");
  });

  it("leaves the path and query byte-for-byte alone", () => {
    // `new URL()` would re-encode these; the replay has to be the same request.
    const u = "https://example.com:443/a%2Fb/c?g=1,2,3&q=a%20b&t=a+b";
    expect(buildCurl(mkFlow({ url: u }))).toContain("'https://example.com/a%2Fb/c?g=1,2,3&q=a%20b&t=a+b'");
  });

  it("survives a single quote in a URL, a header and a body", () => {
    const out = buildCurl(
      mkFlow({
        method: "POST",
        url: "https://example.com/?q=o'brien",
        request_headers: [h("x-note", "it's here")],
        request_body: body(`{"n":"o'brien"}`),
      }),
    );
    expect(out).toContain(`'https://example.com/?q=o'\\''brien'`);
    expect(out).toContain(`-H 'x-note: it'\\''s here'`);
    expect(out).toContain(`--data-raw '{"n":"o'\\''brien"}'`);
  });

  it("sends a body starting with @ as data, not as a filename", () => {
    expect(buildCurl(mkFlow({ method: "POST", request_body: body("@/etc/passwd") }))).toContain(
      "--data-raw '@/etc/passwd'",
    );
  });

  it("writes one continuation per argument", () => {
    const out = buildCurl(mkFlow({ request_headers: [h("a", "1"), h("b", "2")] }));
    expect(out).toBe("curl -X GET 'https://example.com/' \\\n  -H 'a: 1' \\\n  -H 'b: 2'");
  });
});
