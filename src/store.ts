import { create } from "zustand";
import type { Flow, ProxyStatus, CaStatus, WsMessage } from "./api";

interface Store {
  flows: Flow[];
  recording: boolean;
  selectedId: string | null;
  proxy: ProxyStatus;
  ca: CaStatus | null;
  /** Captured WebSocket frames, keyed by the upgrade flow's id. */
  wsMessages: Record<string, WsMessage[]>;
  /** How many of a socket's oldest frames the cap has dropped, by flow id. */
  wsDropped: Record<string, number>;
  /**
   * Flows the user pinned, and notes they wrote on them.
   *
   * Session-only, deliberately: a flow id is minted per exchange, so a pin
   * restored from disk would point at nothing after a restart. Keyed by id
   * rather than held on the flow because a snapshot replaces the whole flow
   * object several times per exchange and would wipe an annotation written
   * mid-flight.
   */
  pinned: ReadonlySet<string>;
  comments: Readonly<Record<string, string>>;

  upsertFlow: (f: Flow) => void;
  /** Apply a frame's worth of snapshots in one store update. */
  upsertFlows: (batch: Flow[]) => void;
  addWsMessage: (m: WsMessage) => void;
  /** Append a frame's worth of captured frames in one store update. */
  /** Append a batch of frames, plus counts of frames dropped before they arrived. */
  addWsMessages: (batch: WsMessage[], dropped?: Record<string, number>) => void;
  loadFlows: (flows: Flow[]) => void;
  clear: () => void;
  setRecording: (v: boolean) => void;
  select: (id: string | null) => void;
  setProxy: (p: ProxyStatus) => void;
  setCa: (c: CaStatus | null) => void;
  togglePin: (id: string) => void;
  /** An empty note removes the comment rather than storing a blank. */
  setComment: (id: string, text: string) => void;
}

/**
 * Retention window for the flow list. The engine keeps the same number of flows
 * (`DEFAULT_MAX_FLOWS`); without a matching cap here a long session grows the
 * React store — and its per-row render cost — without bound.
 */
export const MAX_FLOWS = 10_000;

/**
 * Frames kept per socket. A chatty WebSocket is otherwise unbounded — the same
 * failure as the flow list, one panel down — so the oldest frames are dropped
 * and the panel says how many.
 */
export const MAX_WS_FRAMES = 2_000;

/**
 * Frame payload held across every socket, at most; past it the oldest frames go,
 * whichever socket they belong to. `MAX_WS_FRAMES` alone let one socket hold
 * 2,000 frames of up to 64 KB each, and nothing bounded the sockets together.
 *
 * Temporary: frames are to move into the engine and be fetched for the socket
 * actually open, like bodies are, at which point this budget goes.
 */
export const MAX_WS_BYTES = 64 * 1024 * 1024;

/** Payload a frame holds, counted in string length. */
export function frameBytes(m: WsMessage): number {
  return (m.text?.length ?? 0) + (m.base64?.length ?? 0);
}

/**
 * A body, minus its bytes.
 *
 * The engine caps each preview at 512 KB and the list retains `MAX_FLOWS` of
 * them: holding every one in the webview is what made a long capture run out of
 * memory (JS strings are UTF-16, so an ASCII body costs double). The metadata
 * stays — size, media type, whether it was truncated — so the Inspector still
 * knows what is there and fetches the bytes of the flow actually open, through
 * the `read_body` command. The engine retains the flow either way, which is what
 * makes that fetch answerable.
 *
 * Imported sessions are exempt: their flows exist nowhere else, so `loadFlows`
 * keeps them whole.
 */
function withoutBytes(body: Flow["request_body"]): Flow["request_body"] {
  if (!body || (body.text == null && body.base64 == null)) return body;
  return { ...body, text: null, base64: null };
}

/** A flow as the list keeps it: everything but the body bytes. */
export function withoutBodies(flow: Flow): Flow {
  const request_body = withoutBytes(flow.request_body);
  const response_body = withoutBytes(flow.response_body);
  if (request_body === flow.request_body && response_body === flow.response_body) return flow;
  return { ...flow, request_body, response_body };
}

/**
 * Prepend a newly-seen flow (the list is newest-first), evicting from the tail
 * once `cap` is reached. Captured WebSocket frames of evicted flows go too —
 * they are the larger allocation, and nothing can reach them again.
 */
export function prependWithinCap(
  flow: Flow,
  flows: Flow[],
  wsMessages: Record<string, WsMessage[]>,
  wsDropped: Record<string, number>,
  cap: number,
): {
  flows: Flow[];
  wsMessages?: Record<string, WsMessage[]>;
  wsDropped?: Record<string, number>;
} {
  if (flows.length < cap) return { flows: [flow, ...flows] };
  const kept = [flow, ...flows.slice(0, cap - 1)];
  const dropped = flows.slice(cap - 1);
  const hadFrames = dropped.some((d) => wsMessages[d.id] || wsDropped[d.id]);
  if (!hadFrames) return { flows: kept };
  const nextMessages = { ...wsMessages };
  const nextDropped = { ...wsDropped };
  for (const d of dropped) {
    delete nextMessages[d.id];
    delete nextDropped[d.id];
  }
  return { flows: kept, wsMessages: nextMessages, wsDropped: nextDropped };
}

