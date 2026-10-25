//! 生图工具（`generate_image`）。
//!
//! 闭环是关键：生成结果**必须落盘到工作区**，并且图片随本次工具结果一起回传，
//! 这样模型下一步就能用 `read_image`（或直接看工具结果里的图）验收自己画了什么。
//! 少了任何一半，都会退化成"生成了但没验收"——等于闭着眼睛交付。
//!
//! 参考图入参让「拿已批准的形象派生新图」成为可能：这是 sprite atlas、
//! style transfer 这类工作的前提，纯文生图做不了。

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::brain::coding_agent::CodingImage;
use crate::network::image_alpha;
use crate::network::image_gen::{self, ImageGenRequest};
use crate::tools::sandbox::is_path_safe;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 参考图数量与体积上限：再多就不是"参考"而是"要我合并一堆素材"了。
const MAX_REFERENCES: usize = 4;
const MAX_REFERENCE_BYTES: usize = 8 * 1024 * 1024;

fn resolve_path(raw: &str, ctx: &ToolUseContext) -> String {
    let p = std::path::Path::new(raw);
    if p.is_absolute() || ctx.working_directory.trim().is_empty() {
        raw.to_string()
    } else {
        std::path::Path::new(&ctx.working_directory)
            .join(p)
            .to_string_lossy()
            .to_string()
    }
}

/// 目标已存在时改成交叉版本名（xxx-v2.png）。
///
/// 生图的结果常要反复迭代：直接覆盖意味着上一版无论好坏都没了，
/// 而"对比两版再决定留哪个"是这类工作最自然的节奏。
fn unique_path(path: String) -> String {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return path;
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    for i in 2..=99 {
        let cand = if ext.is_empty() {
            parent.join(format!("{stem}-v{i}"))
        } else {
            parent.join(format!("{stem}-v{i}.{ext}"))
        };
        if !cand.exists() {
            return cand.to_string_lossy().to_string();
        }
    }
    path
}

fn sniff_mime(bytes: &[u8]) -> String {
    image_gen::sniff_mime(bytes)
}

