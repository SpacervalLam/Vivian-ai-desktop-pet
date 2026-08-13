import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    // WebView2 为常青 Chromium，无需兼容旧浏览器
    target: "es2022",
    chunkSizeWarningLimit: 1000,
    rollupOptions: {
      output: {
        // 按稳定 vendor 分组拆包，便于多 Tauri 窗口之间共享缓存、并行加载：
        // - pixi：仅主窗口 Live2D 用到，拆成独立 chunk，其他窗口无需解析
        // - react/tauri/i18n：高频稳定依赖，各自独立缓存
        // - 其余依赖保持 Vite 默认拆包，异步 chunk 按需加载
        manualChunks(id) {
          if (!id.includes("node_modules")) return;
          if (id.includes("/pixi") || id.includes("@pixi/")) return "pixi";
          if (id.includes("@tauri-apps")) return "tauri";
          if (
            id.includes("/react/") ||
            id.includes("/react-dom/") ||
            id.includes("/react-is/") ||
            id.includes("/scheduler/") ||
            id.includes("/zustand/")
          ) {
            return "react";
          }
          if (id.includes("/i18next/") || id.includes("/react-i18next/")) {
            return "i18n";
          }
          // 其余依赖（含 echarts/mermaid 等动态 import 的库）保持 Vite 默认拆包，
          // 让异步 chunk 按需加载，不合并成单一巨型 vendor 包。
        },
      },
    },
  },
}));
