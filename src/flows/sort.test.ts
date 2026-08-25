import { describe, expect, it } from "vitest";
import type { Flow } from "../api";
import { cycleSort, sortable, sortFlows, sortGlyph } from "./sort";

let n = 0;
const f = (over: Partial<Flow> = {}): Flow =>
  ({
    id: `f${++n}`, seq: n, method: "GET", scheme: "https", host: "h", path: "/",
    url: "https://h/", client_addr: "1", pid: null, process: null,
    http_version: "HTTP/1.1", state: "Completed", status: 200,
    request_headers: [], response_headers: [], request_body: null, response_body: null,
    request_size: 0, response_size: 0, content_type: null, started_at: 0,
    duration_ms: null, error: null, resent: false, mapped_from: null,
    is_websocket: false, tunneled: false, mcp: null, internal: false,
    edits: { rule: false, script: false, breakpoint: false },
    ...over,
  }) as Flow;

describe("cycleSort", () => {
  it("walks ascending, descending, then back to capture order", () => {
    expect(cycleSort(null, "duration")).toEqual({ by: "duration", dir: "asc" });
    expect(cycleSort({ by: "duration", dir: "asc" }, "duration")).toEqual({ by: "duration", dir: "desc" });
    // Third click puts the table back — a table you cannot reset is one you stop
    // clicking.
    expect(cycleSort({ by: "duration", dir: "desc" }, "duration")).toBeNull();
  });

  it("starts a different column ascending, whatever the last one was doing", () => {
    expect(cycleSort({ by: "duration", dir: "desc" }, "status")).toEqual({ by: "status", dir: "asc" });
  });

  it("ignores columns that have no ordering", () => {
    expect(sortable("ssl")).toBe(false);
    expect(cycleSort(null, "ssl")).toBeNull();
  });
});

describe("sortFlows", () => {
  it("returns the input untouched in capture order", () => {
    const flows = [f(), f()];
    expect(sortFlows(flows, null)).toBe(flows);
  });

  it("never sorts in place", () => {
    const flows = [f({ duration_ms: 9 }), f({ duration_ms: 1 })];
    const before = [...flows];
    sortFlows(flows, { by: "duration", dir: "asc" });
    expect(flows).toEqual(before);
  });

  it("orders by the column, both ways", () => {
    const flows = [f({ duration_ms: 9 }), f({ duration_ms: 1 }), f({ duration_ms: 5 })];
    const asc = sortFlows(flows, { by: "duration", dir: "asc" }).map((x) => x.duration_ms);
    expect(asc).toEqual([1, 5, 9]);
    const desc = sortFlows(flows, { by: "duration", dir: "desc" }).map((x) => x.duration_ms);
    expect(desc).toEqual([9, 5, 1]);
  });

  it("puts flows still in flight last, not first — pending is not `less than 200`", () => {
    const flows = [f({ status: null }), f({ status: 500 }), f({ status: 200 })];
    expect(sortFlows(flows, { by: "status", dir: "asc" }).map((x) => x.status)).toEqual([200, 500, null]);
    expect(sortFlows(flows, { by: "duration", dir: "asc" }).map((x) => x.status)).toBeDefined();
  });

  it("breaks ties on seq, so equal rows do not shimmer between renders", () => {
    const a = f({ duration_ms: 5 });
    const b = f({ duration_ms: 5 });
    const c = f({ duration_ms: 5 });
    const once = sortFlows([c, a, b], { by: "duration", dir: "asc" }).map((x) => x.id);
    const twice = sortFlows([b, c, a], { by: "duration", dir: "asc" }).map((x) => x.id);
    expect(once).toEqual(twice);
  });

  it("sorts text columns by name", () => {
    const flows = [f({ process: "git" }), f({ process: null }), f({ process: "Chrome" })];
    expect(sortFlows(flows, { by: "client", dir: "asc" }).map((x) => x.process)).toEqual([
      null,
      "Chrome",
      "git",
    ]);
  });
});

describe("sortGlyph", () => {
  it("marks only the sorted column", () => {
    expect(sortGlyph({ by: "status", dir: "asc" }, "status")).toBe("▲");
    expect(sortGlyph({ by: "status", dir: "desc" }, "status")).toBe("▼");
    expect(sortGlyph({ by: "status", dir: "asc" }, "url")).toBe("");
    expect(sortGlyph(null, "status")).toBe("");
  });
});
