import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1450,
    strictPort: true,
    host: host || "0.0.0.0",
    hmr: host
      ? {
        protocol: "ws",
        host,
        port: 1451,
      }
      : undefined,
    watch: {
      // tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
    proxy: {
      "/api/": {
        target: process.env.VITE_API_PROXY_URL || "http://127.0.0.1:8150",
        changeOrigin: true,
      },
    },
  },
}));
