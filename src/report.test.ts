import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import type { Insight } from "./bindings/Insight";
import { hostTable, normalizePath, percentile, reportMarkdown, safeTarget, summarize } from "./report";

function mk(over: Partial<Flow> = {}): Flow {
  return {
    id: "f",
    seq: 0,
    method: "GET",
    scheme: "https",
    host: "api.test",
    path: "/",
    url: "https://api.test/",
    client_addr: "127.0.0.1:1",
    pid: null,
    process: "Chrome",
    http_version: "HTTP/1.1",
    state: "Completed",
    status: 200,
    request_headers: [],
    response_headers: [],
    request_body: null,
    response_body: null,
    request_size: 0,
    response_size: 100,
    content_type: "application/json",
    started_at: 0,
    duration_ms: 100,
    timings: {
      dns_ms: null,
      connect_ms: null,
      tls_ms: null,
      connection_reused: true,
      request_ms: null,
      ttfb_ms: null,
      download_ms: null,
    },
    error: null,
    resent: false,
    mapped_from: null,
    is_websocket: false,
    tunneled: false,
    mcp: null,
    internal: false,
    edits: { rule: false, script: false, breakpoint: false },
    ...over,
  } as Flow;
}

describe("percentile", () => {
  it("uses nearest rank", () => {
    const v = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
    expect(percentile(v, 50)).toBe(50);
    expect(percentile(v, 95)).toBe(100);
    expect(percentile([], 50)).toBeNull();
  });
});

describe("summarize", () => {
  it("counts failures by kind and rates them over finished requests", () => {
    const s = summarize([
      mk({ id: "a", status: 200 }),
      mk({ id: "b", status: 404 }),
      mk({ id: "c", status: 503 }),
      mk({ id: "d", status: null, state: "Error", error: "refused" }),
      mk({ id: "e", status: null, state: "Started", duration_ms: null }),
    ]);
    expect(s.requests).toBe(5);
    expect(s.completed).toBe(4);
    expect(s.failed).toBe(3);
    expect(s.failedBreakdown).toEqual({ client: 1, server: 1, transport: 1 });
    expect(s.status["2xx"]).toBe(1);
    expect(s.status.err).toBe(1);
  });

  it("keeps sockets and streams out of the latency percentiles", () => {
    const s = summarize([
      mk({ id: "a", duration_ms: 100 }),
      mk({ id: "b", duration_ms: 200 }),
      mk({ id: "ws", is_websocket: true, duration_ms: 600_000 }),
      mk({ id: "sse", content_type: "text/event-stream", duration_ms: 600_000 }),
      mk({ id: "tun", tunneled: true, duration_ms: 600_000 }),
    ]);
    expect(s.p99).toBe(200);
    expect(s.slowest?.id).toBe("b");
  });

  it("names the slowest, the largest and the most-called endpoint", () => {
    const s = summarize([
      mk({ id: "a", path: "/users/1", duration_ms: 50, response_size: 10 }),
      mk({ id: "b", path: "/users/2", duration_ms: 900, response_size: 10 }),
      mk({ id: "c", path: "/feed", duration_ms: 10, response_size: 5_000 }),
    ]);
    expect(s.slowest?.id).toBe("b");
    expect(s.largest?.id).toBe("c");
    expect(s.mostCalled).toMatchObject({ endpoint: "GET api.test/users/:id", count: 2 });
  });

  it("does not crown a most-called endpoint that was called once", () => {
    expect(summarize([mk({ path: "/a" }), mk({ path: "/b" })]).mostCalled).toBeNull();
  });

  it("measures the capture window from first start to last finish", () => {
    const s = summarize([mk({ started_at: 1_000, duration_ms: 100 }), mk({ started_at: 5_000, duration_ms: 500 })]);
    expect(s.windowMs).toBe(4_500);
  });
});

describe("normalizePath", () => {
  it("matches the engine's grouping", () => {
    expect(normalizePath("/users/42/posts?x=1")).toBe("/users/:id/posts");
    expect(normalizePath("/o/3fa85f64-5717-4562-b3fc-2c963f66afa6")).toBe("/o/:uuid");
    expect(normalizePath("/blob/9f86d081884c7d65")).toBe("/blob/:hash");
    expect(normalizePath("/v2/search")).toBe("/v2/search");
  });
});

describe("hostTable", () => {
  it("groups hosts under apps, busiest first", () => {
    const groups = hostTable([
      mk({ process: "Slack", host: "slack.com" }),
      mk({ process: "Chrome", host: "a.test" }),
      mk({ process: "Chrome", host: "b.test", status: 500 }),
      mk({ process: "Chrome", host: "b.test" }),
      mk({ process: null, host: "x.test" }),
    ]);
    expect(groups.map((g) => g.app)).toEqual(["Chrome", "", "Slack"]);
    expect(groups[0].hosts.map((h) => [h.host, h.requests, h.failed])).toEqual([
      ["b.test", 2, 1],
      ["a.test", 1, 0],
    ]);
  });
});

describe("reportMarkdown", () => {
  const leaky = mk({
    id: "t",
    path: "/cb?access_token=SECRET123",
    url: "https://api.test/cb?access_token=SECRET123",
    duration_ms: 5_000,
    request_headers: [{ name: "Authorization", value: "Bearer SECRETHEADER" }],
  });
  const finding: Insight = {
    id: "token-in-url:Chrome|api.test",
    rule: "token-in-url",
    severity: "High",
    title: "Credentials in URLs sent to api.test",
    detail: "1 requests carry `access_token` in the query string.",
    advice: "Send credentials in the Authorization header.",
    app: "Chrome",
    host: "api.test",
    flow_ids: ["t"],
    count: 1,
    last_seen: 0,
  };

  const md = reportMarkdown({
    scope: "Chrome",
    generatedAt: new Date("2026-11-04T10:00:00Z"),
    summary: summarize([leaky]),
    insights: [finding],
    mutedCount: 2,
    hosts: hostTable([leaky]),
  });

  it("never carries query strings or header values", () => {
    expect(md).not.toContain("SECRET");
    expect(safeTarget(leaky)).toBe("GET api.test/cb");
  });

  it("has the summary, findings grouped by severity with background, and hosts", () => {
    expect(md).toContain("Scope: Chrome");
    expect(md).toContain("### High (1)");
    expect(md).toContain("#### Credentials in URLs sent to api.test");
    expect(md).toContain("*Why it matters:*");
    expect(md).toContain("_2 muted findings left out._");
    expect(md).toContain("| Chrome | api.test | 1 | 0 |");
  });
});
