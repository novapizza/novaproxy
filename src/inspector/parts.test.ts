import { describe, expect, it } from "vitest";
import type { Flow, Header } from "../api";
import { parseCookies, parseQuery, rawHttp, summaryOf } from "./parts";

function mkFlow(over: Partial<Flow> = {}): Flow {
  return {
    id: "f0", seq: 0, method: "GET", scheme: "https", host: "example.com",
    path: "/", url: "https://example.com/", client_addr: "127.0.0.1:1",
    pid: null, process: null, http_version: "HTTP/1.1", state: "Completed",
    status: 200, request_headers: [], response_headers: [], request_body: null,
    response_body: null, request_size: 0, response_size: 0, content_type: null,
    started_at: 0, duration_ms: null, error: null, resent: false,
    mapped_from: null, is_websocket: false, tunneled: false, mcp: null, internal: false,
    ...over,
  } as Flow;
}
const h = (name: string, value: string): Header => ({ name, value });

describe("parseQuery", () => {
  it("decodes, keeps order and keeps duplicates", () => {
    expect(parseQuery("https://x.com/a?id=1&id=2&q=a%20b&plus=a+b")).toEqual([
      { k: "id", v: "1" },
      { k: "id", v: "2" },
      { k: "q", v: "a b" },
      { k: "plus", v: "a b" },
    ]);
  });

  it("no query, or an empty one, is no rows rather than one blank row", () => {
    expect(parseQuery("https://x.com/a")).toEqual([]);
    expect(parseQuery("https://x.com/a?")).toEqual([]);
  });

  it("stops at the fragment", () => {
    expect(parseQuery("https://x.com/a?x=1#frag=2")).toEqual([{ k: "x", v: "1" }]);
  });
});

describe("parseCookies", () => {
  it("splits the request's single Cookie header", () => {
    const f = mkFlow({ request_headers: [h("Cookie", "a=1; b=2; flag")] });
    expect(parseCookies(f, "request")).toEqual([
      { k: "a", v: "1" },
      { k: "b", v: "2" },
      { k: "flag", v: "" },
    ]);
  });

  it("keeps one row per Set-Cookie, attributes and all", () => {
    const f = mkFlow({
      response_headers: [
        h("Set-Cookie", "sid=abc; Path=/; Secure"),
        h("set-cookie", "theme=dark"),
        h("Content-Type", "text/html"),
      ],
    });
    // Attributes stay on the value: `Secure` is what the panel is read for.
    expect(parseCookies(f, "response")).toEqual([
      { k: "sid", v: "abc; Path=/; Secure" },
      { k: "theme", v: "dark" },
    ]);
  });

  it("no cookies is empty, not a crash", () => {
    expect(parseCookies(mkFlow(), "request")).toEqual([]);
    expect(parseCookies(mkFlow(), "response")).toEqual([]);
  });
});

describe("rawHttp", () => {
  it("reconstructs the request line, headers and body", () => {
    const f = mkFlow({
      method: "POST",
      url: "https://example.com/v3/x?a=1",
      path: "/v3/x",
      request_headers: [h("Host", "example.com"), h("content-type", "application/json")],
      request_body: { size: 2n, truncated: false, media_type: "application/json", decoded_from: null, text: "{}", base64: null, spilled: false } as Flow["request_body"],
    });
    expect(rawHttp(f, "request")).toBe(
      "POST /v3/x?a=1 HTTP/1.1\nHost: example.com\ncontent-type: application/json\n\n{}",
    );
  });

  it("preserves header order and casing, which is why Flow keeps a list", () => {
    const f = mkFlow({ request_headers: [h("b", "2"), h("A", "1")] });
    expect(rawHttp(f, "request").split("\n").slice(1)).toEqual(["b: 2", "A: 1"]);
  });

  it("renders the status line, and says what happened when there is no status", () => {
    expect(rawHttp(mkFlow({ status: 204 }), "response")).toBe("HTTP/1.1 204");
    expect(rawHttp(mkFlow({ status: null, error: "closed" }), "response")).toBe(
      "HTTP/1.1 — closed",
    );
  });

  it("omits the blank line when there is no body", () => {
    expect(rawHttp(mkFlow({ request_headers: [h("A", "1")] }), "request")).toBe(
      "GET / HTTP/1.1\nA: 1",
    );
  });
});

describe("summaryOf", () => {
  it("names the unattributed case instead of leaving it blank", () => {
    const rows = summaryOf(mkFlow({ process: null }), "request");
    expect(rows.find((r) => r.k === "App")?.v).toBe("unknown");
  });

  it("says a response is in flight rather than printing an empty status", () => {
    const rows = summaryOf(mkFlow({ status: null, error: null }), "response");
    expect(rows.find((r) => r.k === "Status")?.v).toBe("in flight");
  });
});
