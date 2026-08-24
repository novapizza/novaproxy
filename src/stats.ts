import type { Flow } from "./bindings/Flow";

/**
 * Numbers behind the stat strip above the flow list. Pure so the arithmetic —
 * which median to pick, what counts as failed — is pinned by tests rather than
 * re-derived inline in the render.
 */
export interface FlowStats {
  /** Rows currently passing the filter. */
  visible: number;
  /** Rows captured in total, filter ignored. */
  total: number;
  /** Median duration over *completed* flows; null while nothing has finished. */
  medianMs: number | null;
  /** 4xx, 5xx and transport errors — everything the user would call a failure. */
  failed: number;
  /** Flows carrying Model Context Protocol traffic. */
  mcp: number;
}

/** Lower median on an even count, so the value is always one a flow really had. */
function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor((sorted.length - 1) / 2)];
}

export function flowStats(all: Flow[], visible: Flow[]): FlowStats {
  const durations: number[] = [];
  let failed = 0;
  let mcp = 0;
  for (const f of all) {
    if (f.duration_ms != null) durations.push(f.duration_ms);
    if (f.error != null || (f.status != null && f.status >= 400)) failed++;
    if (f.mcp != null) mcp++;
  }
  return {
    visible: visible.length,
    total: all.length,
    medianMs: median(durations),
    failed,
    mcp,
  };
}

export const SPARK_BUCKETS = 24;
export const SPARK_WINDOW_MS = 60_000;

/**
 * Response bytes per bucket over the trailing window, oldest bucket first.
 * Flows are attributed to the instant they started — the proxy does not record
 * when each byte landed, so this is a rate estimate, not a wire trace.
 */
export function throughputSeries(
  flows: Flow[],
  now: number,
  buckets = SPARK_BUCKETS,
  windowMs = SPARK_WINDOW_MS,
): number[] {
  const series = new Array<number>(buckets).fill(0);
  if (buckets <= 0 || windowMs <= 0) return series;
  const span = windowMs / buckets;
  const start = now - windowMs;
  for (const f of flows) {
    if (f.started_at < start || f.started_at > now) continue;
    const i = Math.min(buckets - 1, Math.floor((f.started_at - start) / span));
    series[i] += Number(f.response_size);
  }
  return series;
}

/** Bytes per second across the whole window, for the card's headline figure. */
export function throughputRate(series: number[], windowMs = SPARK_WINDOW_MS): number {
  if (windowMs <= 0) return 0;
  const total = series.reduce((a, b) => a + b, 0);
  return (total / windowMs) * 1000;
}

export interface SparkPath {
  /** Polyline through every point. */
  line: string;
  /** The same polyline closed along the baseline, for the gradient fill. */
  area: string;
  /** Where to park the leading dot. */
  last: { x: number; y: number };
}

/**
 * Map a series onto an SVG path in a `w`x`h` box. A flat series (all zero, or
 * all equal) sits on the baseline rather than dividing by a zero range.
 */
export function sparkPath(series: number[], w: number, h: number): SparkPath {
  const n = series.length;
  if (n === 0) return { line: "", area: "", last: { x: w, y: h } };
  const peak = Math.max(...series);
  const step = n === 1 ? 0 : w / (n - 1);
  const points = series.map((v, i) => ({
    x: n === 1 ? w : i * step,
    y: peak === 0 ? h : h - (v / peak) * h,
  }));
  const line = points.map((p, i) => `${i === 0 ? "M" : "L"}${round(p.x)} ${round(p.y)}`).join(" ");
  const first = points[0];
  const last = points[points.length - 1];
  const area = `${line} L${round(last.x)} ${round(h)} L${round(first.x)} ${round(h)} Z`;
  return { line, area, last: { x: round(last.x), y: round(last.y) } };
}

const round = (n: number) => Math.round(n * 100) / 100;

/** Compact byte-rate label for the throughput card. */
export function formatRate(bytesPerSecond: number): string {
  const bits = bytesPerSecond * 8;
  if (bits >= 1e9) return `${(bits / 1e9).toFixed(1)} Gb/s`;
  if (bits >= 1e6) return `${(bits / 1e6).toFixed(1)} Mb/s`;
  if (bits >= 1e3) return `${(bits / 1e3).toFixed(1)} kb/s`;
  return `${Math.round(bits)} b/s`;
}