/// 按 mime 选扩展名（落盘时用真实类型，不信任后端声明）。
fn ext_for(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

pub struct GenerateImageTool;

impl GenerateImageTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GenerateImageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        "Generate an image from a text prompt, optionally guided by reference images (up to 4), and save it into \
         the workspace. The generated image is returned with this tool result, so you can look at it immediately \
         and iterate.\n\
         Reference images carry design intent (character sheet, palette, layout) that words cannot — use them \
         instead of restating the design in the prompt. Backends differ: /images/edits for OpenAI-compatible \
         endpoints, image[] for Seedream; if references are rejected, the error says so.\n\
         Prompting: tag the request with use_case to set the right level of augmentation; if your prompt is already \
         specific, only normalize it — do not invent characters, brands or palettes. Quote text that must appear \
         verbatim. quality=low for drafts, high for final or text-dense assets. transparent=true for cutouts.\n\
         Iterate one change at a time and re-check; when editing, restate the invariants (change only X, keep Y). \
         Output never overwrites an existing file — a versioned sibling is written instead. \
         The output path must be inside the workspace: an image written elsewhere cannot be read back or presented."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按文字描述生成图片，可用参考图（最多 4 张）约束风格/形象，并把结果**落盘到工作区**。\
                     生成的图随本次工具结果一起回传，你可以立刻看到并迭代。\n\
                     需要沿用已有设计（角色设定、画风、版式）时一定要传 reference_images——纯文字传不了这些。\
                     参考图支持取决于后端：OpenAI 兼容走 /images/edits，Seedream 走 image[]；\
                     后端不支持时错误信息里会直说。\n\
                     提示词写法：用 use_case 标归类，让它决定该补多少细节；提示词已经写得很具体时只做规范化，\
                     不要自行添加角色、品牌、配色。要求画面里出现的文字必须逐字写进 prompt。\
                     quality=low 用于草稿与快速试错，文字密集或最终成品用 high；要透明底传 transparent=true。\n\
                     迭代时**一次只改一处**再看效果；改已有图时把不变量写清楚（只改 X，保持 Y）。\
                     输出不会覆盖已存在文件，会写成新版本号（xxx-v2.png）。\n\
                     输出路径必须在工作区内：写到别处的图读不回来，也没法交付。",
            "ja" => "テキストから画像を生成し（参照画像を最大 4 枚まで併用可）、ワークスペースに保存する。\
                     生成画像はこのツール結果と一緒に返るので、すぐ見て反復できる。\n\
                     既存デザイン（キャラ設定・画風・レイアウト）を踏襲するなら reference_images を必ず渡す——\
                     テキストだけでは伝わらない。参照画像の対応はバックエンド次第（OpenAI 互換は /images/edits、\
                     Seedream は image[]）。非対応ならエラーに明記される。\n\
                     プロンプトの書き方：use_case で用途を標記し、補うべき詳細の粒度を決める。すでに具体的なら\
                     正規化のみ行い、キャラ・ブランド・配色を創作しない。画面に入れる文字は逐字書く。\
                     quality=low は下書き、文字が密な最終稿は high。透過にしたいなら transparent=true。\n\
                     反復は**一度に一箇所だけ**変えて確認。既存画像の編集では不変条件（X だけ変え、Y は維持）を書く。\
                     出力は既存ファイルを上書きせず、新しい版番号（xxx-v2.png）で保存される。\n\
                     出力パスはワークスペース内であること。外に書いた画像は読み戻せず、納品もできない。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "画一张图\n生成一个图标\n按这个风格再画一张\n做个 sprite",
            "en" => "draw an image\ngenerate an icon\nmake another one in this style\ncreate a sprite",
            "ja" => "画像を作って\nアイコンを生成\nこの画風でもう一枚\nスプライトを作る",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "画面描述。写清主体、构图、配色、风格；要沿用的设定放 reference_images 而不是堆在文字里" },
                "reference_images": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "参考图路径（最多 4 张，单张 ≤ 8 MiB）。用于风格/形象/版式继承"
                },
                "output_path": {
                    "type": "string",
                    "description": "保存路径（绝对路径，或相对工作目录）。不传则存到工作区 generated/ 下"
                },
                "size": { "type": "string", "description": "尺寸，如 1024x1024 / 1792x1024；不传用配置默认值" },
                "quality": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "auto"],
                    "description": "质量档。草稿/缩略图/快速试错用 low；成品、密集文字、身份敏感用 high"
                },
                "use_case": {
                    "type": "string",
                    "description": "用途归类，例如 photorealistic-natural / product-mockup / ui-mockup / illustration-story / stylized-concept / background-extraction / style-transfer / compositing / sketch-to-render。只用来决定补充多少细节，不会新增创意要求"
                },
                "transparent": {
                    "type": "boolean",
                    "description": "要透明背景。后端支持就原生出 alpha；不支持则铺纯色键控底并在本地抠成透明（仅 PNG 能保留 alpha）"
                },
                "avoid": { "type": "string", "description": "明确不要出现的内容（如 no text, no watermark, no extra characters）" }
            },
            "required": ["prompt"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("").trim();
        if prompt.is_empty() {
            return ValidationResult::failure("prompt 不能为空", 2);
        }
        if let Some(refs) = input.get("reference_images").and_then(|v| v.as_array()) {
            if refs.len() > MAX_REFERENCES {
                return ValidationResult::failure(
                    &format!("参考图最多 {MAX_REFERENCES} 张（现在 {} 张）", refs.len()),
                    2,
                );
            }
            for r in refs {
                let Some(p) = r.as_str() else {
                    return ValidationResult::failure("reference_images 必须是字符串路径数组", 2);
                };
                let resolved = resolve_path(p.trim(), ctx);
                if !is_path_safe(&resolved) {
                    return ValidationResult::failure("参考图路径包含穿越序列（..）", 2);
                }
            }
        }
        if let Some(out) = input.get("output_path").and_then(|v| v.as_str()) {
            let resolved = resolve_path(out.trim(), ctx);
            if !is_path_safe(&resolved) {
                return ValidationResult::failure("输出路径包含穿越序列（..）", 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let raw_prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let size = args.get("size").and_then(|v| v.as_str()).map(|s| s.to_string());
        let use_case = args.get("use_case").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let avoid = args.get("avoid").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let transparent = args.get("transparent").and_then(|v| v.as_bool()).unwrap_or(false);
        let quality = args
            .get("quality")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|q| matches!(q.as_str(), "low" | "medium" | "high" | "auto"));

        let Some((cfg, proxy)) = image_gen::read_runtime() else {
            return ToolResult::standard_error(
                "读不到生图路由：设置 → 任务模型分配 → 图片生成 这条路由不存在。\
                 请重启应用让路由目录重新播种，或手动在 routing_matrix 里补一条 image_gen。",
                Some("ImageGenUnavailable"),
                None,
            );
        };

        // ── 提示词整型 ──
        // 归类只用于决定"补多少"，不给模型追加创意的许可：提示词已经很具体时只做规范化，
        // 写得笼统才补与用途强相关的关键信息。补得多不等于画得对，反而更难复现。
        let mut prompt = raw_prompt.clone();
        if !use_case.is_empty() {
            prompt = format!("Use case: {use_case}\nPrimary request: {prompt}");
        }
        if transparent && cfg.backend == "seedream" {
            // 该后端没有真透明参数：铺一块纯色键控底，生成后本地转 alpha。
            // 不指定键色的话，模型给的白底/灰底在主体里也会出现，抠出来就是一片洞。
            prompt.push_str("\nBackground: one flat solid pure green (#00FF00) fill, no gradient, \
                             no shadow, no ground plane; subject fully inside the frame with clear margin.");
        }
        if !avoid.is_empty() {
            prompt.push_str(&format!("\nAvoid: {avoid}"));
        }

        // 参考图：先全部读进来再发请求——半路失败会浪费一次生成配额
        let mut references: Vec<(String, Vec<u8>)> = Vec::new();
        if let Some(refs) = args.get("reference_images").and_then(|v| v.as_array()) {
            for r in refs {
                let Some(p) = r.as_str() else { continue };
                let path = resolve_path(p.trim(), ctx);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        if bytes.len() > MAX_REFERENCE_BYTES {
                            return ToolResult::standard_error(
                                &format!("参考图过大：{path}（{} MiB，上限 8 MiB）", bytes.len() / 1_048_576),
                                Some("InvalidInput"),
                                None,
                            );
                        }
                        let mime = sniff_mime(&bytes);
                        references.push((mime, bytes));
                    }
                    Err(e) => {
                        return ToolResult::standard_error(
                            &format!("读不到参考图 {path}：{e}。先用 glob / list_dir 确认路径。"),
                            Some("NotFound"),
                            None,
                        );
                    }
                }
            }
        }

        // 输出路径：默认工作区 generated/ 下，带时间戳避免互相覆盖
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let requested = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(|s| resolve_path(s.trim(), ctx));
        let output_path = match requested {
            Some(p) => p,
            None => {
                if ctx.working_directory.trim().is_empty() {
                    return ToolResult::standard_error(
                        "本会话没有工作区，无法确定落盘位置。请传 output_path 绝对路径。",
                        Some("InvalidInput"),
                        None,
                    );
                }
                std::path::Path::new(&ctx.working_directory)
                    .join("generated")
                    .join(format!("image-{stamp}.png"))
                    .to_string_lossy()
                    .to_string()
            }
        };
        if !ctx.is_path_writable(&output_path) {
            return ToolResult::standard_error(
                &format!(
                    "输出路径不在授权工作区内：{output_path}。生图结果必须落在工作区里才能被读回与交付；\
                     需要其它目录请让用户在工作页挂为附加工作区。"
                ),
                Some("PermissionDenied"),
                None,
            );
        }

        // 用户点名要替换某个文件时才允许覆盖，否则给个新版本号
        let output_path = unique_path(output_path);

        let quality_echo = quality.clone();
        let req = ImageGenRequest {
            prompt: prompt.clone(),
            references,
            size,
            quality: quality.clone(),
            transparent,
        };
        let images = match image_gen::generate(&cfg, Some(&proxy), &req).await {
            Ok(v) => v,
            Err(e) => {
                return ToolResult::standard_error(
                    &format!("生图失败：{e}"),
                    Some("ImageGenError"),
                    None,
                )
            }
        };

        // ── 透明收尾 ──
        // 原生出 alpha 的模型直接用；不支持的走"纯色键控底 + 本地抠底"。
        // 抠不出来的情况要说清楚（比如返回的是 JPEG，或者四角颜色不一致），
        // 别把一张带死底边的图当成透明素材交付。
        let mut notes: Vec<String> = Vec::new();
        let mut images = images;
        if transparent {
            for (i, img) in images.iter_mut().enumerate() {
                if img.media_type != "image/png" {
                    notes.push(format!(
                        "#{i} 是 {}，JPEG/WebP 无法保留透明通道；要透明请让后端输出 PNG",
                        img.media_type
                    ));
                    continue;
                }
                if image_alpha::has_alpha(&img.bytes) {
                    continue;
                }
                match image_alpha::cutout_png(&img.bytes, None) {
                    Ok((out, key)) => {
                        notes.push(format!(
                            "#{i} 已把背景色 rgb({},{},{}) 转为透明",
                            key.0, key.1, key.2
                        ));
                        img.bytes = out;
                    }
                    Err(e) => notes.push(format!("#{i} 抠底失败：{e}")),
                }
            }
        }

        // 落盘：多张时加序号后缀
        let mut written: Vec<String> = Vec::new();
        for (i, img) in images.iter().enumerate() {
            let mut path = std::path::PathBuf::from(&output_path);
            if i > 0 {
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                path.set_file_name(format!("{stem}-{i}.{}", ext_for(&img.media_type)));
            } else if path.extension().is_none() {
                path.set_extension(ext_for(&img.media_type));
            }
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return ToolResult::standard_error(
                            &format!("创建输出目录失败：{e}"),
                            None,
                            None,
                        );
                    }
                }
            }
            if let Err(e) = std::fs::write(&path, &img.bytes) {
                return ToolResult::standard_error(&format!("写入 {path:?} 失败：{e}"), None, None);
            }
            written.push(path.to_string_lossy().to_string());
        }

        let revised = images.first().and_then(|i| i.revised_prompt.clone());
        let mut text = format!(
            "已生成 {} 张图并写入：{}\n（后端 {}／模型 {}",
            images.len(),
            written.join("、"),
            cfg.backend,
            cfg.model,
        );
        if !use_case.is_empty() {
            text.push_str(&format!("；用途 {}", use_case));
        }
        if let Some(q) = &quality_echo {
            text.push_str(&format!("；质量 {q}"));
        }
        if transparent {
            text.push_str(if notes.is_empty() { "；透明：已保留" } else { "；透明：见下" });
        }
        text.push_str(&format!(
            "）\n提示词：{}",
            prompt.chars().take(160).collect::<String>()
        ));
        if let Some(r) = &revised {
            text.push_str(&format!("\n后端改写后的提示词：{r}"));
        }
        for n in &notes {
            text.push_str(&format!("\n[{n}]"));
        }
        text.push_str("\n图片已随本条结果送达：直接看画面判断是否达标。迭代时一次只改一处（构图/配色/细节），改完再看；要沿用某张图的设定就用 reference_images，别用文字复述。");

        let mut result = ToolResult::standard_success(
            &text,
            Some(json!({
                "paths": written,
                "backend": cfg.backend,
                "model": cfg.model,
                "revised_prompt": revised,
                "reference_count": req.references.len(),
                "use_case": use_case,
                "quality": quality_echo,
                "transparent": transparent,
                "transparency_notes": notes,
            })),
        );
        // 把首张图挂到结果上，模型下一步就能"看见"它
        if let Some(first) = images.first() {
            result.images = Some(vec![CodingImage {
                media_type: first.media_type.clone(),
                data: base64::engine::general_purpose::STANDARD.encode(&first.bytes),
                name: std::path::Path::new(&written[0])
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string()),
            }]);
        }
        result
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::File
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }

    fn execution_timeout(&self, args: &Value) -> Option<std::time::Duration> {
        // 生图比普通工具慢得多：给到配置上限之外再留余量，
        // 否则外层 120 秒会把正在等的请求掐掉（和 run_command 同一个坑）。
        let configured = image_gen::read_runtime()
            .map(|(c, _)| c.timeout_secs.clamp(20, 600))
            .unwrap_or(180);
        let _ = args;
        Some(std::time::Duration::from_secs(configured + 20))
    }

    fn search_hint(&self) -> &str {
        "generate image draw picture art sprite 生图 画图 生成图片 素材 参考图"
    }
}
