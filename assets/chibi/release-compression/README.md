# 发布图集压缩

当前评估 45 张动作图集，31 张采用压缩版本，14 张保留原文件。默认角色 atlas 保持原文件；Nana 的 12 张校色图集仍保留无损原图，单独维持至少 65% 的发布节省预算。

`npm run build` 在 Vite 白名单复制后运行 `scripts/compress-chibi.mjs`，始终读取 `public/chibi` 原图，依次尝试 WebP Q90、Q95、Q98，alphaQuality=100、effort=6。保留所有帧与画布尺寸，透明度逐像素不变；不透明 RGB RMSE 不超过 5/255，整体及浅色材质每通道平均偏移不超过 1/255。颜色候选不合格或节省不足 5% 时保留原文件；尺寸或透明度变化则终止构建。缓存按原图哈希、配置和编码器版本隔离，复用前重新校验像素。缓存、源图备份、报告不进入安装包。

随后字体和内置贴纸被移入独立可选 ZIP，动作图集全部保留在基础前端。发布检查：

```powershell
npm run build
node tests/chibi-release-compression.test.mjs --dist
node tests/chibi-palette.test.mjs
node tests/companion-assets.test.mjs
```

历史测量：扩大压缩范围后，基础前端 24,198,942 → 22,912,352 字节，主程序 85,319,168 → 84,022,272 字节，安装包 40,674,041 → 39,389,374 字节（38.79 → 37.56 MiB）。此后删除已退役功能及专用资源，当前体积以新生成的发布清单为准。

尝试记录：提高无损编码 effort 对 tend 只节省 5,144 字节、walk-left 38,198 字节；nearLossless Q80 改变了 tend 的 44,570 个透明度像素，已拒绝。缩小单帧分辨率及删帧未实施。
