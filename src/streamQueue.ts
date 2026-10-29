import type { Flow } from "./bindings/Flow";
import type { WsMessage } from "./bindings/WsMessage";

/** One message from the engine's UI stream: changed flows, then frames. */
export interface StreamBatch {
  flows: Flow[];
  ws: WsMessage[];
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

  constructor(
    private readonly flowCap: number,
    private readonly wsCap = MAX_QUEUED_FRAMES,
  ) {}

  push(batch: StreamBatch) {
    for (const f of batch.flows) this.flows.set(f.id, f);
    for (const id of this.flows.keys()) {
      if (this.flows.size <= this.flowCap) break;
      this.flows.delete(id);
    }
    if (batch.ws.length) {
      this.ws.push(...batch.ws);
      if (this.ws.length > this.wsCap) this.ws = this.ws.slice(-this.wsCap);
    }
  }

  take(): StreamBatch {
    const out = { flows: [...this.flows.values()], ws: this.ws };
    this.flows = new Map();
    this.ws = [];
    return out;
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
