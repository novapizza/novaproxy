import React from "react";
import ReactDOM from "react-dom/client";
// Bundled locally so a desktop app keeps its typography offline. Variable
// weight axes cover the whole ramp in design.md from one file per subset.
import "@fontsource-variable/petrona";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import { App } from "./App";
import { ErrorBoundary, installGlobalErrorHandlers } from "./ErrorBoundary";
import "./styles.css";

// Before the app mounts, so a throw during the first render is still caught.
installGlobalErrorHandlers();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
