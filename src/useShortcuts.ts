import { useEffect, useRef } from "react";
import {
  isMac,
  matchChord,
  SHORTCUTS,
  suppressedWhileTyping,
  type ShortcutId,
} from "./shortcuts";

/** What a shortcut does. Absent means "declared but not wired here". */
export type ShortcutHandlers = Partial<Record<ShortcutId, () => void>>;

/**
 * One `keydown` listener for every global chord.
 *
 * One, not one per component: a chord that two listeners both claim fires twice
 * and `preventDefault`s twice, and the bug only shows up on the machine where
 * the second listener happens to mount first.
 *
 * Scopes narrow rather than multiply. `table` chords are handled by the table's
 * own `onKeyDown` — arrow keys belong to the element that has focus, not to the
 * window — and `modal` chords by the modal. This hook dispatches `global` and
 * `inspector` only, and stops entirely while a modal is open.
 */
export function useShortcuts(
  handlers: ShortcutHandlers,
  opts: { modalOpen: boolean; mac?: boolean } = { modalOpen: false },
) {
  // The handler map is rebuilt every render; keeping it in a ref means the
  // listener is attached once instead of being torn down and re-added on each
  // keystroke-induced re-render.
  const latest = useRef(handlers);
  latest.current = handlers;
  const modalOpen = useRef(opts.modalOpen);
  modalOpen.current = opts.modalOpen;

  useEffect(() => {
    const mac = opts.mac ?? isMac();
    const onKey = (e: KeyboardEvent) => {
      if (modalOpen.current) return;
      for (const s of SHORTCUTS) {
        if (s.native) continue; // the OS menu consumed it before we saw it
        if (s.scope !== "global" && s.scope !== "inspector") continue;
        const run = latest.current[s.id];
        if (!run) continue;
        if (!matchChord(e, s.chord, mac)) continue;
        if (suppressedWhileTyping(s.chord, e.target)) continue;
        // Only now: preventDefault on a chord nothing handles would break the
        // platform's own use of it.
        e.preventDefault();
        run();
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [opts.mac]);
}
