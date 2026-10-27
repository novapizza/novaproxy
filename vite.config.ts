import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri drives this dev server; keep the port fixed and don't clobber Rust logs.
export default defineConfig({
  plugins: [react()],
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
