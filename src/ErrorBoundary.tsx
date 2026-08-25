/**
 * The last thing standing between a render error and a blank window.
 *
 * `App.tsx` is one large view tree, so a throw anywhere in it unmounts the
 * whole app. Before this existed that left a white rectangle and no record —
 * the webview console is not attached to anything in a bundled build, so the
 * error went nowhere a user or a support thread could reach.
 *
 * The fallback deliberately offers a reload rather than trying to recover in
 * place: the store, the streaming channel and the engine subscription are all
 * set up on mount, and half of them surviving a crash is worse than none.
 */

import React from "react";
import { api } from "./api";

interface Props {
  children: React.ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // The component stack is the part worth having: a minified message alone
    // rarely says which panel died.
    api.logUi(
      "error",
      "render",
      `${error.message}\n${error.stack ?? ""}\ncomponentStack:${info.componentStack ?? ""}`,
    );
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="crash">
        <div className="crash-card">
          <h1>NovaProxy hit an error</h1>
          <p>
            The window stopped rendering. What happened has been written to the log; reloading
            starts a fresh session, and anything captured in this one is lost.
          </p>
          <pre className="crash-detail">{error.message}</pre>
          <button className="btn-primary" onClick={() => window.location.reload()}>
            Reload
          </button>
        </div>
      </div>
    );
  }
}

/**
 * Catch what React cannot: errors thrown outside the render tree, and promises
 * rejected with nobody listening.
 *
 * Registered once, from `main.tsx`, before the app mounts.
 */
export function installGlobalErrorHandlers(): void {
  window.addEventListener("error", (e) => {
    api.logUi("error", "window-error", `${e.message} at ${e.filename}:${e.lineno}:${e.colno}`);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const reason = e.reason;
    const text =
      reason instanceof Error ? `${reason.message}\n${reason.stack ?? ""}` : String(reason);
    api.logUi("error", "unhandled-rejection", text);
  });
}
