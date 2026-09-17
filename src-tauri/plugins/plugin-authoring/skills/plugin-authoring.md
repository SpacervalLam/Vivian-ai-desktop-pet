---
name: plugin-authoring
description: 当要创建或修改 vivian 插件、维护 LLM/嵌入 Provider 预设或排查插件装载失败时使用。覆盖五类贡献点、Provider CRUD、命名空间、防影子化与内置插件保护。
---

# 插件创作

插件是**一组能力的打包分发单元**：能被整体装载、卸载、删除。一个插件目录（`<用户数据目录>/plugins/<name>/`）持有：

- `plugin.json` —— 清单（name / version / description + 四类贡献点声明）
- `skills/*.md` —— 提示词级知识（front-matter 可选 name/description）
- `tools/*.json` —— 可执行工具定义（格式同自建工具）
- `providers.json` —— LLM 供应商预设数组（可选）
- `embedding-providers.json` —— 云端嵌入供应商预设数组（可选）

## 何时用哪个沉淀工具

| 场景 | 工具 |
|---|---|
| 一条可复用的方法论 | `create_skill` |
| 一个可执行原语 | `create_tool` |
| **一组相关能力**（要整体装卸/分享） | `create_plugin` |
| 修正供应商预设数据 | `update_provider_preset` |
| 盘点 LLM/嵌入预设 | `list_provider_presets` |
| 新增、更新或删除 LLM/嵌入预设 | `manage_provider_preset` |
| 删除整个自建插件 | 委派工作智能体调用 `delete_plugin`（需要用户确认） |

判断口诀：用户要的是"一个能力"还是"一套能力"。单个 PowerShell 工具能解决的不要打包成插件；打包的动机是**内聚**——技能文档教怎么用、工具执行、MCP 提供外部数据，三者围绕同一主题。

## create_plugin 的五类贡献点

**skills**：`[{filename, content}]`。文件名即技能名（去 .md），仅字母/数字/-/_；注册后命名空间为 `<插件名>/<技能名>`，与用户技能天然不冲突。正文是注入 prompt 的指引，写"该做什么/怎么做"，不要写实现代码。

**tools**：`[{name, description, parameters, script, deferred}]`，契约与 create_tool 完全一致——stdin 收 JSON 参数（`$args = [Console]::In.ReadToEnd() | ConvertFrom-Json`）、stdout 出结果、120 秒超时、输出 8000 字符截断。工具名不可与内置/自建/其他插件工具重名（防影子化，重名装载时被跳过）。每次调用都走用户确认（Shell 级）。

**mcpServers**：`[{id, name, command, args, env, cwd, enabled}]`。每条声明会真实拉起一个进程——这是 shell 级敏感点，创建预览卡片会展示完整命令行。用户手配的同 id server 优先，插件不覆盖。

**providers**：供应商预设行（字段 camelCase：id / providerType / endpoint / defaultModel / mainModels / contextWindow / consoleUrl / protocols…）。同 id 按插件目录名字典序先到保留（内置 llm-providers 的预设 id 受保护，其他插件同 id 声明会被跳过）。

**embeddings**：云端嵌入预设行（字段 camelCase：id / provider / endpoint / model / dimension / recommendedFor / verifiedSource）。端点应为 base URL，由运行时统一补 `/embeddings`；不在插件中保存 API Key。

维护内置 `llm-providers` 的单条预设时不要调用 `create_plugin`：先 `list_provider_presets`，核对官方文档后使用 `manage_provider_preset`。陪伴智能体可直接完成这类结构化维护；创建或整体替换自建插件则委派工作智能体调用 `create_plugin`，删除整个自建插件委派其调用 `delete_plugin`。后两者都必须由用户确认。

## 硬性规则

1. **内置插件禁改禁删**：`llm-providers`、`plugin-authoring` 是播种体系所有，升级会覆盖手工修改，且改坏 `plugin-authoring` 会禁用插件创作本身。要改内置的：创建新名字的插件，把内容复制过去再改。
2. **同名 = 整体替换更新**：更新插件时旧贡献（技能/工具/MCP）全部卸载后装新的。删除的工具定义会真的消失——更新时要传**完整的**贡献列表，不是增量。
3. **版本号递增**：更新时 version 应大于旧值，便于用户在插件清单页辨认新旧。
4. **命名**：插件名/技能文件名/工具名/MCP id 都仅允许 ASCII 字母/数字/-/_。插件名同时是技能命名空间前缀。
5. **校验是拒绝而非降级**：任何贡献点不合法（schema 非 object、脚本含破坏性片段、id 非法）整个创建失败，错误信息会指明具体条目。

## 装载语义

- 落盘即装载：`create_plugin` 成功后技能/工具下一轮可用，MCP server 立即连接。失败会明确说"已落盘但装载失败"。
- 手工编辑插件目录（用户直接改文件）后：设置 → 插件页点「重载」，或重启应用。
- 卸载撤销运行时贡献但保留文件；删除两者都移除。

## 失败排查

- 装载失败提示"清单不可用"：plugin.json 解析失败——检查 JSON 语法与字段类型。
- 工具没出现在列表：大概率重名被跳过（防影子化），或脚本含黑名单片段。
- MCP server 没连接：看命令能否在终端手动跑通；用户手配的同 id server 会静默优先。
- 技能没出现：正文为空（只有 front-matter 没有正文）会被跳过。
