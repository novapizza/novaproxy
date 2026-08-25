import type { Flow } from "./bindings/Flow";

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

export function formatRate(bytesPerSecond: number): string {
  const bits = bytesPerSecond * 8;
  if (bits >= 1e9) return `${(bits / 1e9).toFixed(1)} Gb/s`;
  if (bits >= 1e6) return `${(bits / 1e6).toFixed(1)} Mb/s`;
  if (bits >= 1e3) return `${(bits / 1e3).toFixed(1)} kb/s`;
  return `${Math.round(bits)} b/s`;
}
