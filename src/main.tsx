import React from "react";
import ReactDOM from "react-dom/client";
// Bundled locally so a desktop app keeps its typography offline. Variable
// weight axes cover the whole ramp in design.md from one file per subset.
import "@fontsource-variable/petrona";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import { App } from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
