/* The dropdown from design.md §Dropdown. A native <select> cannot carry the
   per-row icon, the mint-washed selected row or the destructive "clear" row the
   design specifies, so the control is ours — which means the keyboard support a
   native select gave away for free has to be written out here. */
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Icon, type IconName } from "./icons";

export interface DropdownItem {
  value: string;
  label: string;
  icon?: IconName;
}

export type DropdownKey = "up" | "down" | "home" | "end";

/**
 * Roving-highlight arithmetic for the open panel, kept pure so the wrap-around
 * cases are pinned by tests rather than re-derived in a keydown handler.
 * `current` is -1 when nothing is highlighted yet; the result is -1 only for an
 * empty list.
 */
export function nextIndex(key: DropdownKey, current: number, count: number): number {
  if (count <= 0) return -1;
  switch (key) {
    case "home":
      return 0;
    case "end":
      return count - 1;
    case "down":
      return current < 0 || current >= count - 1 ? 0 : current + 1;
    case "up":
      return current <= 0 || current > count - 1 ? count - 1 : current - 1;
  }
}

const ARROWS: Record<string, DropdownKey> = {
  ArrowDown: "down",
  ArrowUp: "up",
  Home: "home",
  End: "end",
};

export function Dropdown({
  value,
  items,
  onChange,
  label,
  placeholder,
  emptyLabel,
  clearLabel,
  onClear,
  disabled,
  className,
  title,
}: {
  /** Value of the selected item; anything not in `items` shows `placeholder`. */
  value: string;
  items: DropdownItem[];
  onChange: (value: string) => void;
  /** Accessible name — the control carries no visible <label>. */
  label: string;
  placeholder?: string;
  emptyLabel?: string;
  /** Destructive last row. Rendered only when `onClear` is given. */
  clearLabel?: string;
  onClear?: () => void;
  disabled?: boolean;
  className?: string;
  title?: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const id = useId();

  const selectedIndex = items.findIndex((i) => i.value === value);
  const selected = selectedIndex >= 0 ? items[selectedIndex] : undefined;
  // The clear row is the last stop of the roving highlight, so it is reachable
  // by keyboard and not just by mouse.
  const rowCount = items.length + (onClear ? 1 : 0);
  const clearIndex = onClear ? items.length : -1;

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    setActive(-1);
    if (refocus) trigger.current?.focus();
  }, []);

  // A click on the app behind an absolutely-positioned panel never reaches it,
  // so closing on outside interaction has to be watched for at the document.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) close(false);
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, [open, close]);

  function pick(index: number) {
    if (index === clearIndex) {
      onClear?.();
    } else {
      const item = items[index];
      if (!item) return;
      onChange(item.value);
    }
    close(true);
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (disabled) return;
    if (e.key === "Escape") {
      // Swallowed while open, or the app's global Escape would also fire and
      // close the modal this dropdown sits in.
      if (open) {
        e.stopPropagation();
        close(true);
      }
      return;
    }
    if (e.key === "Tab") {
      if (open) close(false);
      return;
    }
    const arrow = ARROWS[e.key];
    if (arrow) {
      e.preventDefault();
      if (!open) {
        setOpen(true);
        setActive(selectedIndex >= 0 ? selectedIndex : nextIndex(arrow, -1, rowCount));
      } else {
        setActive((a) => nextIndex(arrow, a < 0 ? selectedIndex : a, rowCount));
      }
      return;
    }
    if (open && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      if (active >= 0) pick(active);
    }
  }

  return (
    <div className={`dd ${open ? "open" : ""} ${className ?? ""}`} ref={root} onKeyDown={onKeyDown}>
      <button
        ref={trigger}
        type="button"
        className="dd-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={label}
        aria-controls={open ? `${id}-list` : undefined}
        aria-activedescendant={open && active >= 0 ? `${id}-opt-${active}` : undefined}
        disabled={disabled}
        title={title}
        onClick={() => {
          setActive(open ? -1 : selectedIndex);
          setOpen(!open);
        }}
      >
        <span className="dd-label">{selected ? selected.label : placeholder ?? ""}</span>
        <span className="dd-chev"><Icon name="chevron-down" size={14} /></span>
      </button>

      {open && (
        <div className="dd-panel" id={`${id}-list`} role="listbox" aria-label={label}>
          {items.map((it, i) => (
            <div
              key={it.value}
              id={`${id}-opt-${i}`}
              role="option"
              aria-selected={it.value === value}
              className={`dd-opt ${it.value === value ? "sel" : ""} ${i === active ? "active" : ""}`}
              // Keep focus on the trigger so the panel keeps its keyboard
              // handling right through a mouse hover.
              onPointerDown={(e) => e.preventDefault()}
              onMouseEnter={() => setActive(i)}
              onClick={() => pick(i)}
            >
              {it.icon && <span className="dd-icon"><Icon name={it.icon} size={14} /></span>}
              <span className="dd-opt-label">{it.label}</span>
              {it.value === value && <span className="dd-check"><Icon name="check" size={14} /></span>}
            </div>
          ))}

          {items.length === 0 && <div className="dd-empty">{emptyLabel ?? "Nothing to choose"}</div>}

          {onClear && (
            <>
              <div className="dd-sep" role="presentation" />
              <div
                id={`${id}-opt-${clearIndex}`}
                role="option"
                aria-selected={false}
                className={`dd-clear ${active === clearIndex ? "active" : ""}`}
                onPointerDown={(e) => e.preventDefault()}
                onMouseEnter={() => setActive(clearIndex)}
                onClick={() => pick(clearIndex)}
              >
                <span className="dd-icon"><Icon name="x" size={14} /></span>
                <span className="dd-opt-label">{clearLabel ?? "Clear filter"}</span>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
