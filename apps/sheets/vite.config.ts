import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In the browser the UI talks to the dev bridge (sheets_dev_server) through /api;
// inside Tauri it calls the Rust backend directly.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    proxy: { "/api": "http://127.0.0.1:7878" },
  },
});
