import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import { distinctApps, filterFlows, matchQuery, toastDuration } from "./filter";

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

describe("filterFlows", () => {
  const flows = [
    mkFlow({ id: "a", process: "Google Chrome", host: "a.com" }),
    mkFlow({ id: "b", process: "Slack", host: "b.com" }),
    mkFlow({ id: "c", process: null, host: "c.com" }),
  ];

  it("passes everything through with no app filter and no query", () => {
    expect(filterFlows(flows, "", "").map((f) => f.id)).toEqual(["a", "b", "c"]);
  });

  it("app filter requires an exact process match", () => {
    expect(filterFlows(flows, "", "Slack").map((f) => f.id)).toEqual(["b"]);
    // Not a substring match, unlike the app: query prefix.
    expect(filterFlows(flows, "", "Chrome")).toHaveLength(0);
  });

  it("excludes unattributed flows when an app filter is set", () => {
    expect(filterFlows(flows, "", "Google Chrome").map((f) => f.id)).toEqual(["a"]);
  });

  it("combines the app filter with the search query", () => {
    expect(filterFlows(flows, "host:b.com", "Slack").map((f) => f.id)).toEqual(["b"]);
    expect(filterFlows(flows, "host:a.com", "Slack")).toHaveLength(0);
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
