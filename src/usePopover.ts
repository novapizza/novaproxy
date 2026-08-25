import { useEffect, useRef, useState } from "react";

/**
 * Open/close state for a popover that closes on click-outside or Escape.
 *
 * Click-outside rather than a scrim, deliberately: these all sit above the flows
 * table, and the point of a popover here is that the rows behind it stay readable
 * while you pick.
 *
 * `Escape` is stopped from propagating, because the app's global Escape closes
 * whatever dialog is open — and a popover inside a dialog must swallow the first
 * press rather than closing the dialog out from under itself.
 */
export function usePopover<T extends HTMLElement = HTMLDivElement>() {
  const [open, setOpen] = useState(false);
  const ref = useRef<T | null>(null);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setOpen(false);
    };
    window.addEventListener("mousedown", away);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", away);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  return { open, setOpen, ref, toggle: () => setOpen((v) => !v) };
}
