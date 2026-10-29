import type { ProfilerOnRenderCallback } from "react";

/**
 * Render cost per `<Profiler>` id since the last perf line.
 *
 * React only calls `onRender` in development and profiling builds, so in a
 * release build the line simply has no render part — the counters cost nothing
 * there.
 */
const renders: Record<string, { n: number; ms: number; max: number }> = {};

export const onRender: ProfilerOnRenderCallback = (id, _phase, actualMs) => {
  const r = (renders[id] ??= { n: 0, ms: 0, max: 0 });
  r.n += 1;
  r.ms += actualMs;
  r.max = Math.max(r.max, actualMs);
};

/** `id=renders/total ms/max ms` for each profiled part, then reset. */
export function takeRenderTimes(): string {
  const parts = Object.entries(renders).map(
    ([id, r]) => `${id}=${r.n}/${Math.round(r.ms)}ms/${Math.round(r.max)}ms`,
  );
  for (const id of Object.keys(renders)) delete renders[id];
  return parts.join(" ");
}
