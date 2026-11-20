import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri-style config: fixed port, no screen clearing (keeps Rust errors visible),
// Safari 15 target (WKWebView on macOS).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "safari15",
    sourcemap: false,
    chunkSizeWarningLimit: 300,
  },
});
