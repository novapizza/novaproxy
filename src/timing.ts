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
  /** When the last request body byte went upstream, from the start of the exchange. */
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
 * `Waiting` is time-to-first-byte with connection setup (and any upload that
 * outlasted it, as `Sending request`) subtracted, because TTFB is measured from
 * the start of the exchange and would otherwise double-count the phases
 * sitting inside it.
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

  // The part of the upload that ran past connection setup: the server could
  // not answer before it had the whole request. `request_ms` is when the last
  // body byte left, counted from the start, so it overlaps the setup above.
  if (t?.ttfb_ms != null && t.request_ms != null && t.request_ms > at) {
    push("send", "Sending request", Math.min(t.request_ms, t.ttfb_ms) - at, "var(--c-pink)");
  }
  const wait = serverWaitOf(t);
  if (wait != null) push("wait", "Waiting (server)", wait, "var(--c-amber)");
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
 * Server wait above which a row gets the turtle — Firefox's threshold
 * (`devtools.netmonitor.audits.slow`), so the mark means what people already
 * read it as.
 */
export const SLOW_WAIT_MS = 500;

/**
 * How long the server took to start answering: the `Waiting (server)` phase of
 * the waterfall, so the turtle and the Timing panel quote the same number.
 *
 * `null` when it cannot be blamed on the server — not measured, or the flow sat
 * at a breakpoint, where the wait is the person editing it.
 */
export function serverWaitMs(flow: Flow): number | null {
  if (flow.edits?.breakpoint) return null;
  return serverWaitOf(flow.timings);
}

/**
 * Time to first byte less what came before the server could answer:
 * connection setup, and the request body still going up. The engine's
 * `nova_core::timing::server_wait_ms` is the same definition — keep them in
 * step. Network Conditions latency needs no subtracting: the engine stamps TTFB
 * before it adds that delay.
 */
function serverWaitOf(t: Flow["timings"] | null | undefined): number | null {
  if (t?.ttfb_ms == null) return null;
  const setup = (t.dns_ms ?? 0) + (t.connect_ms ?? 0) + (t.tls_ms ?? 0);
  const sent = Math.max(t.request_ms ?? 0, setup);
  return Math.max(t.ttfb_ms - sent, 0);
}

/** The server wait, when it is slow enough to mark. */
export function slowWaitMs(flow: Flow): number | null {
  const wait = serverWaitMs(flow);
  return wait != null && wait > SLOW_WAIT_MS ? wait : null;
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