/**
 * Apply a batch of snapshots to the list.
 *
 * Snapshots arrive several times per flow (started → response → completed) and,
 * under load, hundreds of times a second; the channel coalesces them into one
 * batch per animation frame. Doing that here rather than per snapshot is what
 * keeps a burst of traffic from re-rendering the list once per message.
 *
 * Flows are found through an id index built once per batch instead of a scan per
 * snapshot. Prepending shifts every index by one, so the index is read through
 * `shift` and the id is confirmed before writing — an entry can also be stale
 * because its flow was evicted at the tail.
 *
 * Body bytes are dropped on the way in (see `withoutBodies`); the engine keeps
 * them, and the Inspector asks for the one it is showing.
 */
export function applyFlowBatch(
  batch: Flow[],
  flows: Flow[],
  wsMessages: Record<string, WsMessage[]>,
  wsDropped: Record<string, number>,
  recording: boolean,
  cap: number,
): {
  flows?: Flow[];
  wsMessages?: Record<string, WsMessage[]>;
  wsDropped?: Record<string, number>;
} {
  if (batch.length === 0) return {};

  const index = new Map<string, number>();
  for (let i = 0; i < flows.length; i++) index.set(flows[i].id, i);

  let next = flows;
  let ws: Record<string, WsMessage[]> | undefined;
  let dropped: Record<string, number> | undefined;
  let shift = 0;
  let copied = false;

  for (const raw of batch) {
    const flow = withoutBodies(raw);
    const stored = index.get(flow.id);
    const at = stored === undefined ? -1 : stored + shift;
    if (at >= 0 && at < next.length && next[at].id === flow.id) {
      if (!copied) {
        next = next.slice();
        copied = true;
      }
      next[at] = flow;
      continue;
    }
    if (!recording) continue;
    const out = prependWithinCap(flow, next, ws ?? wsMessages, dropped ?? wsDropped, cap);
    next = out.flows; // a fresh array, so later in-place writes are safe
    if (out.wsMessages) ws = out.wsMessages;
    if (out.wsDropped) dropped = out.wsDropped;
    copied = true;
    shift += 1;
    index.set(flow.id, -shift); // now at index 0
  }

  if (next === flows && !ws) return {};
  const change: {
    flows: Flow[];
    wsMessages?: Record<string, WsMessage[]>;
    wsDropped?: Record<string, number>;
  } = { flows: next };
  if (ws) change.wsMessages = ws;
  if (dropped) change.wsDropped = dropped;
  return change;
}

/**
 * Append captured frames, grouped by flow so a batch touching one socket copies
 * that socket's list once rather than once per frame.
 *
 * Frames belonging to no flow in `flows` are dropped: eviction deletes a flow's
 * frames, and a socket that keeps streaming afterwards would otherwise recreate
 * the entry — an orphan nothing renders and nothing ever cleans up. (The App
 * applies each animation frame's snapshots before its WS frames, so a frame
 * cannot outrun the flow it belongs to.)
 *
 * Only the newest `cap` frames of a socket are kept. The count of what was
 * dropped is kept alongside so the panel can say so, rather than quietly showing
 * a partial conversation.
 */
export function appendWsMessages(
  batch: WsMessage[],
  flows: Flow[],
  wsMessages: Record<string, WsMessage[]>,
  wsDropped: Record<string, number>,
  cap: number,
  opts: { byteBudget?: number; dropped?: Record<string, number> } = {},
): { wsMessages?: Record<string, WsMessage[]>; wsDropped?: Record<string, number> } {
  const upstream = opts.dropped ?? {};
  const hasUpstream = Object.keys(upstream).length > 0;
  if (batch.length === 0 && !hasUpstream) return {};
  const retained = new Set(flows.map((f) => f.id));
  let dropped: Record<string, number> | undefined;
  // Frames the engine or the queue dropped on the way count like ones cut here.
  for (const id in upstream) {
    if (!retained.has(id)) continue;
    dropped = dropped ?? { ...wsDropped };
    dropped[id] = (dropped[id] ?? 0) + upstream[id];
  }
  const byFlow = new Map<string, WsMessage[]>();
  for (const m of batch) {
    if (!retained.has(m.flow_id)) continue;
    const seen = byFlow.get(m.flow_id);
    if (seen) seen.push(m);
    else byFlow.set(m.flow_id, [m]);
  }
  if (byFlow.size === 0) return dropped ? { wsDropped: dropped } : {};

  const next = { ...wsMessages };
  for (const [id, add] of byFlow) {
    const prev = next[id];
    const all = prev ? prev.concat(add) : add;
    if (all.length <= cap) {
      next[id] = all;
      continue;
    }
    const over = all.length - cap;
    next[id] = all.slice(over);
    dropped = dropped ?? { ...wsDropped };
    dropped[id] = (dropped[id] ?? 0) + over;
  }
  const evicted = evictOverBudget(next, opts.byteBudget ?? Infinity);
  for (const id in evicted) {
    dropped = dropped ?? { ...wsDropped };
    dropped[id] = (dropped[id] ?? 0) + evicted[id];
  }
  return dropped ? { wsMessages: next, wsDropped: dropped } : { wsMessages: next };
}

