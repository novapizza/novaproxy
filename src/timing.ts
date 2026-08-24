import type { Flow } from "./api";

/**
 * One measured phase of an exchange, positioned for a waterfall bar.
 * `startMs` is the offset from the start of the exchange.
 */
export interface Phase {
  key: string;
  label: string;
  ms: number;
  startMs: number;
  color: string;
}

export interface TimingBreakdown {
  /** Phases in waterfall order; only phases that were actually measured. */
  phases: Phase[];
  /** Wall-clock total, when the flow completed. */
  totalMs: number | null;
  /** Span the bars are scaled against (total, or the sum of the phases). */
  spanMs: number;
  /** The request went out on an already-open connection. */
  reused: boolean;
  /** How long the request body took to stream upstream, when there was one. */
  requestMs: number | null;
  /** Nothing was measured — render an explanation instead of empty bars. */
  empty: boolean;
}

/**
 * Turn a flow's measured timings into waterfall phases.
 *
 * Only measured values become phases: a flow that reused a connection has no
 * DNS/connect/TLS phase, and a flow with no timings at all (a rule-blocked
 * request, a Map Local response, an imported session from an older build)
 * reports `empty` so the UI can say so rather than draw invented bars.
 *
 * `Waiting` is time-to-first-byte with connection setup subtracted, because TTFB
 * is measured from the start of the exchange and would otherwise double-count
 * the setup phases sitting inside it.
 */
export function timingBreakdown(flow: Flow): TimingBreakdown {
  const t = flow.timings;
  const totalMs = flow.duration_ms ?? null;
  const phases: Phase[] = [];
  let at = 0;
  const push = (key: string, label: string, ms: number | null | undefined, color: string) => {
    if (ms == null) return;
    phases.push({ key, label, ms, startMs: at, color });
    at += ms;
  };

  push("dns", "DNS lookup", t?.dns_ms, "var(--c-violet)");
  push("connect", "TCP connect", t?.connect_ms, "var(--c-blue)");
  push("tls", "TLS handshake", t?.tls_ms, "var(--c-cyan)");

  const setup = at;
  if (t?.ttfb_ms != null) {
    push("wait", "Waiting (server)", Math.max(t.ttfb_ms - setup, 0), "var(--c-amber)");
  }
  push("download", "Download", t?.download_ms, "var(--c-green)");

  const measured = at;
  return {
    phases,
    totalMs,
    spanMs: Math.max(totalMs ?? 0, measured, 0.001),
    reused: !!t?.connection_reused,
    requestMs: t?.request_ms ?? null,
    empty: phases.length === 0,
  };
}

/**
 * Millisecond value with no unit, for the places that print the unit
 * themselves. Durations arrive as f64 from the engine, so every one of them has
 * to pass through here — printed raw, a median of 248.46826171875 overruns the
 * card it sits in.
 */
export function formatDuration(ms: number): string {
  if (ms < 1) return ms.toFixed(2);
  if (ms < 100) return ms.toFixed(1);
  return `${Math.round(ms)}`;
}

/** Compact millisecond label: sub-millisecond values keep a decimal. */
export function formatMs(ms: number): string {
  return `${formatDuration(ms)}ms`;
}
