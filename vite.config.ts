import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import pkg from "./package.json" with { type: "json" };

// Tauri drives this dev server; keep the port fixed and don't clobber Rust logs.
export default defineConfig({
  plugins: [react()],
  // The app's own version, for What's New. `package.json` is one of the files
  // the release bumps, so this cannot drift from the installer.
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: "127.0.0.1",
    // The Rust side is not the frontend's business, and `target/` runs to
    // tens of thousands of files. Watching it starves the dev server on
    // Windows: every request queues behind the watcher's file-system calls and
    // the window stays blank.
    watch: {
      ignored: ["**/target/**", "**/src-tauri/**", "**/crates/**"],
    },
  },
  build: {
    target: "es2021",
    outDir: "dist",
  },
});
