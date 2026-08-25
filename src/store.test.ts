import { beforeEach, describe, expect, it } from "vitest";
import {
  applyFlowBatch,
  appendWsMessages,
  MAX_FLOWS,
  MAX_WS_FRAMES,
  prependWithinCap,
  useStore,
  withoutBodies,
} from "./store";
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

describe("applyFlowBatch (one store update per frame)", () => {
  const list = (...ids: string[]) => ids.map((id) => mkFlow(id));

  it("prepends a batch of new flows newest-first", () => {
    const { flows } = applyFlowBatch(list("a", "b", "c"), [], {}, {}, true, 10);
    expect(flows!.map((f) => f.id)).toEqual(["c", "b", "a"]);
  });

  it("applies a flow's whole lifecycle arriving in one batch", () => {
    const { flows } = applyFlowBatch([mkFlow("a"), mkFlow("b"), mkFlow("a", { state: "Completed", status: 200 })], [], {}, {}, true, 10);
    expect(flows!.map((f) => f.id)).toEqual(["b", "a"]);
    expect(flows!.find((f) => f.id === "a")!.status).toBe(200);
  });

  it("updates flows already in the list without reordering them", () => {
    const existing = list("b", "a");
    const { flows } = applyFlowBatch([mkFlow("a", { status: 404 })], existing, {}, {}, true, 10);
    expect(flows!.map((f) => f.id)).toEqual(["b", "a"]);
    expect(flows![1].status).toBe(404);
    expect(existing[1].status).toBeNull(); // the input list is not mutated
  });

  it("tracks positions as prepends shift them", () => {
    // "old" starts at index 0; two prepends push it to index 2, and its update
    // must still land on it.
    const { flows } = applyFlowBatch([mkFlow("new1"), mkFlow("new2"), mkFlow("old", { status: 500 })], list("old"), {}, {}, true, 10);
    expect(flows!.map((f) => f.id)).toEqual(["new2", "new1", "old"]);
    expect(flows![2].status).toBe(500);
  });

  it("never writes a snapshot over a different flow after an eviction", () => {
    // Cap 2: prepending "new" evicts "old", so "old"'s stale index now points at
    // a row belonging to someone else. Its late snapshot is re-added (as it was
    // before batching) and must not overwrite "recent".
    const { flows } = applyFlowBatch([mkFlow("new"), mkFlow("old", { status: 500 })], list("recent", "old"), {}, {}, true, 2);
    expect(flows!.map((f) => f.id)).toEqual(["old", "new"]);
    expect(flows!.every((f) => f.id !== "recent" || f.status === null)).toBe(true);
  });

  it("drops new flows while paused but still updates known ones", () => {
    const { flows } = applyFlowBatch([mkFlow("fresh"), mkFlow("a", { status: 204 })], list("a"), {}, {}, false, 10);
    expect(flows!.map((f) => f.id)).toEqual(["a"]);
    expect(flows![0].status).toBe(204);
  });

  it("reports no change rather than a new list when nothing applied", () => {
    // Paused, and the batch holds only flows the list never saw.
    expect(applyFlowBatch(list("x"), [], {}, {}, false, 10)).toEqual({});
    expect(applyFlowBatch([], list("a"), {}, {}, true, 10)).toEqual({});
  });

  it("drops the frames of flows the batch evicted", () => {
    const ws = { old: [{ flow_id: "old" } as never] };
    const out = applyFlowBatch([mkFlow("new")], list("recent", "old"), ws, {}, true, 2);
    expect(out.wsMessages).toEqual({});
  });

  it("matches upsertFlow applied one snapshot at a time", () => {
    reset();
    const batch = [mkFlow("a"), mkFlow("b"), mkFlow("a", { status: 200 }), mkFlow("c")];
    for (const f of batch) useStore.getState().upsertFlow(f);
    const oneByOne = useStore.getState().flows;

    reset();
    useStore.getState().upsertFlows(batch);
    expect(useStore.getState().flows).toEqual(oneByOne);
  });
});

