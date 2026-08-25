import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import {
  activeFilterCount,
  describeFilter,
  filterFromJson,
  filterToJson,
  applyFilter,
  buildPredicate,
  EMPTY_FILTER,
  isFiltering,
  matchQuery,
  mcpLabel,
  toastDuration,
  toggleIn,
  type FlowFilter,
} from "./filter";

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

  it("the MCP type chip keeps just the MCP exchanges", () => {
    const flows = [mcp("a", "tools/call", "read_file"), mkFlow({ id: "b" })];
    const mcpOnly = { ...EMPTY_FILTER, type: new Set(["mcp" as const]) };
    expect(applyFilter(flows, mcpOnly).map((f) => f.id)).toEqual(["a"]);
    expect(applyFilter(flows, EMPTY_FILTER).map((f) => f.id)).toEqual(["a", "b"]);
  });

  it("hides NovaProxy's own traffic unless asked for", () => {
    // With the MCP endpoint on, the agent's own calls would otherwise swamp the
    // list the user is reading.
    const flows = [mcp("own", "tools/call", "list_flows", { internal: true }), mkFlow({ id: "b" })];
    expect(applyFilter(flows, EMPTY_FILTER).map((f) => f.id)).toEqual(["b"]);
    expect(applyFilter(flows, { ...EMPTY_FILTER, includeInternal: true }).map((f) => f.id)).toEqual([
      "own",
      "b",
    ]);
    // Still hidden under the MCP chip, which is the case that matters when
    // debugging someone else's MCP server.
    expect(applyFilter(flows, { ...EMPTY_FILTER, type: new Set(["mcp" as const]) })).toHaveLength(0);
  });

  it("mcp: query matches any MCP flow, or a method/tool substring", () => {
    const flows = [mcp("a", "tools/call", "read_file"), mcp("b", "resources/read", null), mkFlow({ id: "c" })];
    const q = (query: string) => applyFilter(flows, { ...EMPTY_FILTER, query }).map((f) => f.id);
    expect(q("mcp:")).toEqual(["a", "b"]);
    expect(q("mcp:read_file")).toEqual(["a"]);
    expect(q("mcp:resources")).toEqual(["b"]);
    expect(q("mcp:absent")).toEqual([]);
  });

  it("free-text search also reaches the MCP method and tool", () => {
    const flows = [mcp("a", "tools/call", "read_file"), mkFlow({ id: "b" })];
    expect(applyFilter(flows, { ...EMPTY_FILTER, query: "read_file" }).map((f) => f.id)).toEqual(["a"]);
  });

  it("the type chip composes with an app scope", () => {
    const flows = [
      mcp("a", "tools/call", "read_file", { process: "node" }),
      mcp("b", "tools/call", "read_file", { process: "Claude" }),
    ];
    const scoped = {
      ...EMPTY_FILTER,
      type: new Set(["mcp" as const]),
      scope: { kind: "app" as const, name: "node" },
    };
    expect(applyFilter(flows, scoped).map((f) => f.id)).toEqual(["a"]);
  });
});

/* --------------------------- the composed filter --------------------------- */

const f2 = (over: Partial<Flow> = {}) =>
  mkFlow({ is_websocket: false, tunneled: false, mcp: null, internal: false,
    edits: { rule: false, script: false, breakpoint: false }, ...over } as Partial<Flow>);

const filter = (over: Partial<FlowFilter> = {}): FlowFilter => ({ ...EMPTY_FILTER, ...over });

describe("buildPredicate", () => {
  it("an empty filter takes everything except NovaProxy's own traffic", () => {
    const keep = buildPredicate(EMPTY_FILTER);
    expect(keep(f2({ status: 500 }))).toBe(true);
    expect(keep(f2({ internal: true }))).toBe(false);
    expect(buildPredicate(filter({ includeInternal: true }))(f2({ internal: true }))).toBe(true);
  });

  it("an empty group means all of it — there is no `All` chip to press", () => {
    expect(EMPTY_FILTER.status.size).toBe(0);
    const keep = buildPredicate(EMPTY_FILTER);
    expect(keep(f2({ status: 200 }))).toBe(true);
    expect(keep(f2({ status: 404 }))).toBe(true);
  });

  it("chips inside one group are OR", () => {
    const keep = buildPredicate(filter({ status: new Set(["4xx", "5xx"]) }));
    expect(keep(f2({ status: 404 }))).toBe(true);
    expect(keep(f2({ status: 502 }))).toBe(true);
    expect(keep(f2({ status: 200 }))).toBe(false);
  });

  it("groups are AND — which is the whole point of splitting them", () => {
    // "which API is failing": JSON *and* 4xx/5xx.
    const keep = buildPredicate(
      filter({ type: new Set(["json"]), status: new Set(["4xx", "5xx"]) }),
    );
    expect(keep(f2({ content_type: "application/json", status: 401 }))).toBe(true);
    expect(keep(f2({ content_type: "application/json", status: 200 }))).toBe(false);
    expect(keep(f2({ content_type: "image/png", status: 401 }))).toBe(false);
  });

  it("a status filter hides flows still in flight rather than guessing a class", () => {
    const pending = f2({ status: null, error: null });
    expect(buildPredicate(filter({ status: new Set(["2xx"]) }))(pending)).toBe(false);
    expect(buildPredicate(EMPTY_FILTER)(pending)).toBe(true);
  });

  it("the scope ANDs with the chips instead of replacing them", () => {
    const keep = buildPredicate(
      filter({ scope: { kind: "app", name: "git" }, status: new Set(["4xx"]) }),
    );
    expect(keep(f2({ process: "git", status: 401 }))).toBe(true);
    expect(keep(f2({ process: "git", status: 200 }))).toBe(false);
    expect(keep(f2({ process: "Chrome", status: 401 }))).toBe(false);
  });

  it("the pinned scope reads the set it is handed", () => {
    const keep = buildPredicate(filter({ scope: { kind: "pinned" } }), { pinned: new Set(["keep"]) });
    expect(keep(f2({ id: "keep" }))).toBe(true);
    expect(keep(f2({ id: "drop" }))).toBe(false);
  });

  it("the query still applies, prefixes and all", () => {
    const keep = buildPredicate(filter({ query: "method:post" }));
    expect(keep(f2({ method: "POST" }))).toBe(true);
    expect(keep(f2({ method: "GET" }))).toBe(false);
  });

  it("protocol takes the WebSocket upgrade off the https pile", () => {
    const ws = buildPredicate(filter({ proto: new Set(["ws"]) }));
    expect(ws(f2({ is_websocket: true }))).toBe(true);
    expect(ws(f2({ is_websocket: false }))).toBe(false);
    expect(buildPredicate(filter({ proto: new Set(["https"]) }))(f2({ is_websocket: true }))).toBe(false);
  });
});

