import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import {
  ALL_TRAFFIC,
  buildScopeTree,
  MAX_PATH_DEPTH,
  matchScope,
  sameScope,
  scopeKey,
  type Scope,
} from "./scope";

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
    edits: { rule: false, script: false, breakpoint: false },
    ...over,
  } as Flow;
}

describe("matchScope", () => {
  it("`all` takes everything", () => {
    expect(matchScope(mkFlow(), ALL_TRAFFIC)).toBe(true);
  });

  it("`app` is an exact process match", () => {
    const f = mkFlow({ process: "Google Chrome" });
    expect(matchScope(f, { kind: "app", name: "Google Chrome" })).toBe(true);
    expect(matchScope(f, { kind: "app", name: "Chrome" })).toBe(false);
  });

  it("an unattributed flow answers to the empty bucket, not to every app", () => {
    const f = mkFlow({ process: null });
    expect(matchScope(f, { kind: "app", name: "" })).toBe(true);
    expect(matchScope(f, { kind: "app", name: "git" })).toBe(false);
  });

  it("`host` is exact — a subdomain is a different host", () => {
    const f = mkFlow({ host: "api.example.com" });
    expect(matchScope(f, { kind: "host", host: "api.example.com" })).toBe(true);
    expect(matchScope(f, { kind: "host", host: "example.com" })).toBe(false);
  });

  it("`path` takes the prefix and everything under it", () => {
    const scope: Scope = { kind: "path", host: "api.example.com", prefix: "/v3/shoots" };
    const at = (path: string) => matchScope(mkFlow({ host: "api.example.com", path }), scope);
    expect(at("/v3/shoots")).toBe(true);
    expect(at("/v3/shoots/4821")).toBe(true);
    expect(at("/v3/shoots?page=2")).toBe(true);
    // The boundary matters: this is a different resource, not a child.
    expect(at("/v3/shootsummary")).toBe(false);
    expect(at("/v3")).toBe(false);
  });

  it("`path` is scoped to its host as well as its prefix", () => {
    const scope: Scope = { kind: "path", host: "api.example.com", prefix: "/v3" };
    expect(matchScope(mkFlow({ host: "other.com", path: "/v3/x" }), scope)).toBe(false);
  });

  it("`pinned` reads the set it is handed, and is empty without one", () => {
    const f = mkFlow({ id: "abc" });
    expect(matchScope(f, { kind: "pinned" })).toBe(false);
    expect(matchScope(f, { kind: "pinned" }, { pinned: new Set(["abc"]) })).toBe(true);
    expect(matchScope(f, { kind: "pinned" }, { pinned: new Set(["other"]) })).toBe(false);
  });
});

describe("scopeKey", () => {
  it("distinguishes every kind, so selections compare by string", () => {
    const keys = [
      scopeKey({ kind: "all" }),
      scopeKey({ kind: "pinned" }),
      scopeKey({ kind: "app", name: "git" }),
      scopeKey({ kind: "host", host: "git" }),
      scopeKey({ kind: "path", host: "git", prefix: "/x" }),
    ];
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("sameScope compares by value, not by identity", () => {
    expect(sameScope({ kind: "host", host: "a" }, { kind: "host", host: "a" })).toBe(true);
    expect(sameScope({ kind: "host", host: "a" }, { kind: "host", host: "b" })).toBe(false);
  });
});

describe("buildScopeTree", () => {
  const flows = [
    mkFlow({ id: "1", process: "Google Chrome", host: "api.example.com", path: "/v3/shoots/1" }),
    mkFlow({ id: "2", process: "Google Chrome", host: "api.example.com", path: "/v3/shoots/2" }),
    mkFlow({ id: "3", process: "Google Chrome", host: "cdn.example.com", path: "/img/a.png" }),
    mkFlow({ id: "4", process: "git", host: "github.com", path: "/x.git/info/refs", scheme: "http" }),
    mkFlow({ id: "5", process: null, host: "github.com", path: "/y" }),
  ];

  it("counts apps, and keeps the unattributed bucket last rather than first", () => {
    const tree = buildScopeTree(flows);
    expect(tree.total).toBe(5);
    // Sorted by name alone, `""` would float above every app the user
    // recognises — "we could not tell" is a fallback, so it reads as one.
    expect(tree.apps.map((a) => [a.name, a.count])).toEqual([
      ["Google Chrome", 3],
      ["git", 1],
      ["", 1],
    ]);
  });

  it("lists the hosts each app talked to", () => {
    const chrome = buildScopeTree(flows).apps[0];
    expect(chrome.hosts).toEqual([
      { host: "api.example.com", count: 2 },
      { host: "cdn.example.com", count: 1 },
    ]);
  });

  it("orders busiest first, ties by name, so the list does not reshuffle as it grows", () => {
    const tree = buildScopeTree(flows);
    expect(tree.domains.map((d) => d.host)).toEqual([
      "api.example.com",
      "github.com",
      "cdn.example.com",
    ]);
  });

  it("marks a host as TLS when any of its flows was HTTPS", () => {
    const tree = buildScopeTree(flows);
    const byHost = Object.fromEntries(tree.domains.map((d) => [d.host, d.tls]));
    expect(byHost["api.example.com"]).toBe(true);
    // github.com has one http flow and one https flow.
    expect(byHost["github.com"]).toBe(true);
    expect(buildScopeTree([mkFlow({ scheme: "http" })]).domains[0].tls).toBe(false);
  });

  it("builds the path tree, query dropped, prefix carried for the scope", () => {
    const api = buildScopeTree(flows).domains[0];
    expect(api.children).toEqual([
      {
        segment: "v3",
        prefix: "/v3",
        count: 2,
        children: [{ segment: "shoots", prefix: "/v3/shoots", count: 2, children: [] }],
      },
    ]);
  });

  it("stops at MAX_PATH_DEPTH rather than mirroring every URL", () => {
    const deep = buildScopeTree([mkFlow({ path: "/a/b/c/d/e" })]).domains[0];
    let depth = 0;
    for (let node = deep.children[0]; node; node = node.children[0]) depth++;
    expect(depth).toBe(MAX_PATH_DEPTH);
  });

  it("an empty capture builds an empty tree rather than throwing", () => {
    expect(buildScopeTree([])).toEqual({ apps: [], domains: [], total: 0 });
  });
});
