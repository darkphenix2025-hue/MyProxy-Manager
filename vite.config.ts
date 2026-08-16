import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  build: {
    rollupOptions: {
      output: {
        // Keep third-party dependencies out of the application entry chunk.
        // Split the large, stable dependency families and locale resources without
        // producing hundreds of tiny (or empty) package chunks.
        manualChunks(id) {
          if (id.includes("/src/locales/")) {
            const locale = id.split("/src/locales/")[1]?.replace(/\.json$/, "");
            return locale ? `locale-${locale}` : undefined;
          }

          if (!id.includes("node_modules")) return undefined;

          const packagePath = id.split("node_modules/")[1];
          const packageParts = packagePath.split("/");
          const packageName = packagePath.startsWith("@")
            ? packageParts.slice(0, 2).join("-")
            : packageParts[0];

          if (packageName === "react" || packageName === "react-dom" || packageName === "react-router" || packageName === "react-router-dom") {
            return "vendor-react";
          }
          if (packageName === "antd" || packageName.startsWith("rc-")) {
            return "vendor-antd";
          }
          if (packageName === "recharts" || packageName.startsWith("d3-")) {
            return "vendor-charts";
          }
          if (packageName === "framer-motion") return "vendor-motion";
          return undefined;
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "0.0.0.0",
    hmr: host
      ? {
        protocol: "ws",
        host,
        port: 1421,
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
