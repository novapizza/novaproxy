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
  },
  build: {
    target: "es2021",
    outDir: "dist",
  },
});
