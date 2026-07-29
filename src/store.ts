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

  upsertFlow: (f: Flow) => void;
  addWsMessage: (m: WsMessage) => void;
  loadFlows: (flows: Flow[]) => void;
  clear: () => void;
  setRecording: (v: boolean) => void;
  select: (id: string | null) => void;
  setProxy: (p: ProxyStatus) => void;
  setCa: (c: CaStatus | null) => void;
}

/**
 * Retention window for the flow list. The engine keeps the same number of flows
 * (`DEFAULT_MAX_FLOWS`); without a matching cap here a long session grows the
 * React store — and its per-row render cost — without bound.
 */
export const MAX_FLOWS = 10_000;

/**
 * Prepend a newly-seen flow (the list is newest-first), evicting from the tail
 * once `cap` is reached. Captured WebSocket frames of evicted flows go too —
 * they are the larger allocation, and nothing can reach them again.
 */
export function prependWithinCap(
  flow: Flow,
  flows: Flow[],
  wsMessages: Record<string, WsMessage[]>,
  cap: number,
): { flows: Flow[]; wsMessages?: Record<string, WsMessage[]> } {
  if (flows.length < cap) return { flows: [flow, ...flows] };
  const kept = [flow, ...flows.slice(0, cap - 1)];
  const dropped = flows.slice(cap - 1);
  if (!dropped.some((d) => wsMessages[d.id])) return { flows: kept };
  const next = { ...wsMessages };
  for (const d of dropped) delete next[d.id];
  return { flows: kept, wsMessages: next };
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
  recording: true,
  selectedId: null,
  proxy: emptyProxy,
  ca: null,
  wsMessages: {},

  // Snapshots arrive multiple times per flow (started → response → completed).
  // Replace in place if we've seen the id; otherwise prepend (newest first).
  upsertFlow: (f) =>
    set((s) => {
      const idx = s.flows.findIndex((x) => x.id === f.id);
      if (idx >= 0) {
        const next = s.flows.slice();
        next[idx] = f;
        return { flows: next };
      }
      if (!s.recording) return {};
      return prependWithinCap(f, s.flows, s.wsMessages, MAX_FLOWS);
    }),
  // Append a captured WS frame to its flow's list (ordered by arrival).
  addWsMessage: (m) =>
    set((s) => {
      const prev = s.wsMessages[m.flow_id] ?? [];
      return { wsMessages: { ...s.wsMessages, [m.flow_id]: [...prev, m] } };
    }),
  // Replace the flow list (used when importing a saved .nova session).
  loadFlows: (flows) => set({ flows, selectedId: null, wsMessages: {} }),
  clear: () => set({ flows: [], selectedId: null, wsMessages: {} }),
  setRecording: (v) => set({ recording: v }),
  select: (id) => set({ selectedId: id }),
  setProxy: (p) => set({ proxy: p }),
  setCa: (c) => set({ ca: c }),
}));
