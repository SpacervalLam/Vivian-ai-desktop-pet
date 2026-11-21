// 预览专用配置：只做一件事——把依赖预构建缓存挪到独立目录。
// 原因：默认的 node_modules/.vite/deps 一旦需要重建，vite 会整目录删除，
// 而本机的删除守卫对超过 50 项的批量删除会直接拒绝，dev server 当场启动失败。
// 换一个空的新目录就不存在"删旧缓存"这一步。
import { defineConfig } from "vite";
import base from "./vite.config";

export default defineConfig(async (env) => {
  const resolved = typeof base === "function" ? await (base as any)(env) : base;
  return { ...resolved, cacheDir: "node_modules/.vite-preview" };
});
