import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import { distinctApps, filterFlows, matchChip, matchQuery, mcpLabel, SLOW_MS, toastDuration } from "./filter";

// Minimal Flow factory — only the fields the filter helpers touch matter here.
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
    ...over,
  } as Flow;
}

describe("matchChip", () => {
  it("`all` lets everything through", () => {
    expect(matchChip(mkFlow({ status: 500 }), "all")).toBe(true);
    expect(matchChip(mkFlow({ status: 200 }), "all")).toBe(true);
  });

  it("`errors` takes 4xx, 5xx and transport failures, but not 3xx", () => {
    expect(matchChip(mkFlow({ status: 404 }), "errors")).toBe(true);
    expect(matchChip(mkFlow({ status: 500 }), "errors")).toBe(true);
    expect(matchChip(mkFlow({ status: null, error: "refused" }), "errors")).toBe(true);
    expect(matchChip(mkFlow({ status: 304 }), "errors")).toBe(false);
    expect(matchChip(mkFlow({ status: 200 }), "errors")).toBe(false);
  });

  it("`slow` takes durations at or over the threshold", () => {
    expect(matchChip(mkFlow({ duration_ms: SLOW_MS }), "slow")).toBe(true);
    expect(matchChip(mkFlow({ duration_ms: SLOW_MS - 1 }), "slow")).toBe(false);
  });

  it("in-flight flows are neither slow nor failed — they are simply not done", () => {
    const pending = mkFlow({ status: null, duration_ms: null, error: null });
    expect(matchChip(pending, "slow")).toBe(false);
    expect(matchChip(pending, "errors")).toBe(false);
    expect(matchChip(pending, "all")).toBe(true);
  });

  it("composes with the query rather than replacing it", () => {
    const flows = [
      mkFlow({ id: "a", status: 500, host: "api.example.com" }),
      mkFlow({ id: "b", status: 500, host: "cdn.example.com" }),
      mkFlow({ id: "c", status: 200, host: "api.example.com" }),
    ];
    expect(filterFlows(flows, "host:api", { chip: "errors" }).map((f) => f.id)).toEqual(["a"]);
  });
});

describe("matchQuery", () => {
  it("matches everything on an empty or whitespace query", () => {
    expect(matchQuery(mkFlow(), "")).toBe(true);
    expect(matchQuery(mkFlow(), "   ")).toBe(true);
  });

  it("app: prefix matches the process name case-insensitively, as a substring", () => {
    const f = mkFlow({ process: "Google Chrome" });
    expect(matchQuery(f, "app:chrome")).toBe(true);
    expect(matchQuery(f, "APP: Google")).toBe(true);
    expect(matchQuery(f, "app:safari")).toBe(false);
  });

  it("app: prefix never matches flows without attribution", () => {
    expect(matchQuery(mkFlow({ process: null }), "app:chrome")).toBe(false);
    // ...except the degenerate empty pattern, which matches all.
    expect(matchQuery(mkFlow({ process: null }), "app:")).toBe(true);
  });

  it("free-text search also covers the process name", () => {
    expect(matchQuery(mkFlow({ process: "Slack" }), "slack")).toBe(true);
    expect(matchQuery(mkFlow({ process: null }), "slack")).toBe(false);
  });

  it("keeps existing prefixes working", () => {
    const f = mkFlow({ method: "POST", status: 404, host: "api.example.com" });
    expect(matchQuery(f, "method:post")).toBe(true);
    expect(matchQuery(f, "method:get")).toBe(false);
    expect(matchQuery(f, "status:404")).toBe(true);
    expect(matchQuery(f, "host:api.")).toBe(true);
  });
});

describe("filterFlows", () => {
  const flows = [
    mkFlow({ id: "a", process: "Google Chrome", host: "a.com" }),
    mkFlow({ id: "b", process: "Slack", host: "b.com" }),
    mkFlow({ id: "c", process: null, host: "c.com" }),
  ];

  it("passes everything through with no app filter and no query", () => {
    expect(filterFlows(flows, "").map((f) => f.id)).toEqual(["a", "b", "c"]);
  });

  it("app filter requires an exact process match", () => {
    expect(filterFlows(flows, "", { app: "Slack" }).map((f) => f.id)).toEqual(["b"]);
    // Not a substring match, unlike the app: query prefix.
    expect(filterFlows(flows, "", { app: "Chrome" })).toHaveLength(0);
  });

  it("excludes unattributed flows when an app filter is set", () => {
    expect(filterFlows(flows, "", { app: "Google Chrome" }).map((f) => f.id)).toEqual(["a"]);
  });

  it("combines the app filter with the search query", () => {
    expect(filterFlows(flows, "host:b.com", { app: "Slack" }).map((f) => f.id)).toEqual(["b"]);
    expect(filterFlows(flows, "host:a.com", { app: "Slack" })).toHaveLength(0);
  });
});

