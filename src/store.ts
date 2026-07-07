import { create } from "zustand";
import type { Flow, ProxyStatus, CaStatus } from "./api";

interface Store {
  flows: Flow[];
  recording: boolean;
  selectedId: string | null;
  proxy: ProxyStatus;
  ca: CaStatus | null;

  upsertFlow: (f: Flow) => void;
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
  // Replace the flow list (used when importing a saved .nova session).
  loadFlows: (flows) => set({ flows, selectedId: null }),
  clear: () => set({ flows: [], selectedId: null }),
  setRecording: (v) => set({ recording: v }),
  select: (id) => set({ selectedId: id }),
  setProxy: (p) => set({ proxy: p }),
  setCa: (c) => set({ ca: c }),
}));
