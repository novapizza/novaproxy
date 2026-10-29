/**
 * Development only: turn off React 19's "performance tracks".
 *
 * To label the browser's Performance panel, React deep-diffs the changed props
 * of every component it renders. The flow list is a prop thousands of objects
 * long, so under live capture that diffing took most of the main thread and
 * most of the garbage: a dev build froze long before a release build would, and
 * measured nothing like one. React enables the tracks only if `console.timeStamp`
 * exists when react-dom is evaluated, which is why this is a module of its own,
 * imported before it. Set VITE_REACT_PERF_TRACKS=1 to keep them for a
 * profiling session.
 */
if (import.meta.env.DEV && !import.meta.env.VITE_REACT_PERF_TRACKS) {
  (console as { timeStamp?: unknown }).timeStamp = undefined;
}

export {};
