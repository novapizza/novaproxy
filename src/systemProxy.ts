import { api, type ProxyStatus } from "./api";
import { useStore } from "./store";

/**
 * Turn the system proxy on or off — the one path every caller takes (the
 * header switch, the command palette, launch, the walkthrough), so they all
 * show the same busy state and none can start a second change mid-flight.
 *
 * With `followRecording`, recording goes on and off with the proxy (see
 * `Prefs.recordingFollowsProxy`). Returns the new status, or `null` when a
 * change was already in progress and this one was dropped. Throws what the
 * backend reported.
 */
export async function switchSystemProxy(
  enable: boolean,
  { followRecording }: { followRecording: boolean },
): Promise<ProxyStatus | null> {
  const store = useStore.getState();
  if (store.proxyBusy) return null;
  store.setProxyBusy(true);
  try {
    const next = await api.setSystemProxy(enable);
    const s = useStore.getState();
    s.setProxy(next);
    if (followRecording && next.system_proxy === enable) s.setRecording(enable);
    return next;
  } finally {
    useStore.getState().setProxyBusy(false);
  }
}
