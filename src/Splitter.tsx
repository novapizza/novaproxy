import { useRef } from "react";

/**
 * A draggable divider between two panes.
 *
 * One component for all three of them (sidebar, inspector, request/response),
 * because the fiddly parts are identical and were worth getting right once:
 *
 * - **Pointer capture, not window listeners.** It keeps the drag alive when the
 *   cursor outruns the handle or leaves the window, and releases itself if the
 *   pointer is lost — a drag that stays stuck to the mouse after a lost pointer
 *   event is the bug this avoids.
 * - **Live value while dragging, committed once at the end.** A drag emits
 *   hundreds of moves; writing a preference on each one would write hundreds of
 *   times, so `onDrag` paints and `onCommit` persists.
 * - **Keyboard and double-click.** Arrow keys nudge (12px, or 48 with Shift) so
 *   the divider is not mouse-only, and double-click resets it — a pane dragged
 *   to nothing has to be recoverable without hunting in Settings.
 */
export function Splitter({
  orientation,
  value,
  min,
  max,
  reset,
  onDrag,
  onCommit,
  label,
  /** Maps a pointer position to a value. Given the client x/y and the rect of
   *  the element being sized, so a caller can size in px or in percent. */
  measure,
}: {
  /** `vertical` = a vertical bar you drag left/right. */
  orientation: "vertical" | "horizontal";
  value: number;
  min: number;
  max: number;
  reset: number;
  onDrag: (v: number) => void;
  onCommit: (v: number) => void;
  label: string;
  measure: (e: { clientX: number; clientY: number }, rect: DOMRect) => number;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const clamp = (v: number) => Math.min(max, Math.max(min, v));

  const startDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const handle = ref.current;
    // The pane being sized is the sibling before the handle, which is what the
    // caller's `measure` is written against.
    const pane = handle?.previousElementSibling as HTMLElement | null;
    if (!handle || !pane) return;
    const rect = pane.getBoundingClientRect();
    handle.setPointerCapture(e.pointerId);

    const move = (ev: PointerEvent) => onDrag(clamp(measure(ev, rect)));
    const up = (ev: PointerEvent) => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      handle.releasePointerCapture(ev.pointerId);
      onCommit(clamp(measure(ev, rect)));
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };

  const nudge = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? 48 : 12;
    const back = orientation === "vertical" ? "ArrowLeft" : "ArrowUp";
    const fwd = orientation === "vertical" ? "ArrowRight" : "ArrowDown";
    const delta = e.key === back ? -step : e.key === fwd ? step : 0;
    if (!delta) return;
    e.preventDefault();
    const next = clamp(value + delta);
    onDrag(next);
    onCommit(next);
  };

  return (
    <div
      ref={ref}
      className={`splitter ${orientation}`}
      role="separator"
      aria-orientation={orientation}
      aria-label={label}
      aria-valuenow={Math.round(value)}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      onPointerDown={startDrag}
      onKeyDown={nudge}
      onDoubleClick={() => {
        onDrag(reset);
        onCommit(reset);
      }}
    >
      <span className="grip" />
    </div>
  );
}
