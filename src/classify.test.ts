import { describe, expect, it } from "vitest";
import type { Flow, Header } from "./api";
import {
  headerValue,
  protoOf,
  statusClassOf,
  typeOf,
} from "./classify";

function mkFlow(over: Partial<Flow> = {}): Flow {
  return {
    id: "f0",
    seq: 0,
    method: "GET",
    scheme: "https",
    host: "example.com",
    path: "/",
    url: "https://example.com/",
    client_addr: "127.0.0.1:1",
    pid: null,
    process: null,
    http_version: "HTTP/1.1",
    state: "Completed",
    status: 200,
    request_headers: [],
    response_headers: [],
    request_body: null,
    response_body: null,
    request_size: 0,
    response_size: 0,
    content_type: null,
    started_at: 0,
    duration_ms: null,
    error: null,
    resent: false,
    mapped_from: null,
    is_websocket: false,
    tunneled: false,
    mcp: null,
    internal: false,
    ...over,
  } as Flow;
}

const ct = (value: string): Header[] => [{ name: "Content-Type", value }];

describe("protoOf", () => {
  it("reads the scheme, and lets a WebSocket upgrade outrank it", () => {
    expect(protoOf(mkFlow({ scheme: "https" }))).toBe("https");
    expect(protoOf(mkFlow({ scheme: "http" }))).toBe("http");
    expect(protoOf(mkFlow({ scheme: "https", is_websocket: true }))).toBe("ws");
  });

  it("a tunneled CONNECT is still HTTPS — only its body is missing", () => {
    expect(protoOf(mkFlow({ scheme: "https", tunneled: true }))).toBe("https");
  });
});

describe("headerValue", () => {
  it("matches case-insensitively and returns the first hit", () => {
    const headers: Header[] = [
      { name: "content-type", value: "a" },
      { name: "Content-Type", value: "b" },
    ];
    expect(headerValue(headers, "Content-Type")).toBe("a");
    expect(headerValue(headers, "missing")).toBeNull();
  });
});

describe("typeOf", () => {
  it("claims MCP before JSON, or every tool call would hide inside json", () => {
    const f = mkFlow({
      content_type: "application/json",
      mcp: { method: "tools/call", tool: "read_file", id: "1", transport: "Http" },
    } as Partial<Flow>);
    expect(typeOf(f)).toBe("mcp");
  });

  it("recognises GraphQL from the path even when the reply is plain JSON", () => {
    expect(typeOf(mkFlow({ path: "/graphql", content_type: "application/json" }))).toBe("graphql");
    expect(typeOf(mkFlow({ path: "/frontend/graphql?operationName=Feed" }))).toBe("graphql");
    // …and from either content type.
    expect(typeOf(mkFlow({ content_type: "application/graphql-response+json" }))).toBe("graphql");
  });

  it("does not mistake a path that merely starts with the word", () => {
    expect(typeOf(mkFlow({ path: "/graphqlish/thing" }))).not.toBe("graphql");
  });

  it("takes JSON from either side, parameters and +json suffixes included", () => {
    expect(typeOf(mkFlow({ content_type: "application/json; charset=utf-8" }))).toBe("json");
    expect(typeOf(mkFlow({ content_type: "application/vnd.api+json" }))).toBe("json");
    expect(typeOf(mkFlow({ request_headers: ct("application/json") }))).toBe("json");
  });

  it("a form is defined by what the client sent, not by the reply", () => {
    expect(
      typeOf(mkFlow({ request_headers: ct("application/x-www-form-urlencoded"), content_type: "text/html" })),
    ).toBe("form");
    expect(typeOf(mkFlow({ request_headers: ct("multipart/form-data; boundary=x") }))).toBe("form");
    // The other direction is not a form: a server answering urlencoded is odd,
    // but it is not a form submission.
    expect(typeOf(mkFlow({ content_type: "application/x-www-form-urlencoded" }))).toBe("other");
  });

  it("collapses everything a browser renders into `document`", () => {
    for (const t of ["text/html", "text/css", "application/javascript", "text/plain"]) {
      expect(typeOf(mkFlow({ content_type: t }))).toBe("document");
    }
  });

  it("takes images, video, audio and fonts as media", () => {
    for (const t of ["image/png", "video/mp4", "audio/mpeg", "font/woff2"]) {
      expect(typeOf(mkFlow({ content_type: t }))).toBe("media");
    }
  });

  it("xml, and then the fallback", () => {
    expect(typeOf(mkFlow({ content_type: "application/xml" }))).toBe("xml");
    expect(typeOf(mkFlow({ content_type: "application/octet-stream" }))).toBe("other");
    expect(typeOf(mkFlow({ content_type: null }))).toBe("other");
  });

  it("a tunneled flow knows nothing, and says so instead of guessing", () => {
    expect(typeOf(mkFlow({ tunneled: true, content_type: null }))).toBe("other");
  });
});

describe("statusClassOf", () => {
  it("maps the hundreds", () => {
    expect(statusClassOf(mkFlow({ status: 101 }))).toBe("1xx");
    expect(statusClassOf(mkFlow({ status: 204 }))).toBe("2xx");
    expect(statusClassOf(mkFlow({ status: 304 }))).toBe("3xx");
    expect(statusClassOf(mkFlow({ status: 401 }))).toBe("4xx");
    expect(statusClassOf(mkFlow({ status: 502 }))).toBe("5xx");
  });

  it("a transport failure is its own class, whatever the status says", () => {
    expect(statusClassOf(mkFlow({ status: null, error: "connection refused" }))).toBe("err");
    expect(statusClassOf(mkFlow({ status: 200, error: "closed mid-body" }))).toBe("err");
  });

  it("in flight is null — no class, rather than a guess the table would show as fact", () => {
    expect(statusClassOf(mkFlow({ status: null, error: null }))).toBeNull();
  });

  it("a status outside 1xx-5xx counts as a failure, not as nothing", () => {
    expect(statusClassOf(mkFlow({ status: 999 }))).toBe("err");
  });
});
