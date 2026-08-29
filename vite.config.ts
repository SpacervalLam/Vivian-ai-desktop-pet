import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { rmSync } from "node:fs";
import { resolve } from "node:path";

const host = process.env.TAURI_DEV_HOST;

/**
 * 构建后从 dist 中删除明文美术资源目录。
 *
 * public/{Vivian,Nana,world-bg} 会被 vite 默认拷贝到 dist，但这些资源
 * 已通过 scripts/encrypt-assets.mjs 打包进加密的 vivian.bundle.enc，
 * 生产运行时统一走 http://model.localhost 自定义协议解密加载。
 * dist 保留明文副本既冗余又破坏加密意义，构建产物中必须清除。
 * dev 模式不受影响（vite dev server 直接服务 public/ 原始文件）。
 */
function stripEncryptedAssets(): Plugin {
  const ENCRYPTED_DIRS = ["Vivian", "Nana", "world-bg"];
  return {
    name: "strip-encrypted-assets",
    apply: "build",
    closeBundle() {
      // vite 从项目根目录启动，cwd 即项目根（ESM 下无 __dirname）
      for (const dir of ENCRYPTED_DIRS) {
        rmSync(resolve(process.cwd(), "dist", dir), { recursive: true, force: true });
      }
      console.log(
        `[strip-encrypted-assets] 已从 dist 移除明文资源目录: ${ENCRYPTED_DIRS.join(", ")}`,
      );
    },
  };
}

export default defineConfig(async () => ({
  plugins: [react(), stripEncryptedAssets()],
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
