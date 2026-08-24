/* The NovaProxy mark, per design.md §Brandmark. Inline rather than an <img> so
   the gradient runs on the same tokens as the rest of the UI — a brandmark that
   drifted from the palette would be the one place the tokens do not reach.
   The rail renders it at 32px, where the logo's own rule drops the wire and the
   arrow: below ~40px they mush together, so only the nova node survives.
   Full lockup (node + wire + arrow) and every raster size live in assets/logo. */
import { useId } from "react";

export function Brandmark({ size = 32, className }: { size?: number; className?: string }) {
  // Two of these on one page would otherwise fight over the gradient id.
  const gradient = `${useId()}-mk`;
  return (
    <svg
      viewBox="0 0 1024 1024"
      width={size}
      height={size}
      className={className}
      role="img"
      aria-label="NovaProxy"
    >
      <defs>
        <linearGradient id={gradient} gradientUnits="objectBoundingBox" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="var(--c-indigo)" />
          <stop offset=".42" stopColor="var(--c-cyan)" />
          <stop offset="1" stopColor="var(--accent-hover)" />
        </linearGradient>
      </defs>
      <path
        d="M512 83.2Q587.4 413.4 840 512Q587.4 610.6 512 940.8Q436.6 610.6 184 512Q436.6 413.4 512 83.2Z"
        fill={`url(#${gradient})`}
      />
    </svg>
  );
}