/**
 * Drop the oldest frames across every socket until their payload fits `budget`,
 * replacing the arrays in `held` (never mutating them). Returns how many frames
 * each socket lost.
 */
function evictOverBudget(held: Record<string, WsMessage[]>, budget: number): Record<string, number> {
  const lost: Record<string, number> = {};
  if (!Number.isFinite(budget)) return lost;
  let total = 0;
  for (const id in held) for (const m of held[id]) total += frameBytes(m);
  if (total <= budget) return lost;

  // Start index per socket, so eviction walks forward instead of shifting arrays.
  const start: Record<string, number> = {};
  for (const id in held) start[id] = 0;
  while (total > budget) {
    let oldest: string | null = null;
    for (const id in held) {
      const m = held[id][start[id]];
      if (m && (oldest === null || m.at < held[oldest][start[oldest]].at)) oldest = id;
    }
    if (oldest === null) break;
    total -= frameBytes(held[oldest][start[oldest]]);
    start[oldest] += 1;
    lost[oldest] = (lost[oldest] ?? 0) + 1;
  }
  for (const id in lost) held[id] = held[id].slice(start[id]);
  return lost;
}

const emptyProxy: ProxyStatus = {
  running: false,
  host: null,
  port: null,
  flows_captured: 0n,
  system_proxy: false,
  pending_restore: false,
};

export const useStore = create<Store>((set) => ({
  flows: [],
  // Paused until asked. A fresh launch has no engine running and no proxy
  // pointed at it, so "recording" would be a promise the app cannot keep —
  // and the first thing the onboarding walkthrough does is hand the user the
  // button that starts it.
  recording: false,
  selectedId: null,
  proxy: emptyProxy,
  ca: null,
  wsMessages: {},
  wsDropped: {},
  pinned: new Set(),
  comments: {},

  // Snapshots arrive multiple times per flow (started → response → completed).
  // Replace in place if we've seen the id; otherwise prepend (newest first).
  upsertFlow: (f) =>
    set((s) => applyFlowBatch([f], s.flows, s.wsMessages, s.wsDropped, s.recording, MAX_FLOWS)),
  upsertFlows: (batch) =>
    set((s) => applyFlowBatch(batch, s.flows, s.wsMessages, s.wsDropped, s.recording, MAX_FLOWS)),
  // Append captured WS frames to their flow's list (ordered by arrival).
  addWsMessage: (m) =>
    set((s) =>
      appendWsMessages([m], s.flows, s.wsMessages, s.wsDropped, MAX_WS_FRAMES, { byteBudget: MAX_WS_BYTES }),
    ),
  addWsMessages: (batch, dropped) =>
    set((s) =>
      appendWsMessages(batch, s.flows, s.wsMessages, s.wsDropped, MAX_WS_FRAMES, {
        byteBudget: MAX_WS_BYTES,
        dropped,
      }),
    ),
  // Replace the flow list (used when importing a saved .nova session). Imported
  // flows keep their bodies: nothing else has a copy to fetch them from.
  loadFlows: (flows) => set({ flows, selectedId: null, wsMessages: {}, wsDropped: {} }),
  // Annotations go with the flows they annotate: a pin on a flow that no longer
  // exists is a count in the sidebar pointing at nothing.
  clear: () =>
    set({
      flows: [],
      selectedId: null,
      wsMessages: {},
      wsDropped: {},
      pinned: new Set(),
      comments: {},
    }),
  setRecording: (v) => set({ recording: v }),
  select: (id) => set({ selectedId: id }),
  setProxy: (p) => set({ proxy: p }),
  setCa: (c) => set({ ca: c }),
  togglePin: (id) =>
    set((s) => {
      const next = new Set(s.pinned);
      if (!next.delete(id)) next.add(id);
      return { pinned: next };
    }),
  setComment: (id, text) =>
    set((s) => {
      const next = { ...s.comments };
      const trimmed = text.trim();
      if (trimmed === "") delete next[id];
      else next[id] = trimmed;
      return { comments: next };
    }),
}));
