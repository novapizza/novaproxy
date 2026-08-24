/* The NovaProxy mark, per design.md §Brandmark. Inline rather than an <img> so
   the gradient runs on the same tokens as the rest of the UI — a brandmark that
   drifted from the palette would be the one place the tokens do not reach.
   The rail renders it at 32px, where the logo's own rule swaps in the simplified
   drawing: the hollow end rings bleed shut and the star vanishes below ~40px, so
   the wire becomes a solid tick and the N grows. Below 24px the wire goes too.
   Full lockup and every raster size live in assets/logo. */
import { useId } from "react";

/* Simplified geometry, 1024 grid — mirrors C_* in assets/logo/build.py. */
const N_PATH = "M404 724L404 300L620 724L620 300";
const WIRE = ["M154 515L330 515", "M694 515L870 515"];
const STAR =
  "M0,-1 C.1,-.29 .29,-.1 1,0 .29,.1 .1,.29 0,1 -.1,.29 -.29,.1 -1,0 -.29,-.1 -.1,-.29 0,-1 Z";
const BOX_WITH_WIRE = "131 99 762 762";
const BOX_NODE_ONLY = "249 190 580 580";

export function Brandmark({
  size = 32,
  className,
}: {
  size?: number;
  className?: string;
}) {
  // Two of these on one page would otherwise fight over the gradient id.
  const gradient = `${useId()}-mk`;
  const wire = size >= 24;
  return (
    <svg
      viewBox={wire ? BOX_WITH_WIRE : BOX_NODE_ONLY}
      width={size}
      height={size}
      className={className}
      role="img"
      aria-label="NovaProxy"
    >
      <defs>
        <linearGradient
          id={gradient}
          gradientUnits="userSpaceOnUse"
          x1="358"
          y1="254"
          x2="666"
          y2="770"
        >
          <stop offset="0" stopColor="var(--accent)" />
          <stop offset="1" stopColor="var(--accent-hover)" />
        </linearGradient>
      </defs>
      {wire &&
        WIRE.map((d) => (
          <path
            key={d}
            d={d}
            fill="none"
            stroke="var(--accent)"
            strokeWidth={46}
            strokeLinecap="round"
            opacity={0.62}
          />
        ))}
      <path
        d={N_PATH}
        fill="none"
        stroke={`url(#${gradient})`}
        strokeWidth={92}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d={STAR}
        transform="translate(654,256) scale(66)"
        fill="var(--accent-hover)"
      />
    </svg>
  );
}
