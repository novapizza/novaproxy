import { describe, expect, it } from "vitest";
import type { Flow } from "./api";
import {
  clauseActive,
  describeClause,
  matchClause,
  matchClauses,
  newClause,
  opsFor,
  withField,
  type Clause,
} from "./builder";

const flow = (over: Partial<Flow> = {}): Flow =>
  ({
    id: "f", seq: 1, method: "GET", scheme: "https", host: "api.example.com",
    path: "/v3/shoots", url: "https://api.example.com/v3/shoots",
    client_addr: "1", pid: null, process: "Google Chrome",
    http_version: "HTTP/1.1", state: "Completed", status: 200,
    request_headers: [{ name: "Authorization", value: "Bearer sk-1" }],
    response_headers: [{ name: "content-type", value: "application/json" }],
    request_body: null, response_body: null, request_size: 0, response_size: 0,
    content_type: "application/json", started_at: 0, duration_ms: 100,
    error: null, resent: false, mapped_from: null, is_websocket: false,
    tunneled: false, mcp: null, internal: false,
    edits: { rule: false, script: false, breakpoint: false },
    ...over,
  }) as Flow;

const c = (over: Partial<Clause> = {}): Clause => ({ ...newClause(), ...over });

describe("matchClause", () => {
  it("a blank value never narrows anything", () => {
    expect(matchClause(flow(), c({ field: "host", op: "is", value: "  " }))).toBe(true);
  });

  it("a row switched off keeps its value and stops filtering", () => {
    const off = c({ field: "host", op: "is", value: "other.com", enabled: false });
    expect(matchClause(flow(), off)).toBe(true);
    expect(clauseActive(off)).toBe(false);
  });

  it("text operators are case-insensitive", () => {
    const f = flow();
    expect(matchClause(f, c({ field: "host", op: "contains", value: "EXAMPLE" }))).toBe(true);
    expect(matchClause(f, c({ field: "host", op: "is", value: "API.example.com" }))).toBe(true);
    expect(matchClause(f, c({ field: "path", op: "startsWith", value: "/v3" }))).toBe(true);
    expect(matchClause(f, c({ field: "path", op: "endsWith", value: "shoots" }))).toBe(true);
  });

  it("negations mean what they say", () => {
    const f = flow();
    expect(matchClause(f, c({ field: "host", op: "notContains", value: "google" }))).toBe(true);
    expect(matchClause(f, c({ field: "host", op: "notContains", value: "example" }))).toBe(false);
    expect(matchClause(f, c({ field: "method", op: "isNot", value: "POST" }))).toBe(true);
  });

  it("searches every header on both sides as `name: value`", () => {
    const f = flow();
    expect(matchClause(f, c({ field: "header", op: "contains", value: "bearer sk-" }))).toBe(true);
    expect(matchClause(f, c({ field: "header", op: "contains", value: "content-type" }))).toBe(true);
    expect(matchClause(f, c({ field: "header", op: "contains", value: "x-nope" }))).toBe(false);
  });

  it("a half-typed regex matches nothing instead of throwing", () => {
    const f = flow();
    expect(matchClause(f, c({ field: "path", op: "matches", value: "^/v[0-9]+/" }))).toBe(true);
    expect(matchClause(f, c({ field: "path", op: "matches", value: "[unclosed" }))).toBe(false);
  });

  it("numeric operators compare numbers, and refuse non-numbers", () => {
    expect(matchClause(flow({ status: 503 }), c({ field: "status", op: "gt", value: "500" }))).toBe(true);
    expect(matchClause(flow({ status: 404 }), c({ field: "status", op: "gt", value: "500" }))).toBe(false);
    expect(matchClause(flow({ status: 100 }), c({ field: "status", op: "lt", value: "200" }))).toBe(true);
    // A flow that failed has no status: "over 500" is not true of it.
    expect(
      matchClause(flow({ status: null, error: "refused" }), c({ field: "status", op: "gt", value: "500" })),
    ).toBe(false);
  });

  it("kind and protocol read the classifier, not the raw fields", () => {
    const f = flow();
    expect(matchClause(f, c({ field: "kind", op: "is", value: "json" }))).toBe(true);
    expect(matchClause(f, c({ field: "proto", op: "is", value: "https" }))).toBe(true);
    expect(matchClause(flow({ is_websocket: true }), c({ field: "proto", op: "is", value: "ws" }))).toBe(true);
  });

  it("an unattributed client is empty text, not a crash", () => {
    expect(matchClause(flow({ process: null }), c({ field: "client", op: "contains", value: "chrome" }))).toBe(false);
  });
});

describe("matchClauses", () => {
  it("ANDs the rows", () => {
    const rows = [
      c({ field: "host", op: "contains", value: "example" }),
      c({ field: "method", op: "is", value: "GET" }),
    ];
    expect(matchClauses(flow(), rows)).toBe(true);
    expect(matchClauses(flow({ method: "POST" }), rows)).toBe(false);
  });

  it("no rows means no narrowing", () => {
    expect(matchClauses(flow(), [])).toBe(true);
  });
});

describe("opsFor / withField", () => {
  it("a numeric field has no substring operators", () => {
    expect(opsFor("status")).not.toContain("contains");
    expect(opsFor("url")).toContain("contains");
  });

  it("changing the field repairs an operator the new field cannot take", () => {
    const text = c({ field: "url", op: "contains" });
    const moved = withField(text, "status");
    expect(moved.op).toBe("is");
    // …and keeps one that still applies.
    expect(withField(c({ field: "url", op: "is" }), "status").op).toBe("is");
  });
});

describe("describeClause", () => {
  it("reads as a sentence, for the saved-filter label", () => {
    expect(describeClause(c({ field: "status", op: "gt", value: "500" }))).toBe("Status is over 500");
    expect(describeClause(c({ field: "header", op: "contains", value: "bearer" }))).toBe(
      "Header contains bearer",
    );
  });
});
