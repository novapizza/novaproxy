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

  it("drops queued flows but not frames on clear", () => {
    const q = new StreamQueue(10);
    q.push({ flows: [flow("a")], ws: [frame(1)] });
    q.discardFlows();
    const out = q.take();
    expect(out.flows).toEqual([]);
    expect(out.ws).toHaveLength(1);
  });
});
