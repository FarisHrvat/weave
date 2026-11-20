import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// noVNC uses top-level await; it is loaded only when a VNC screen opens.
const supported = { "top-level-await": true };

export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: { host: "127.0.0.1", port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  esbuild: { supported },
  optimizeDeps: { esbuildOptions: { supported } },
  build: { target: "safari13" },
});
