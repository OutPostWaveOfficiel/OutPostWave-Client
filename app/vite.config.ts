import { defineConfig } from "vite";

// Vite config tuned for Tauri: fixed port, no screen clearing, modern targets.
export default defineConfig({
  root: "ui",
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: false,
  },
});