describe("body bytes are not kept in the list", () => {
  const withBody = (id: string) =>
    mkFlow(id, {
      request_body: { size: 3, truncated: false, media_type: "application/json", decoded_from: null, text: "{}", base64: null, spilled: false } as never,
      response_body: { size: 9, truncated: true, media_type: "image/png", decoded_from: null, text: null, base64: "AAAA", spilled: true } as never,
    });

  it("keeps every piece of metadata and drops only the bytes", () => {
    const out = withoutBodies(withBody("a"));
    expect(out.request_body).toEqual({
      size: 3, truncated: false, media_type: "application/json", decoded_from: null,
      text: null, base64: null, spilled: false,
    });
    // Truncation, media type and `spilled` survive: the Inspector needs them to
    // know what to fetch and what to say about it.
    expect(out.response_body!.truncated).toBe(true);
    expect(out.response_body!.spilled).toBe(true);
    expect(out.response_body!.base64).toBeNull();
  });

  it("leaves a flow that carries no bytes untouched", () => {
    const bare = mkFlow("a");
    expect(withoutBodies(bare)).toBe(bare); // same object: nothing to copy
  });

  it("strips flows arriving from the engine", () => {
    const { flows } = applyFlowBatch([withBody("a")], [], {}, {}, true, 10);
    expect(flows![0].request_body!.text).toBeNull();
    expect(flows![0].response_body!.base64).toBeNull();
  });

  it("keeps the bodies of an imported session, which nothing else holds", () => {
    reset();
    useStore.getState().loadFlows([withBody("a")]);
    expect(useStore.getState().flows[0].request_body!.text).toBe("{}");
  });
});

describe("appendWsMessages", () => {
  const frame = (flow_id: string, seq: number) => ({ flow_id, seq }) as never;
  const held = (...ids: string[]) => ids.map((id) => mkFlow(id));

  it("appends frames in arrival order, grouped by flow", () => {
    const { wsMessages } = appendWsMessages(
      [frame("a", 1), frame("b", 1), frame("a", 2)],
      held("a", "b"),
      {},
      {},
      MAX_WS_FRAMES,
    );
    expect(wsMessages!.a).toEqual([frame("a", 1), frame("a", 2)]);
    expect(wsMessages!.b).toEqual([frame("b", 1)]);
  });

  it("keeps frames already captured for the flow", () => {
    const prev = { a: [frame("a", 1)] };
    const { wsMessages } = appendWsMessages([frame("a", 2)], held("a"), prev, {}, MAX_WS_FRAMES);
    expect(wsMessages!.a).toHaveLength(2);
    expect(prev.a).toHaveLength(1); // input untouched
  });

  it("reports no change for an empty batch", () => {
    expect(appendWsMessages([], held("a"), { a: [frame("a", 1)] }, {}, MAX_WS_FRAMES)).toEqual({});
  });

  it("drops frames of flows the list no longer holds", () => {
    const { wsMessages } = appendWsMessages(
      [frame("evicted", 1), frame("a", 1)],
      held("a"),
      {},
      {},
      MAX_WS_FRAMES,
    );
    expect(wsMessages!.evicted).toBeUndefined();
    expect(wsMessages!.a).toEqual([frame("a", 1)]);
  });

  it("reports no change when every frame belongs to an evicted flow", () => {
    expect(appendWsMessages([frame("evicted", 1)], held("a"), {}, {}, MAX_WS_FRAMES)).toEqual({});
  });

  it("keeps the newest frames once a socket hits the cap", () => {
    const prev = { a: [frame("a", 1), frame("a", 2), frame("a", 3)] };
    const { wsMessages, wsDropped } = appendWsMessages([frame("a", 4)], held("a"), prev, {}, 3);
    expect(wsMessages!.a).toEqual([frame("a", 2), frame("a", 3), frame("a", 4)]);
    expect(wsDropped!.a).toBe(1);
  });

  it("adds to a socket's dropped count rather than resetting it", () => {
    const prev = { a: [frame("a", 9)] };
    const { wsDropped } = appendWsMessages(
      [frame("a", 10), frame("a", 11)],
      held("a"),
      prev,
      { a: 40 },
      1,
    );
    expect(wsDropped!.a).toBe(42);
  });

  it("caps each socket independently", () => {
    const { wsMessages, wsDropped } = appendWsMessages(
      [frame("a", 1), frame("a", 2), frame("b", 1)],
      held("a", "b"),
      {},
      {},
      1,
    );
    expect(wsMessages!.a).toEqual([frame("a", 2)]);
    expect(wsMessages!.b).toEqual([frame("b", 1)]);
    expect(wsDropped).toEqual({ a: 1 });
  });

  it("leaves the dropped map alone while every socket is under the cap", () => {
    const out = appendWsMessages([frame("a", 1)], held("a"), {}, {}, MAX_WS_FRAMES);
    expect(out.wsDropped).toBeUndefined();
  });

  it("forgets the dropped count of an evicted flow", () => {
    const list = (...ids: string[]) => ids.map((id) => mkFlow(id));
    const out = prependWithinCap(mkFlow("new"), list("recent", "old"), {}, { old: 12 }, 2);
    expect(out.wsDropped).toEqual({});
  });
});

