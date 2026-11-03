import type { Flow } from "./bindings/Flow";
import type { WsMessage } from "./bindings/WsMessage";
import { frameBytes } from "./store";

/** One message from the engine's UI stream: changed flows, then frames. */
export interface StreamBatch {
  flows: Flow[];
  ws: WsMessage[];
  /** Frames dropped before they could be sent, per flow id; absent when none were. */
  ws_dropped?: Record<string, number>;
}

/**
 * How often queued snapshots reach the store, at most.
 *
 * Every flush re-derives the tree, the filter and the sort over up to 10k flows,
 * so its rate is what the main thread pays under load. Five a second still reads
 * as live; sixty (one per animation frame, as it was) left no time for anything
 * else once traffic was heavy.
 */
export const FLUSH_MS = 200;

/** Frames held between flushes, at most; past it the oldest are dropped. */
export const MAX_QUEUED_FRAMES = 20_000;

/**
 * Frame payload held between flushes, at most. While the window is hidden no
 * flush runs, and 20,000 frames of up to 64 KB is over a gigabyte.
 *
 * Temporary, like `MAX_WS_BYTES`: it goes once frames stay in the engine.
 */
export const MAX_QUEUED_WS_BYTES = 32 * 1024 * 1024;

/**
 * What arrived since the last flush.
 *
 * Flows are keyed by id, so a flow that changed twice is applied once, as it
 * now is, in the position it was first queued. That matters while the window is
 * hidden: no animation frame fires, nothing flushes, and the queue is all that
 * grows — so it is bounded. Past `flowCap` distinct flows the oldest are dropped,
 * which loses nothing the list would have kept: `flowCap` newer flows would
 * have evicted them anyway.
 */
export class StreamQueue {
  private flows = new Map<string, Flow>();
  private ws: WsMessage[] = [];
  private wsBytes = 0;
  /** Frames dropped on the way here, by the engine or by this queue, per flow. */
  private wsDropped: Record<string, number> = {};

  constructor(
    private readonly flowCap: number,
    private readonly wsCap = MAX_QUEUED_FRAMES,
    private readonly wsByteCap = MAX_QUEUED_WS_BYTES,
  ) {}

  push(batch: StreamBatch) {
    for (const f of batch.flows) this.flows.set(f.id, f);
    for (const id of this.flows.keys()) {
      if (this.flows.size <= this.flowCap) break;
      this.flows.delete(id);
    }
    for (const id in batch.ws_dropped ?? {}) this.countDropped(id, batch.ws_dropped![id]);
    if (batch.ws.length) {
      for (const m of batch.ws) this.wsBytes += frameBytes(m);
      this.ws.push(...batch.ws);
      // Past either cap the oldest go; the newest frame always stays.
      let cut = 0;
      while (
        cut < this.ws.length - 1 &&
        (this.ws.length - cut > this.wsCap || this.wsBytes > this.wsByteCap)
      ) {
        this.wsBytes -= frameBytes(this.ws[cut]);
        this.countDropped(this.ws[cut].flow_id, 1);
        cut += 1;
      }
      if (cut) this.ws = this.ws.slice(cut);
    }
  }

  take(): StreamBatch {
    const out: StreamBatch = { flows: [...this.flows.values()], ws: this.ws };
    if (Object.keys(this.wsDropped).length) out.ws_dropped = this.wsDropped;
    this.flows = new Map();
    this.ws = [];
    this.wsBytes = 0;
    this.wsDropped = {};
    return out;
  }

  private countDropped(id: string, n: number) {
    this.wsDropped[id] = (this.wsDropped[id] ?? 0) + n;
  }

  /** Flows and frames waiting for the next flush. */
  get size() {
    return { flows: this.flows.size, ws: this.ws.length };
  }

  /** The capture was cleared: what is queued belongs to it. */
  discardFlows() {
    this.flows.clear();
  }
}
