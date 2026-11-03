import { describe, expect, it } from "vitest";
import type { Flow } from "./bindings/Flow";
import type { WsMessage } from "./bindings/WsMessage";
import { StreamQueue } from "./streamQueue";

const flow = (id: string, state: Flow["state"] = "Started") => ({ id, state }) as Flow;
const frame = (seq: number) => ({ flow_id: "a", seq: BigInt(seq) }) as WsMessage;

describe("StreamQueue", () => {
  it("applies a flow once, as it last was, where it was first queued", () => {
    const q = new StreamQueue(10);
    q.push({ flows: [flow("a"), flow("b")], ws: [] });
    q.push({ flows: [flow("a", "Completed")], ws: [] });
    const { flows } = q.take();
    expect(flows.map((f) => f.id)).toEqual(["a", "b"]);
    expect(flows[0].state).toBe("Completed");
    expect(q.take().flows).toEqual([]);
  });

  it("keeps only the newest flows past its cap", () => {
    const q = new StreamQueue(2);
    q.push({ flows: [flow("a"), flow("b"), flow("c")], ws: [] });
    expect(q.take().flows.map((f) => f.id)).toEqual(["b", "c"]);
  });

  it("keeps only the newest frames past its cap", () => {
    const q = new StreamQueue(10, 2);
    q.push({ flows: [], ws: [frame(1), frame(2), frame(3)] });
    expect(q.take().ws.map((m) => Number(m.seq))).toEqual([2, 3]);
  });

  it("counts the frames it drops against their socket", () => {
    const q = new StreamQueue(10, 2);
    q.push({ flows: [], ws: [frame(1), frame(2), frame(3)] });
    expect(q.take().ws_dropped).toEqual({ a: 1 });
    expect(q.take().ws_dropped).toBeUndefined();
  });

  it("keeps the newest frames that fit its byte budget", () => {
    const sized = (seq: number) => ({ ...frame(seq), text: "x".repeat(10) }) as WsMessage;
    const q = new StreamQueue(10, 100, 25);
    q.push({ flows: [], ws: [sized(1), sized(2)] });
    q.push({ flows: [], ws: [sized(3)] });
    const out = q.take();
    expect(out.ws.map((m) => Number(m.seq))).toEqual([2, 3]);
    expect(out.ws_dropped).toEqual({ a: 1 });
  });

  it("keeps the newest frame even when it alone is over the byte budget", () => {
    const q = new StreamQueue(10, 100, 5);
    q.push({ flows: [], ws: [{ ...frame(1), text: "x".repeat(10) } as WsMessage] });
    expect(q.take().ws).toHaveLength(1);
  });

  it("adds the engine's dropped counts to its own", () => {
    const q = new StreamQueue(10, 1);
    q.push({ flows: [], ws: [], ws_dropped: { a: 5, b: 2 } });
    q.push({ flows: [], ws: [frame(1), frame(2)] });
    expect(q.take().ws_dropped).toEqual({ a: 6, b: 2 });
  });

  it("drops queued flows but not frames on clear", () => {
    const q = new StreamQueue(10);
    q.push({ flows: [flow("a")], ws: [frame(1)] });
    q.discardFlows();
    const out = q.take();
    expect(out.flows).toEqual([]);
    expect(out.ws).toHaveLength(1);
  });
});