describe("prependWithinCap (retention window)", () => {
  const list = (...ids: string[]) => ids.map((id) => mkFlow(id));

  it("prepends while under the cap", () => {
    const { flows, wsMessages } = prependWithinCap(mkFlow("new"), list("a", "b"), {}, {}, 5);
    expect(flows.map((f) => f.id)).toEqual(["new", "a", "b"]);
    expect(wsMessages).toBeUndefined();
  });

  it("evicts the oldest flows once the cap is reached", () => {
    // Newest-first, so "c" is the oldest and must be the one dropped.
    const { flows } = prependWithinCap(mkFlow("new"), list("a", "b", "c"), {}, {}, 3);
    expect(flows.map((f) => f.id)).toEqual(["new", "a", "b"]);
    expect(flows).toHaveLength(3);
  });

  it("drops the WebSocket frames of evicted flows", () => {
    const ws = {
      c: [{ flow_id: "c" } as never],
      a: [{ flow_id: "a" } as never],
    };
    const { wsMessages } = prependWithinCap(mkFlow("new"), list("a", "b", "c"), ws, {}, 3);
    expect(wsMessages).toEqual({ a: ws.a });
  });

  it("leaves the frame map untouched when no evicted flow had frames", () => {
    const ws = { a: [{ flow_id: "a" } as never] };
    const out = prependWithinCap(mkFlow("new"), list("a", "b", "c"), ws, {}, 3);
    // Same object identity: no needless re-render of every WS panel.
    expect(out.wsMessages).toBeUndefined();
  });

  it("is used by the store with a cap matching the engine's retention", () => {
    expect(MAX_FLOWS).toBe(10_000);
  });
});

describe("annotations", () => {
  it("pins toggle, and are a set rather than a list", () => {
    const s = useStore.getState();
    s.togglePin("a");
    s.togglePin("b");
    s.togglePin("a");
    expect([...useStore.getState().pinned]).toEqual(["b"]);
  });

  it("an empty comment removes the note instead of storing a blank", () => {
    const s = useStore.getState();
    s.setComment("a", "  look at this  ");
    expect(useStore.getState().comments.a).toBe("look at this");
    s.setComment("a", "   ");
    expect(useStore.getState().comments).toEqual({});
  });

  it("Clear drops annotations with the flows they annotate", () => {
    // A pin on a flow that no longer exists is a sidebar count pointing at
    // nothing.
    const s = useStore.getState();
    s.togglePin("a");
    s.setComment("a", "note");
    useStore.getState().clear();
    expect([...useStore.getState().pinned]).toEqual([]);
    expect(useStore.getState().comments).toEqual({});
  });
});
