import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import {
  flowStats,
  formatRate,
  sparkPath,
  throughputRate,
  throughputSeries,
} from "./stats";

// Minimal Flow factory — only the fields the stat helpers touch matter here.
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
    mcp: null,
    ...over,
  } as Flow;
}

describe("flowStats", () => {
  it("reports visible against total, not visible against visible", () => {
    const all = [mkFlow({ id: "a" }), mkFlow({ id: "b" }), mkFlow({ id: "c" })];
    const s = flowStats(all, all.slice(0, 1));
    expect(s.visible).toBe(1);
    expect(s.total).toBe(3);
  });

  it("takes the median over the whole capture, ignoring in-flight flows", () => {
    const all = [
      mkFlow({ duration_ms: 500 }),
      mkFlow({ duration_ms: 100 }),
      mkFlow({ duration_ms: 300 }),
      mkFlow({ duration_ms: null }),
    ];
    expect(flowStats(all, all).medianMs).toBe(300);
  });

  it("picks the lower of the two middles on an even count", () => {
    const all = [mkFlow({ duration_ms: 10 }), mkFlow({ duration_ms: 20 })];
    expect(flowStats(all, all).medianMs).toBe(10);
  });

  it("has no median until something completes", () => {
    expect(flowStats([mkFlow({ duration_ms: null })], []).medianMs).toBeNull();
    expect(flowStats([], []).medianMs).toBeNull();
  });

  it("counts 4xx, 5xx and transport errors as failures, but not 3xx", () => {
    const all = [
      mkFlow({ status: 200 }),
      mkFlow({ status: 304 }),
      mkFlow({ status: 404 }),
      mkFlow({ status: 500 }),
      mkFlow({ status: null, error: "connection refused" }),
    ];
    expect(flowStats(all, all).failed).toBe(3);
  });

  it("never double-counts a flow that both errored and carries a status", () => {
    const all = [mkFlow({ status: 502, error: "upstream closed" })];
    expect(flowStats(all, all).failed).toBe(1);
  });

  it("counts MCP flows", () => {
    const all = [
      mkFlow({ mcp: { transport: "Http", method: "tools/call" } as Flow["mcp"] }),
      mkFlow({ mcp: null }),
    ];
    expect(flowStats(all, all).mcp).toBe(1);
  });
});

describe("throughputSeries", () => {
  const now = 1_000_000;

  it("buckets response bytes by start time, oldest first", () => {
    const flows = [
      mkFlow({ started_at: now - 59_000, response_size: 100n }),
      mkFlow({ started_at: now - 1_000, response_size: 400n }),
    ];
    const s = throughputSeries(flows, now, 4, 60_000);
    expect(s).toEqual([100, 0, 0, 400]);
  });

  it("sums flows landing in the same bucket", () => {
    const flows = [
      mkFlow({ started_at: now - 500, response_size: 10n }),
      mkFlow({ started_at: now - 600, response_size: 5n }),
    ];
    expect(throughputSeries(flows, now, 4, 60_000)[3]).toBe(15);
  });

  it("drops flows older than the window and any dated in the future", () => {
    const flows = [
      mkFlow({ started_at: now - 90_000, response_size: 999n }),
      mkFlow({ started_at: now + 5_000, response_size: 999n }),
    ];
    expect(throughputSeries(flows, now, 4, 60_000)).toEqual([0, 0, 0, 0]);
  });

  it("keeps a flow landing exactly on `now` in the last bucket", () => {
    const flows = [mkFlow({ started_at: now, response_size: 7n })];
    expect(throughputSeries(flows, now, 4, 60_000)[3]).toBe(7);
  });

  it("returns a zeroed series for an empty capture and for degenerate settings", () => {
    expect(throughputSeries([], now, 3, 60_000)).toEqual([0, 0, 0]);
    expect(throughputSeries([mkFlow({ started_at: now })], now, 3, 0)).toEqual([0, 0, 0]);
    expect(throughputSeries([], now, 0, 60_000)).toEqual([]);
  });
});

describe("throughputRate", () => {
  it("averages the series across the window in bytes per second", () => {
    expect(throughputRate([1000, 1000], 2000)).toBe(1000);
  });

  it("is zero for an empty series or a zero window", () => {
    expect(throughputRate([], 60_000)).toBe(0);
    expect(throughputRate([1, 2], 0)).toBe(0);
  });
});

describe("sparkPath", () => {
  it("spans the box and scales the peak to the top", () => {
    const p = sparkPath([0, 5, 10], 100, 50);
    expect(p.line).toBe("M0 50 L50 25 L100 0");
    expect(p.last).toEqual({ x: 100, y: 0 });
  });

  it("closes the area back along the baseline", () => {
    expect(sparkPath([0, 10], 10, 4).area).toBe("M0 4 L10 0 L10 4 L0 4 Z");
  });

  it("rests a flat series on the baseline instead of dividing by zero", () => {
    expect(sparkPath([0, 0, 0], 100, 50).line).toBe("M0 50 L50 50 L100 50");
  });

  it("puts a single sample at the right edge, where the leading dot goes", () => {
    const p = sparkPath([3], 100, 50);
    expect(p.last.x).toBe(100);
  });

  it("returns empty paths for an empty series", () => {
    expect(sparkPath([], 100, 50)).toEqual({ line: "", area: "", last: { x: 100, y: 50 } });
  });
});

describe("formatRate", () => {
  it("reports bits per second, stepped by magnitude", () => {
    expect(formatRate(0)).toBe("0 b/s");
    expect(formatRate(100)).toBe("800 b/s");
    expect(formatRate(1_000)).toBe("8.0 kb/s");
    expect(formatRate(1_000_000)).toBe("8.0 Mb/s");
    expect(formatRate(1_000_000_000)).toBe("8.0 Gb/s");
  });
});
