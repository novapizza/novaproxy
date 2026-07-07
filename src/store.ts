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

const emptyProxy: ProxyStatus = {
  running: false,
  host: null,
  port: null,
  flows_captured: 0n,
  system_proxy: false,
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
      return { flows: [f, ...s.flows] };
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
