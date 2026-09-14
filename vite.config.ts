import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { resolve, join, dirname, basename, relative } from "node:path";
import { cpSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";

const host = process.env.TAURI_DEV_HOST;

/**
 * 注意：Vite 5 的配置查找顺序里 `vite.config.js` 排在 `vite.config.ts` 前面。
 * 一旦被 `tsc` 编译出同名的 `.js`，后续所有构建都会静默改用那份副本，本文件的
 * 改动全部失效且毫无提示。项目已把 tsconfig.node.json 设为只做类型检查、不产出
 * 文件；若发现改动不生效，先确认仓库根目录没有多出 vite.config.js。
 */

/**
 * 构建期按白名单复制明文资源。
 *
 * public 下的 Vivian / Nana / world-bg 元数据已由资源加密步骤打包进
 * vivian.bundle.enc（现作为预构建产物随仓库提供，原生成脚本 scripts/encrypt-assets.mjs 已移除）。chibi 图集是主窗口实际使用的 sprite 纹理，需要随前端静态资源发布。
 *
 * 这里用「显式清单 + copyPublicDir 关闭」而不是「先全量复制再删除」：删除式清理
 * 一旦失效（子进程缺 rm、异常被吞）就会静默留下整套明文，包体翻倍且加密形同虚设。
 *
 * 白名单条目的路径必须对得上实际目录。条目失效时不能静默跳过——曾经因为目录改名
 * 后条目未同步，图集一路静默缺失到发布会，装完只见透明窗口。所以这里显式告警。
 */
interface KeepEntry {
  /** public 下的相对路径，可以是文件或目录。 */
  path: string;
  /**
   * 文件名过滤器，仅对目录生效。
   *
   * 桌宠的素材目录里同时躺着运行时雪碧图和制图中间产物（逐帧原图、制图源），
   * 前者要进包、后者全仓零引用，只能按文件名区分。
   */
  match?: RegExp;
}

function copyPublicAssets(): Plugin {
  const KEEP: KeepEntry[] = [
    { path: "room" },
    // 主窗口待机/高兴等姿态使用 chibi/*-atlas.webp（地址见 ChibiPetCanvas.css 的 --atlas-url）。
    { path: "chibi/vivian-atlas.webp" },
    { path: "chibi/nana-atlas.webp" },
    // 走动/转身/眨眼/表情动作由 ChibiPetCanvas 内联引用这两处的雪碧图。同目录下的
    // 逐帧序列与 walk/source 只是制图时的中间产物，运行时一次都不会加载。
    { path: "chibi/walk", match: /-sheet\.webp$/ },
    { path: "chibi/motion", match: /-sheet\.webp$/ },
    { path: "fonts" },
    { path: "icons" },
    { path: "favicon.ico" },
  ];

  /** 统计目录下命中过滤器的文件数，用于确认过滤器没有把整个目录筛空。 */
  const countMatched = (dir: string, match: RegExp, skipped: (path: string) => boolean): number => {
    let total = 0;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (skipped(full)) continue;
      if (entry.isDirectory()) total += countMatched(full, match, skipped);
      else if (match.test(entry.name)) total += 1;
    }
    return total;
  };

  return {
    name: "copy-public-assets",
    apply: "build",
    closeBundle() {
      const publicDir = resolve(process.cwd(), "public");
      const outDir = resolve(process.cwd(), "dist");
      const missing: string[] = [];
      const empty: string[] = [];
      for (const entry of KEEP) {
        const src = join(publicDir, entry.path);
        if (!existsSync(src)) {
          missing.push(entry.path);
          continue;
        }
        // 制图源目录（如 chibi/walk/source）里躺着同名图集的生成中间产物，
        // 只看文件名的话会连它们一起打包，得按目录段排除。
        const skipped = (candidate: string) =>
          relative(src, candidate).split(/[\\/]/).includes("source");
        if (entry.match && statSync(src).isDirectory() && countMatched(src, entry.match, skipped) === 0) {
          empty.push(`${entry.path} (${entry.match})`);
          continue;
        }
        const destination = join(outDir, entry.path);
        mkdirSync(dirname(destination), { recursive: true });
        cpSync(src, destination, {
          recursive: true,
          filter: entry.match
            ? (source) => {
                if (skipped(source)) return false;
                if (statSync(source).isDirectory()) return true;
                return entry.match!.test(basename(source));
              }
            : undefined,
        });
      }
      console.log(`[copy-public-assets] 已复制明文资源: ${KEEP.length - missing.length - empty.length}/${KEEP.length}`);
      if (missing.length > 0) {
        console.warn(
          `[copy-public-assets] 白名单条目在 public 下不存在，已跳过: ${missing.join(", ")}\n` +
            `  若资源目录已改名或删除，请同步更新 KEEP，否则运行时会静默 404。`,
        );
      }
      if (empty.length > 0) {
        console.warn(
          `[copy-public-assets] 白名单过滤器未命中任何文件，已跳过: ${empty.join(", ")}\n` +
            `  素材命名规则变了却没同步过滤器时，图集会整体缺失且构建零报错。`,
        );
      }
    },
  };
}

export default defineConfig(async () => ({
  plugins: [react(), copyPublicAssets()],
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
    // 关闭默认的全量 public 拷贝，明文资源改由 copy-public-assets 按白名单复制
    copyPublicDir: false,
    // dist 是纯构建产物目录：chunk 名带内容 hash，旧版本不会被同名覆盖，
    // 不清空就会随构建轮次持续累积并原样进安装包
    emptyOutDir: true,
    // WebView2 为常青 Chromium，无需兼容旧浏览器
    target: "es2022",
    chunkSizeWarningLimit: 1000,
    rollupOptions: {
      output: {
        // 按稳定 vendor 分组拆包，便于多 Tauri 窗口之间共享缓存、并行加载：
        // - react/tauri/i18n：高频稳定依赖，各自独立缓存
        // - 其余依赖保持 Vite 默认拆包，异步 chunk 按需加载
        manualChunks(id) {
          if (!id.includes("node_modules")) return;
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
          // 其余依赖（含 mermaid 等动态 import 的库）保持 Vite 默认拆包，
          // 让异步 chunk 按需加载，不合并成单一巨型 vendor 包。
        },
      },
    },
  },
}));