describe("applyFilter", () => {
  it("keeps the input order — the table decides sorting, not the filter", () => {
    const flows = [f2({ id: "a", seq: 3n }), f2({ id: "b", seq: 2n }), f2({ id: "c", seq: 1n })];
    expect(applyFilter(flows, EMPTY_FILTER).map((f) => f.id)).toEqual(["a", "b", "c"]);
  });
});

describe("activeFilterCount", () => {
  it("counts groups, not chips: three status chips are one decision", () => {
    expect(activeFilterCount(EMPTY_FILTER)).toBe(0);
    expect(activeFilterCount(filter({ status: new Set(["2xx", "4xx", "5xx"]) }))).toBe(1);
    expect(activeFilterCount(filter({ status: new Set(["2xx"]), type: new Set(["json"]) }))).toBe(2);
  });

  it("a scope and a query each count; showing internal traffic does not", () => {
    expect(activeFilterCount(filter({ scope: { kind: "host", host: "x" } }))).toBe(1);
    expect(activeFilterCount(filter({ query: "  " }))).toBe(0);
    expect(activeFilterCount(filter({ query: "x" }))).toBe(1);
    expect(activeFilterCount(filter({ includeInternal: true }))).toBe(0);
    expect(isFiltering(EMPTY_FILTER)).toBe(false);
  });
});

describe("toggleIn", () => {
  it("adds what is missing, removes what is there, and never mutates the input", () => {
    const a: ReadonlySet<string> = new Set(["x"]);
    expect([...toggleIn(a, "y")].sort()).toEqual(["x", "y"]);
    expect([...toggleIn(a, "x")]).toEqual([]);
    expect([...a]).toEqual(["x"]);
  });
});


describe("saved filters", () => {
  it("round-trips through storage", () => {
    const original = filter({
      proto: new Set(["https"]),
      type: new Set(["json", "graphql"]),
      status: new Set(["4xx"]),
      scope: { kind: "host", host: "api.example.com" },
      query: "shoots",
    });
    const back = filterFromJson(JSON.parse(JSON.stringify(filterToJson(original))));
    expect(back.proto).toEqual(original.proto);
    expect(back.type).toEqual(original.type);
    expect(back.status).toEqual(original.status);
    expect(back.scope).toEqual(original.scope);
    expect(back.query).toBe("shoots");
  });

  it("drops chip ids it no longer recognises instead of matching nothing", () => {
    const back = filterFromJson({ type: ["json", "gopher"], status: ["9xx"] });
    expect([...back.type]).toEqual(["json"]);
    expect([...back.status]).toEqual([]);
  });

  it("survives junk, an empty blob and a missing scope", () => {
    expect(filterFromJson(null)).toEqual(EMPTY_FILTER);
    expect(filterFromJson({ scope: "everything" }).scope).toEqual({ kind: "all" });
    expect(filterFromJson({ proto: "https" }).proto.size).toBe(0);
  });

  it("comes back enabled — a saved filter is one you are about to use", () => {
    expect(filterFromJson(filterToJson(filter({ enabled: false }))).enabled).toBe(true);
  });
});

describe("describeFilter", () => {
  it("names a filter by its parts, groups in a fixed order", () => {
    expect(
      describeFilter(
        filter({ status: new Set(["4xx", "5xx"]), type: new Set(["json"]), proto: new Set(["https"]) }),
      ),
    ).toBe("HTTPS · JSON · 4xx · 5xx");
  });

  it("includes the scope and the query", () => {
    expect(describeFilter(filter({ scope: { kind: "host", host: "api.x" } }))).toBe("api.x");
    expect(describeFilter(filter({ query: "shoots" }))).toBe("“shoots”");
    expect(describeFilter(filter({ scope: { kind: "app", name: "" } }))).toBe("unknown app");
  });

  it("says `everything` rather than nothing at all", () => {
    expect(describeFilter(EMPTY_FILTER)).toBe("everything");
  });
});
