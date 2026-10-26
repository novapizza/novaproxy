/* The NovaProxy app icon, as the Nova Suite icon family draws it (canvas board
   04, the same art as NovaSuite.AppIcon and the OS icon from assets/logo):
   a near-black tile shared by every suite app, the NovaProxy green as a corner
   glow and hairline, the wire in green 300 and the N in the app gradient.
   Inline rather than an <img> so it stays crisp at every size and needs no
   asset request. The tile is dark in both themes, like the Dock icon. */
import { useId } from "react";

const TILE = "#13111A";
const C300 = "#9BE3B4";
const C500 = "#3EB56D";
const STAR =
  "M0,-1 C.1,-.29 .29,-.1 1,0 .29,.1 .1,.29 0,1 -.1,.29 -.29,.1 -1,0 -.29,-.1 -.1,-.29 0,-1 Z";

export function Brandmark({
  size = 36,
  className,
  title = "NovaProxy",
}: {
  size?: number;
  className?: string;
  /** Pass null when the icon sits next to the app's name and is decorative. */
  title?: string | null;
}) {
  // Two of these on one page would otherwise fight over the ids.
  const id = `np-${useId().replace(/:/g, "")}`;
  return (
    <svg
      viewBox="80 80 864 864"
      width={size}
      height={size}
      className={className}
      role={title ? "img" : undefined}
      aria-label={title ?? undefined}
      aria-hidden={title ? undefined : true}
    >
      <defs>
        <linearGradient id={`${id}n`} gradientUnits="userSpaceOnUse" x1="380" y1="290" x2="650" y2="740">
          <stop offset="0" stopColor="#DCFCE7" />
          <stop offset=".5" stopColor="#6EDB9A" />
          <stop offset="1" stopColor="#2E9A59" />
        </linearGradient>
        <clipPath id={`${id}c`}>
          <rect x="100" y="100" width="824" height="824" rx="190" />
        </clipPath>
        <filter id={`${id}g`} x="-60%" y="-60%" width="220%" height="220%">
          <feGaussianBlur stdDeviation="70" />
        </filter>
        <filter id={`${id}b`} x="-50%" y="-50%" width="200%" height="200%">
          <feGaussianBlur stdDeviation="20" />
        </filter>
      </defs>
      <rect x="100" y="100" width="824" height="824" rx="190" fill={TILE} />
      <g clipPath={`url(#${id}c)`}>
        <circle cx="330" cy="280" r="300" fill={C500} opacity=".45" filter={`url(#${id}g)`} />
      </g>
      <rect
        x="100" y="100" width="824" height="824" rx="190"
        fill="none" stroke={C300} strokeOpacity=".55" strokeWidth="10"
      />
      <g
        transform="translate(512 515) scale(.88) translate(-512 -515)"
        fill="none" stroke={C300} strokeLinecap="round" opacity=".85"
      >
        <path d="M188 515L340 515" strokeWidth="34" />
        <path d="M684 515L836 515" strokeWidth="34" />
        <circle cx="150" cy="515" r="38" strokeWidth="26" />
        <circle cx="874" cy="515" r="38" strokeWidth="26" />
      </g>
      <path
        d="M418 700L418 330L606 700L606 330"
        fill="none" stroke={`url(#${id}n)`} strokeWidth="80"
        strokeLinecap="round" strokeLinejoin="round"
      />
      <circle cx="668" cy="262" r="56" fill={C300} opacity=".5" filter={`url(#${id}b)`} />
      <path transform="translate(668 262) scale(58)" fill="#FFFFFF" d={STAR} />
    </svg>
  );
}
