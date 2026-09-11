import { useRef } from "react";

/** What a drag reports to the caller, for turning a pointer into a value. */
export interface DragContext {
  /** Pointer movement since the grab, in px. */
  dx: number;
  dy: number;
  /** The value the divider had when it was grabbed. */
  start: number;
  /** Rect of the divider's container — the basis a percentage is a percentage of. */
  rect: DOMRect;
}

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
 * - **Anchored on the grab, not on the cursor's absolute position.** `measure`
 *   is handed how far the pointer has moved and what the value was when it was
 *   grabbed, so a press with no movement resolves to exactly the current value.
 *   Measuring absolutely instead made a plain click snap the divider to
 *   wherever the arithmetic happened to land — the inspector collapsed to its
 *   minimum on every click, because the handle sits at the bottom edge of the
 *   pane the position was being measured against.
 * - **Live value while dragging, committed once at the end.** A drag emits
 *   hundreds of moves; writing a preference on each one would write hundreds of
 *   times, so `onDrag` paints and `onCommit` persists — and a press that never
 *   moved commits nothing at all.
 * - **Keyboard and double-click.** Arrow keys nudge (`step`, or 4× with Shift)
 *   so the divider is not mouse-only, and double-click resets it — a pane
 *   dragged to nothing has to be recoverable without hunting in Settings.
 */
export function Splitter({
  orientation,
  value,
  min,
  max,
  reset,
  step = 12,
  onDrag,
  onCommit,
  label,
  /** Maps a drag to a value, in whatever unit the caller is sizing in. */
  measure,
}: {
  /** `vertical` = a vertical bar you drag left/right. */
  orientation: "vertical" | "horizontal";
  value: number;
  min: number;
  max: number;
  reset: number;
  /** One arrow key's worth of movement, in the value's own unit (px, or %). */
  step?: number;
  onDrag: (v: number) => void;
  onCommit: (v: number) => void;
  label: string;
  measure: (ctx: DragContext) => number;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const clamp = (v: number) => Math.min(max, Math.max(min, v));

  const startDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const handle = ref.current;
    // The percentages are of the container the handle lives in, which is also
    // the box the sibling panes divide up — so that is the rect to measure in.
    const container = handle?.parentElement;
    if (!handle || !container) return;
    const rect = container.getBoundingClientRect();
    const start = value;
    const [sx, sy] = [e.clientX, e.clientY];
    let moved = false;
    handle.setPointerCapture(e.pointerId);

    const at = (ev: PointerEvent) =>
      clamp(measure({ dx: ev.clientX - sx, dy: ev.clientY - sy, start, rect }));
    const move = (ev: PointerEvent) => {
      moved = true;
      onDrag(at(ev));
    };
    const up = (ev: PointerEvent) => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      handle.releasePointerCapture(ev.pointerId);
      if (moved) onCommit(at(ev));
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };

  const nudge = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const amount = e.shiftKey ? step * 4 : step;
    const back = orientation === "vertical" ? "ArrowLeft" : "ArrowUp";
    const fwd = orientation === "vertical" ? "ArrowRight" : "ArrowDown";
    const delta = e.key === back ? -amount : e.key === fwd ? amount : 0;
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
