//! 插件创建工具 - create_plugin
//!
//! 运行时插件创造的执行侧（创造模式）：把一组能力贡献（技能 / 可执行工具 /
//! MCP server 声明 / 供应商预设）打包为一个完整插件，校验通过后原子落盘并
//! 立即装载——技能与工具下一轮即可用，MCP server 立即连接。
//!
//! 与其他能力沉淀工具的分工：
//! - `create_skill`：单条提示词级知识
//! - `create_tool`：单个可执行原语
//! - `create_plugin`：**一组能力的打包分发单元**（能被整体装载/卸载/删除）
//!
//! 设计要点（对齐 update_provider_preset 的结构化传统）：
//! - **写不出坏数据**：全部校验先于落盘（名字、schema、脚本黑名单、
//!   MCP 声明、预设行），临时目录写满后才替换正式目录
//! - **内置插件禁改**：llm-providers / plugin-authoring 是播种体系所有，
//!   覆盖它们会被升级播种冲掉；改内置的正确方式是复制为新插件
//! - **装载即生效**：落盘后走 `plugins::load_one`（先卸旧贡献再装新），
//!   与设置页「重载」按钮同一条路径
//! - **授权**：High risk + 用户预览卡片（MCP 声明是 shell 级敏感点）

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::plugins::ProviderPresetData;
use crate::skills::SkillService;
use crate::tools::custom_tools::CustomToolDef;
use crate::tools::mcp::McpServerConfig;
use crate::tools::registry::ToolSystem;
use crate::tools::McpManager;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// create_plugin 工具 - 打包一组能力贡献为完整插件并立即装载。
pub struct CreatePluginTool {
    skill_service: Arc<SkillService>,
    mcp_manager: Arc<McpManager>,
    tool_system: Arc<ToolSystem>,
}

impl CreatePluginTool {
    pub fn new(
        skill_service: Arc<SkillService>,
        mcp_manager: Arc<McpManager>,
        tool_system: Arc<ToolSystem>,
    ) -> Self {
        Self { skill_service, mcp_manager, tool_system }
    }
}

/// 从工具入参解析技能文件列表：[{filename, content}]。
fn parse_skills(input: &Value) -> Result<Vec<(String, String)>, String> {
    let Some(arr) = input.get("skills").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    arr.iter()
        .map(|s| {
            let filename = s
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let content = s
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if filename.is_empty() {
                return Err("技能条目缺 filename（如 \"my_skill.md\"）".into());
            }
            if content.trim().is_empty() {
                return Err(format!("技能 {filename} 的 content 为空"));
            }
            Ok((filename, content))
        })
        .collect()
}

/// 从工具入参解析工具定义列表：[{name, description, parameters?, script, deferred?}]。
fn parse_tools(input: &Value) -> Result<Vec<CustomToolDef>, String> {
    let Some(arr) = input.get("tools").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    arr.iter()
        .map(|t| {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let description = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let script = t
                .get("script")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if name.is_empty() {
                return Err("工具条目缺 name".into());
            }
            if description.is_empty() {
                return Err(format!("工具 {name} 缺 description"));
            }
            if script.is_empty() {
                return Err(format!("工具 {name} 缺 script"));
            }
            Ok(CustomToolDef {
                name,
                description,
                parameters: t.get("parameters").cloned().unwrap_or(Value::Null),
                script,
                deferred: t.get("deferred").and_then(|v| v.as_bool()).unwrap_or(false),
                created_at: crate::memory::types::current_timestamp(),
            })
        })
        .collect()
}

