# Nana 旧图集校色

已修复左右行走、生气、思考、得意，以及 wake、busy-in、busy-loop、cast、tend-out、tend、tired，共十二张图集。前五张的正常首尾帧保持原样，新增七张按默认状态的浅色材质四分位数校正全部动作帧，兼顾中间色与高光，修正偏暗灰和动作切换时闪色。

以 `public/chibi/nana-atlas.webp` 的默认状态为颜色基准。既有生成图提供校色参考，新图集复用参考并直接采样默认图，无需重新绘制动作。最终用逐通道单调颜色映射处理原像素；保留全部原始透明通道、动作轮廓、512 px 格位、正常首尾帧和暗色描边。无损 WebP 编码可能改变完全透明像素中不可见的 RGB。

原图备份为 `nana-*-original.webp`，生成参考及提示记录在本目录。备份、参考和预览不进入安装包。

重建：`node assets/chibi/color-correction/build.mjs`。
备份原图只在首次创建时写入，重复校色不会累积提亮。
验证：`node tests/chibi-palette.test.mjs`。
只读审查：`node scripts/audit-chibi-palette.mjs`，输出 `tmp/chibi-palette/audit.json`。

`build-report.json` 记录各帧前后颜色均值和映射参数。`previews/nana-walk-before-after.gif` 左侧为原图，右侧为校色后的同一帧。
