import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import { formatRate, throughputRate, throughputSeries } from "./stats";

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

describe("formatRate", () => {
  it("reports bits per second, stepped by magnitude", () => {
    expect(formatRate(0)).toBe("0 b/s");
    expect(formatRate(100)).toBe("800 b/s");
    expect(formatRate(1_000)).toBe("8.0 kb/s");
    expect(formatRate(1_000_000)).toBe("8.0 Mb/s");
    expect(formatRate(1_000_000_000)).toBe("8.0 Gb/s");
  });
});
