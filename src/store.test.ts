import { beforeEach, describe, expect, it } from "vitest";
import { MAX_FLOWS, prependWithinCap, useStore } from "./store";
import type { Flow } from "./api";

// Minimal Flow factory — only the fields the store touches matter here.
function mkFlow(id: string, over: Partial<Flow> = {}): Flow {
  return {
    id,
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
    state: "Started",
    status: null,
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

const reset = () =>
  useStore.setState({ flows: [], recording: true, selectedId: null });

describe("store.upsertFlow", () => {
  beforeEach(reset);

  it("prepends a newly-seen flow (newest first)", () => {
    useStore.getState().upsertFlow(mkFlow("a"));
    useStore.getState().upsertFlow(mkFlow("b"));
    expect(useStore.getState().flows.map((f) => f.id)).toEqual(["b", "a"]);
  });

  it("updates an existing flow in place without changing order", () => {
    const s = useStore.getState();
    s.upsertFlow(mkFlow("a"));
    s.upsertFlow(mkFlow("b"));
    s.upsertFlow(mkFlow("a", { state: "Completed", status: 200 }));

    const flows = useStore.getState().flows;
    expect(flows.map((f) => f.id)).toEqual(["b", "a"]); // order preserved
    expect(flows).toHaveLength(2); // no duplicate
    const a = flows.find((f) => f.id === "a")!;
    expect(a.state).toBe("Completed");
    expect(a.status).toBe(200);
  });

  it("drops brand-new flows while paused (not recording)", () => {
    useStore.setState({ recording: false });
    useStore.getState().upsertFlow(mkFlow("a"));
    expect(useStore.getState().flows).toHaveLength(0);
  });

  it("still updates an already-seen flow while paused", () => {
    useStore.getState().upsertFlow(mkFlow("a")); // seen while recording
    useStore.setState({ recording: false });
    useStore.getState().upsertFlow(mkFlow("a", { status: 500 }));

    const flows = useStore.getState().flows;
    expect(flows).toHaveLength(1);
    expect(flows[0].status).toBe(500);
  });
});

describe("store misc actions", () => {
  beforeEach(reset);

  it("loadFlows replaces the list and clears the selection", () => {
    useStore.setState({ selectedId: "x" });
    useStore.getState().loadFlows([mkFlow("a"), mkFlow("b")]);
    expect(useStore.getState().flows.map((f) => f.id)).toEqual(["a", "b"]);
    expect(useStore.getState().selectedId).toBeNull();
  });

  it("clear empties flows and selection", () => {
    useStore.getState().upsertFlow(mkFlow("a"));
    useStore.setState({ selectedId: "a" });
    useStore.getState().clear();
    expect(useStore.getState().flows).toHaveLength(0);
    expect(useStore.getState().selectedId).toBeNull();
  });
});

describe("prependWithinCap (retention window)", () => {
  const list = (...ids: string[]) => ids.map((id) => mkFlow(id));

  it("prepends while under the cap", () => {
    const { flows, wsMessages } = prependWithinCap(mkFlow("new"), list("a", "b"), {}, 5);
    expect(flows.map((f) => f.id)).toEqual(["new", "a", "b"]);
    expect(wsMessages).toBeUndefined();
  });

  it("evicts the oldest flows once the cap is reached", () => {
    // Newest-first, so "c" is the oldest and must be the one dropped.
    const { flows } = prependWithinCap(mkFlow("new"), list("a", "b", "c"), {}, 3);
    expect(flows.map((f) => f.id)).toEqual(["new", "a", "b"]);
    expect(flows).toHaveLength(3);
  });

  it("drops the WebSocket frames of evicted flows", () => {
    const ws = {
      c: [{ flow_id: "c" } as never],
      a: [{ flow_id: "a" } as never],
    };
    const { wsMessages } = prependWithinCap(mkFlow("new"), list("a", "b", "c"), ws, 3);
    expect(wsMessages).toEqual({ a: ws.a });
  });

  it("leaves the frame map untouched when no evicted flow had frames", () => {
    const ws = { a: [{ flow_id: "a" } as never] };
    const out = prependWithinCap(mkFlow("new"), list("a", "b", "c"), ws, 3);
    // Same object identity: no needless re-render of every WS panel.
    expect(out.wsMessages).toBeUndefined();
  });

  it("is used by the store with a cap matching the engine's retention", () => {
    expect(MAX_FLOWS).toBe(10_000);
  });
});
