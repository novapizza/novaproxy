import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import type { Timings } from "./bindings/Timings";
import { formatMs, timingBreakdown } from "./timing";

const noTimings: Timings = {
  dns_ms: null,
  connect_ms: null,
  tls_ms: null,
  connection_reused: false,
  request_ms: null,
  ttfb_ms: null,
  download_ms: null,
};

function mkFlow(timings: Partial<Timings>, over: Partial<Flow> = {}): Flow {
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
    duration_ms: 200,
    timings: { ...noTimings, ...timings },
    error: null,
    resent: false,
    mapped_from: null,
    is_websocket: false,
    tunneled: false,
    ...over,
  } as Flow;
}

describe("timingBreakdown", () => {
  it("lays the measured phases out as a waterfall", () => {
    const b = timingBreakdown(
      mkFlow({ dns_ms: 10, connect_ms: 20, tls_ms: 30, ttfb_ms: 120, download_ms: 80 }),
    );
    expect(b.phases.map((p) => [p.key, p.ms, p.startMs])).toEqual([
      ["dns", 10, 0],
      ["connect", 20, 10],
      ["tls", 30, 30],
      // Waiting is TTFB minus the 60ms of setup that sits inside it, so the
      // phases don't double-count the connection.
      ["wait", 60, 60],
      ["download", 80, 120],
    ]);
    expect(b.empty).toBe(false);
  });

  it("does not invent phases for a reused connection", () => {
    const b = timingBreakdown(mkFlow({ connection_reused: true, ttfb_ms: 50, download_ms: 10 }));
    expect(b.phases.map((p) => p.key)).toEqual(["wait", "download"]);
    expect(b.reused).toBe(true);
    // Nothing was subtracted from TTFB, because no setup was measured.
    expect(b.phases[0].ms).toBe(50);
  });

  it("omits DNS when no lookup happened and TLS on plaintext HTTP", () => {
    const b = timingBreakdown(mkFlow({ connect_ms: 5, ttfb_ms: 20, download_ms: 5 }));
    expect(b.phases.map((p) => p.key)).toEqual(["connect", "wait", "download"]);
    expect(b.phases[1].ms).toBe(15);
  });

  it("reports a flow with no measurements as empty rather than drawing zeros", () => {
    const b = timingBreakdown(mkFlow({}));
    expect(b.empty).toBe(true);
    expect(b.phases).toEqual([]);
  });

  it("tolerates flows from older sessions that carry no timings at all", () => {
    const flow = mkFlow({});
    // Imported .nova files predating the timing work have no `timings` key.
    delete (flow as unknown as Record<string, unknown>).timings;
    const b = timingBreakdown(flow);
    expect(b.empty).toBe(true);
    expect(b.reused).toBe(false);
  });

  it("never lets a phase exceed the scale span", () => {
    // Measured phases can slightly exceed a stale duration; the span grows.
    const b = timingBreakdown(mkFlow({ ttfb_ms: 300, download_ms: 100 }, { duration_ms: 200 }));
    expect(b.spanMs).toBe(400);
    expect(b.phases.every((p) => p.startMs + p.ms <= b.spanMs)).toBe(true);
  });

  it("still reports an in-flight flow's measured phases", () => {
    const b = timingBreakdown(mkFlow({ ttfb_ms: 40 }, { state: "Started", duration_ms: null }));
    expect(b.totalMs).toBeNull();
    expect(b.phases.map((p) => p.key)).toEqual(["wait"]);
    expect(b.spanMs).toBe(40);
  });

  it("surfaces the request-body phase separately from the waterfall", () => {
    const b = timingBreakdown(mkFlow({ request_ms: 12, ttfb_ms: 50, download_ms: 5 }));
    expect(b.requestMs).toBe(12);
    // It overlaps setup/waiting, so it must not become a waterfall bar.
    expect(b.phases.map((p) => p.key)).toEqual(["wait", "download"]);
  });
});

describe("formatMs", () => {
  it("keeps precision where it matters", () => {
    expect(formatMs(0.42)).toBe("0.42ms");
    expect(formatMs(12.34)).toBe("12.3ms");
    expect(formatMs(1234.6)).toBe("1235ms");
  });
});