/// 从工具入参解析 MCP server 声明列表（字段对齐 McpServerConfig）。
fn parse_mcp_servers(input: &Value) -> Result<Vec<McpServerConfig>, String> {
    let Some(arr) = input.get("mcpServers").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    arr.iter()
        .map(|m| {
            let id = m
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let command = m
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if id.is_empty() {
                return Err("MCP server 条目缺 id".into());
            }
            if command.is_empty() {
                return Err(format!("MCP server {id} 缺 command"));
            }
            Ok(McpServerConfig {
                name: m
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&id)
                    .to_string(),
                id,
                transport: "stdio".into(),
                command,
                args: m
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                env: m
                    .get("env")
                    .and_then(|v| v.as_object())
                    .map(|o| {
                        o.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default(),
                cwd: m.get("cwd").and_then(|v| v.as_str()).map(String::from),
                enabled: m.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                source_plugin: None, // 落盘后由 merge_plugin_servers 写入归属
            })
        })
        .collect()
}

/// 从工具入参解析供应商预设列表（camelCase 对齐 ProviderPresetData）。
fn parse_providers(input: &Value) -> Result<Vec<ProviderPresetData>, String> {
    let Some(arr) = input.get("providers").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    arr.iter()
        .map(|p| {
            let id = p
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let provider_type = p
                .get("providerType")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if id.is_empty() {
                return Err("预设条目缺 id".into());
            }
            if provider_type.is_empty() {
                return Err(format!("预设 {id} 缺 providerType"));
            }
            let protocols = p.get("protocols").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .map(|x| crate::plugins::ProviderProtocolData {
                        provider_type: x
                            .get("providerType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        label_key: x
                            .get("labelKey")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        label: x.get("label").and_then(|v| v.as_str()).map(String::from),
                        endpoint: x
                            .get("endpoint")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect::<Vec<_>>()
            });
            Ok(ProviderPresetData {
                id,
                label_key: p.get("labelKey").and_then(|v| v.as_str()).map(String::from),
                label: p.get("label").and_then(|v| v.as_str()).map(String::from),
                provider_type,
                endpoint: p
                    .get("endpoint")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                default_model: p
                    .get("defaultModel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                main_models: p
                    .get("mainModels")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                context_window: p.get("contextWindow").and_then(|v| v.as_u64()).map(|v| v as u32),
                suggested_max_tokens: p
                    .get("suggestedMaxTokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
                needs_secret: p.get("needsSecret").and_then(|v| v.as_bool()),
                needs_app_id: p.get("needsAppId").and_then(|v| v.as_bool()),
                console_url: p.get("consoleUrl").and_then(|v| v.as_str()).map(String::from),
                protocols,
                verified_at: None,
                verified_source: p.get("verifiedSource").and_then(|v| v.as_str()).map(String::from),
            })
        })
        .collect()
}

#[async_trait]
impl Tool for CreatePluginTool {
    fn name(&self) -> &str {
        "create_plugin"
    }

    fn description(&self) -> &str {
        "Package a set of capability contributions into a complete plugin, persist it atomically \
         and load it immediately (skills usable next turn, MCP servers connected right away). \
         A plugin bundles four contribution types: skills (markdown prompt knowledge), tools \
         (PowerShell-backed executable tools, same format as create_tool), mcpServers (stdio \
         MCP server declarations), providers (LLM provider presets). Use this when the user \
         wants a coherent, uninstallable capability bundle instead of a single skill or tool. \
         Semantics: same plugin name = update (old contributions unloaded, replaced whole); \
         built-in plugins (llm-providers, plugin-authoring) are read-only — copy them into a \
         new plugin instead. Creation always requires user approval via preview card, which \
         shows MCP commands and tool scripts in full."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "把一组能力贡献打包为完整插件：校验通过后原子落盘并立即装载\
            （技能与工具下一轮可用，MCP server 立即连接）。插件是四类贡献的打包分发单元：\
            skills（markdown 提示词知识）、tools（PowerShell 可执行工具，格式同 create_tool）、\
            mcpServers（stdio MCP server 声明）、providers（LLM 供应商预设）。\
            当用户要的是可整体装载/卸载/删除的能力包而非单条技能或单个工具时使用。\
            语义：同名插件 = 更新（旧贡献整体卸载后替换）；内置插件（llm-providers、\
            plugin-authoring）禁止覆盖——要改内置的，复制成新插件再改。\
            创建始终需要用户通过预览卡片授权（MCP 命令行与工具脚本全文可见）。",
            "ja" => "一連の能力貢献を完全なプラグインとしてパッケージ化する：検証通過後 \
            アトミックに保存し即座にロード（スキルとツールは次ターンから利用可能、MCP \
            サーバーは即時接続）。プラグインは 4 種の貢献の配布単位：skills（markdown \
            プロンプト知識）、tools（PowerShell 実行ツール、create_tool と同形式）、\
            mcpServers（stdio MCP サーバー宣言）、providers（LLM プロバイダ プリセット）。\
            単一のスキルやツールではなく、全体としてロード/アンロード/削除可能な能力 \
            バンドルが必要な場合に使う。同名プラグイン = 更新（旧貢献を全体アンロード後 \
            置換）。内蔵プラグイン（llm-providers、plugin-authoring）の上書きは禁止——\
            内蔵を変えたい場合は新プラグインとしてコピーしてから編集。作成には常に \
            プレビューカードによるユーザー承認が必要（MCP コマンドラインとツール \
            スクリプト全文が表示される）。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "做个插件\n打包成一个插件\n把这些能力做成插件",
            "en" => "create a plugin\nmake a plugin bundling these\npackage as a plugin",
            "ja" => "プラグインを作って\nこれらをプラグインにまとめて",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Plugin id / directory name: ASCII letters/digits/'-'/'_' only (e.g. 'weather-suite'); becomes the skill namespace prefix. Same name = update that plugin"
                },
                "version": {
                    "type": "string",
                    "description": "Semver-ish version string, e.g. \"1.0.0\"; bump it on updates"
                },
                "description": {
                    "type": "string",
                    "description": "What the plugin bundles (shown in the plugin inventory)"
                },
                "skills": {
                    "type": "array",
                    "description": "Prompt-level knowledge files; filename uses letters/digits/'-'/'_' (+ optional .md), content is markdown with optional front-matter (name/description)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "filename": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["filename", "content"]
                    }
                },
                "tools": {
                    "type": "array",
                    "description": "Executable tools, same contract as create_tool (stdin JSON args / stdout result, 120s timeout); names must not collide with built-in tools",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "parameters": { "type": "object", "description": "JSON Schema (type: object)" },
                            "script": { "type": "string" },
                            "deferred": { "type": "boolean" }
                        },
                        "required": ["name", "description", "script"]
                    }
                },
                "mcpServers": {
                    "type": "array",
                    "description": "stdio MCP server declarations; each becomes a real spawned process — the preview card shows every command line for user review. Existing user-configured server ids are NOT overridden",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name": { "type": "string" },
                            "command": { "type": "string" },
                            "args": { "type": "array", "items": { "type": "string" } },
                            "env": { "type": "object" },
                            "cwd": { "type": "string" },
                            "enabled": { "type": "boolean" }
                        },
                        "required": ["id", "command"]
                    }
                },
                "providers": {
                    "type": "array",
                    "description": "LLM provider presets (same semantics as update_provider_preset rows); field names camelCase (id, providerType, endpoint, defaultModel, mainModels, ...)",
                    "items": { "type": "object" }
                }
            },
            "required": ["name", "version", "description"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "插件 id / 目录名：仅 ASCII 字母/数字/-/_（如 'weather-suite'）；同时是技能命名空间前缀。同名 = 更新该插件"
                    },
                    "version": {
                        "type": "string",
                        "description": "版本号（如 \"1.0.0\"）；更新时应递增"
                    },
                    "description": {
                        "type": "string",
                        "description": "插件打包了什么（显示在插件清单中）"
                    },
                    "skills": {
                        "type": "array",
                        "description": "提示词级知识文件；filename 仅用字母/数字/-/_（可带 .md），content 为 markdown（支持 front-matter name/description）",
                        "items": {
                            "type": "object",
                            "properties": {
                                "filename": { "type": "string" },
                                "content": { "type": "string" }
                            },
                            "required": ["filename", "content"]
                        }
                    },
                    "tools": {
                        "type": "array",
                        "description": "可执行工具，契约同 create_tool（stdin 收 JSON 参数 / stdout 出结果，120 秒超时）；工具名不可与内置工具冲突",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "description": { "type": "string" },
                                "parameters": { "type": "object", "description": "JSON Schema（type: object）" },
                                "script": { "type": "string" },
                                "deferred": { "type": "boolean" }
                            },
                            "required": ["name", "description", "script"]
                        }
                    },
                    "mcpServers": {
                        "type": "array",
                        "description": "stdio MCP server 声明；每条都会真实拉起进程——预览卡片会完整展示每条命令行供用户审核。已存在的用户手配 server id 不会被覆盖",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "name": { "type": "string" },
                                "command": { "type": "string" },
                                "args": { "type": "array", "items": { "type": "string" } },
                                "env": { "type": "object" },
                                "cwd": { "type": "string" },
                                "enabled": { "type": "boolean" }
                            },
                            "required": ["id", "command"]
                        }
                    },
                    "providers": {
                        "type": "array",
                        "description": "LLM 供应商预设（语义同 update_provider_preset 的行）；字段名 camelCase（id、providerType、endpoint、defaultModel、mainModels、…）",
                        "items": { "type": "object" }
                    }
                },
                "required": ["name", "version", "description"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let name = input.get("name").and_then(|v| v.as_str()).unwrap_or_default().trim();
        let version = input.get("version").and_then(|v| v.as_str()).unwrap_or_default().trim();
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            return ValidationResult::failure("name 是必填项", 2);
        }
        if version.is_empty() {
            return ValidationResult::failure("version 是必填项", 2);
        }
        if description.is_empty() {
            return ValidationResult::failure("description 是必填项", 2);
        }
        // 结构性预检（深度校验在落盘函数统一执行，这里拦住明显的形状错误）
        for field in ["skills", "tools", "mcpServers", "providers"] {
            if let Some(v) = input.get(field) {
                if !v.is_array() {
                    return ValidationResult::failure(&format!("{field} 必须是数组"), 2);
                }
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 强制用户确认：MCP 声明会拉起真实进程、工具脚本是可执行代码，
        // 预览卡片须完整展示命令行与脚本后由用户决定。
        PermissionResult::ask("创建插件需要用户授权：预览卡片将展示全部 MCP 命令行与工具脚本")
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let version = args
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();

        // 用户已通过预览卡片授权（executor 权限门放行后才进入这里）
        let draft = crate::plugins::PluginDraft {
            name,
            version,
            description,
            skills: match parse_skills(&args) {
                Ok(v) => v,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            },
            tools: match parse_tools(&args) {
                Ok(v) => v,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            },
            mcp_servers: match parse_mcp_servers(&args) {
                Ok(v) => v,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            },
            providers: match parse_providers(&args) {
                Ok(v) => v,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            },
        };

        // 1. 原子落盘（全量校验内建）
        let dir = match crate::plugins::write_plugin_files(&draft) {
            Ok(d) => d,
            Err(e) => return ToolResult::standard_error(&e, None, None),
        };
        // 2. 立即装载（含卸载旧贡献 + 连接新增 MCP server）。
        //    authorized=true：本工具走 High risk + 用户预览卡片授权，装载即写入
        //    信任指纹（用户已在卡片上确认过插件内容，无需再到设置页信任一次）
        let report = match crate::plugins::load_one(
            &self.skill_service,
            &self.mcp_manager,
            &self.tool_system,
            &draft.name,
            true,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return ToolResult::standard_error(
                    &format!("插件已落盘但装载失败: {e}（可在设置 → 插件页排查后重载）"),
                    None,
                    None,
                )
            }
        };

        ToolResult::standard_success(
            &format!(
                "插件「{}」v{} 已创建并装载（{}）：{} 条技能、{} 个工具、{} 个 MCP server、{} 条供应商预设。\
                 技能与工具现在就可用；用户可在设置 → 插件页重载或删除。",
                draft.name,
                args.get("version").and_then(|v| v.as_str()).unwrap_or(""),
                dir.display(),
                report.skills.len(),
                report.tools.len(),
                report.mcp_servers.len(),
                draft.providers.len()
            ),
            Some(json!({
                "name": draft.name,
                "dir": dir.display().to_string(),
                "skills": report.skills,
                "tools": report.tools,
                "mcp_servers": report.mcp_servers,
                "providers": draft.providers.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        // 落盘可执行脚本 + 拉起 MCP 进程，与 Shell 同级管控
        ToolRiskTier::Shell
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "create plugin bundle package capabilities 插件 打包 能力包 create_plugin"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "沉淀单条方法论（改用 create_skill）",
            "创建单个可执行工具（改用 create_tool）",
            "修改内置插件 llm-providers（复制为新插件再改）",
        ]
    }
}