describe("distinctApps", () => {
  it("collects unique process names, sorted, skipping unattributed flows", () => {
    const flows = [
      mkFlow({ process: "Slack" }),
      mkFlow({ process: "Google Chrome" }),
      mkFlow({ process: "Slack" }),
      mkFlow({ process: null }),
    ];
    expect(distinctApps(flows)).toEqual(["Google Chrome", "Slack"]);
  });

  it("returns an empty list when nothing is attributed", () => {
    expect(distinctApps([mkFlow(), mkFlow()])).toEqual([]);
  });
});

describe("toastDuration", () => {
  it("uses the explicit duration when given", () => {
    expect(toastDuration("whatever", 1234)).toBe(1234);
  });

  it("clamps short messages to the minimum", () => {
    expect(toastDuration("Saved")).toBe(2600);
  });

  it("scales with message length", () => {
    const short = toastDuration("Request continued");
    const long = toastDuration("Failed to install CA certificate: the keychain rejected the item");
    expect(long).toBeGreaterThan(short);
    expect(long).toBe(2000 + 64 * 55);
  });

  it("caps very long messages at 9 seconds", () => {
    expect(toastDuration("x".repeat(500))).toBe(9000);
  });
});

describe("MCP filtering", () => {
  const mcp = (id: string, method: string, tool: string | null, over: Partial<Flow> = {}) =>
    mkFlow({
      id,
      host: "localhost",
      path: "/mcp",
      mcp: { method, tool, id: "1", transport: "Http" },
      ...over,
    });

  it("labels an MCP flow by method and tool", () => {
    expect(mcpLabel(mcp("a", "tools/call", "read_file"))).toBe("tools/call → read_file");
    expect(mcpLabel(mcp("b", "initialize", null))).toBe("initialize");
    expect(mcpLabel(mkFlow({ id: "c" }))).toBe("");
  });

  it("the MCP chip keeps just the MCP exchanges", () => {
    const flows = [mcp("a", "tools/call", "read_file"), mkFlow({ id: "b" })];
    expect(filterFlows(flows, "", { chip: "mcp" }).map((f) => f.id)).toEqual(["a"]);
    expect(filterFlows(flows, "").map((f) => f.id)).toEqual(["a", "b"]);
  });

  it("hides NovaProxy's own traffic unless asked for", () => {
    // With the MCP endpoint on, the agent's own calls would otherwise swamp the
    // list the user is reading.
    const flows = [mcp("own", "tools/call", "list_flows", { internal: true }), mkFlow({ id: "b" })];
    expect(filterFlows(flows, "").map((f) => f.id)).toEqual(["b"]);
    expect(filterFlows(flows, "", { includeInternal: true }).map((f) => f.id)).toEqual(["own", "b"]);
    // Still hidden under the MCP chip, which is the case that matters when debugging
    // someone else's MCP server.
    expect(filterFlows(flows, "", { chip: "mcp" })).toHaveLength(0);
  });

  it("mcp: query matches any MCP flow, or a method/tool substring", () => {
    const flows = [mcp("a", "tools/call", "read_file"), mcp("b", "resources/read", null), mkFlow({ id: "c" })];
    expect(filterFlows(flows, "mcp:").map((f) => f.id)).toEqual(["a", "b"]);
    expect(filterFlows(flows, "mcp:read_file").map((f) => f.id)).toEqual(["a"]);
    expect(filterFlows(flows, "mcp:resources").map((f) => f.id)).toEqual(["b"]);
    expect(filterFlows(flows, "mcp:absent")).toHaveLength(0);
  });

  it("free-text search also reaches the MCP method and tool", () => {
    const flows = [mcp("a", "tools/call", "read_file"), mkFlow({ id: "b" })];
    expect(filterFlows(flows, "read_file").map((f) => f.id)).toEqual(["a"]);
  });

  it("filters compose with the app dropdown", () => {
    const flows = [
      mcp("a", "tools/call", "read_file", { process: "node" }),
      mcp("b", "tools/call", "read_file", { process: "Claude" }),
    ];
    expect(filterFlows(flows, "", { chip: "mcp", app: "node" }).map((f) => f.id)).toEqual(["a"]);
  });
});
