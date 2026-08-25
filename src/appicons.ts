import { api } from "./api";

/**
 * App icons, fetched once per app and kept for the session.
 *
 * The table asks for the icon of every row it paints, which for a scrolling
 * capture is thousands of calls for a handful of distinct apps — so the cache is
 * the point, not an optimisation. It lives at module scope rather than in React
 * state because it is not view state: the icon for "Google Chrome" does not
 * change when a component unmounts.
 *
 * Misses are cached too. An app with no bundle (a CLI tool, a daemon) has no
 * icon and must not be asked about again.
 */
const icons = new Map<string, string | null>();
const inflight = new Map<string, Promise<void>>();
const listeners = new Set<() => void>();

/** Subscribe to "some icon arrived", for components rendering from the cache. */
export function onIconsChanged(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/**
 * The icon for `name` if it is already known, `null` if there is none, and
 * `undefined` while nothing has been asked yet — which is also when this kicks
 * off the fetch. Callers render the fallback glyph for anything but a string.
 */
export function appIcon(name: string | null | undefined): string | null | undefined {
  if (!name) return null;
  if (icons.has(name)) return icons.get(name);
  if (!inflight.has(name)) {
    inflight.set(
      name,
      api
        .appIcon(name)
        .then((url) => {
          icons.set(name, url ?? null);
        })
        // A failed lookup caches as "no icon": retrying on every repaint would
        // be a request storm over a decoration.
        .catch(() => {
          icons.set(name, null);
        })
        .finally(() => {
          inflight.delete(name);
          for (const fn of listeners) fn();
        }),
    );
  }
  return undefined;
}

/** Test seam: forget everything fetched so far. */
export function resetIconCache() {
  icons.clear();
  inflight.clear();
}
