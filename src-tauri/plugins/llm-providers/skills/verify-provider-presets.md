---
name: verify-provider-presets
description: 联网核对 LLM 与云端嵌入供应商官方 API 文档，并用 list_provider_presets / manage_provider_preset 新增、更新或删除过期预设，保证端点、协议、模型、维度和上下文参数与官方一致
---

# 联网核对供应商预设

本技能把「盘点当前预设 → 上网核对官方 API 文档 → 结构化新增、更新或删除预设」沉淀为可重复执行的流程，覆盖 LLM 与云端嵌入服务。

开始时先经 `tool_search` 加载 `list_provider_presets` 和 `manage_provider_preset`，再调用前者取得当前完整行。不得凭记忆直接覆盖或删除。

写入规则：

- `action=upsert, kind=llm`：提交完整 LLM 行；兼容旧流程的 `update_provider_preset` 仍可使用。
- `action=upsert, kind=embedding`：提交 `id/provider/endpoint/model/dimension`，并附 `verifiedSource`。
  这是**厂商级的单模型写入**：`id` 是厂商 id（如 `glm`/`openai`），厂商行已存在时只合并
  这一个模型，不会覆盖同厂商的其他模型。可选字段：`region`（地域）、`consoleUrl`、
  `dimensionParam`（`dimensions` / `output_dimension`，缺省=不下发维度）、
  `maxBatch`（单请求 input 条数上限）、`dimensions`（可调维度）、`needsApiKey`（本地服务传 false）、`note`。
- `action=delete`：仅在官方确认服务或模型已经退役、且当前 id 确实存在时使用；删除预设不会删除 API Key，也不会切换当前运行配置。
- 新服务使用新稳定 id；已有 id 不改名。
LLM 厂商迭代极快：模型名退役、上下文窗口翻倍、端点迁移、新协议上线，预设数据随时会过期。
执行本技能时**逐家核对、只信官方文档、逐字段修订**。

## 第 0 步：判定执行环境

检测 `src/components/ConfigWindow.tsx`（应用安装根/工作目录下）是否存在且可读：

- **存在 → 开发机完整路径**：核对后除插件数据外，还要同步源码兜底与文档（见第 3 步）
- **不存在 → 运行时路径**（打包安装版）：只做核对 + `update_provider_preset` 工具写入，
  结果中注明「内置兜底与源码分级未同步，属开发期待办」

同时扫描核对队列：先用 `list_provider_presets` 读取当前 LLM 与嵌入预设（经 tool_search 精确加载
工具或直接读插件数据文件），`verifiedAt` 距今超过 **30 天**或缺失的预设进入本次
核对队列；30 天内的跳过（除非用户点名要求核对某家）。

## 数据源（按优先级）

1. **插件 providers.json（主数据源，运行时生效）**
   `<用户数据目录>/plugins/llm-providers/providers.json`
   —— JSON 数组，每项一个供应商预设；前端设置 → LLM 页的厂商卡片按 `id` 与内置兜底合并，
   插件数据**按 id 覆盖**内置同名项、新 id 追加在「自定义」卡片之前。改完重开设置窗口即生效。
2. **前端内置兜底（仅开发机同步，防止插件缺失时数据过期）**
   `src/components/ConfigWindow.tsx` 的 `PROVIDER_PRESETS` 常量
3. **后端工作智能体输出预算（仅开发机，输出上限变化时同步）**
   `src-tauri/src/providers/factory.rs` 的 `work_model_default_max_tokens`（按端点域名分级）
4. **文档（收尾同步，仅开发机）**：`README.md` 的 suggestedMaxTokens 分级说明、`CODE_WIKI.md` 的输出预算表

## 字段核对清单（每家供应商逐项过）

| 字段                         | 核对要点                                                                                                                     | 常见过期形态                                         |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------- |
| `endpoint`                 | 官方文档「快速开始」的 base\_url 原文（注意是否带 `/v1`、是否区分国内外站）                                                                           | 端点迁移、国内/国际站分拆（如 moonshot.cn / moonshot.ai）     |
| `providerType`             | 该端点实际协议：`openai`=Responses、`chat_completions`=Chat Completions、`anthropic`/`gemini`/`zhipu`/`wenxin`/`doubao`/`spark`=原生 | 厂商新上线 Responses/Anthropic 兼容层后未跟进              |
| `protocols`                | 厂商支持的协议变体全集（≥2 个才显示协议切换器）；Anthropic 兼容端点通常为 `{base}/anthropic`                                                           | 新协议上线未补充；旧协议下线未移除                              |
| `defaultModel`             | 当前推荐的旗舰/主力模型 ID 原文（保持大小写与分隔符一致）                                                                                          | 默认模型退役后指向失效 ID                                 |
| `mainModels`               | 官方模型列表中**当前有效**的模型 ID；已宣布退役/停用的必须移除                                                                                      | 保留已停用模型名（如 deepseek-chat/reasoner 2026-07 已停用） |
| `contextWindow`            | 上下文窗口（输入+输出合计，tokens）；官方区分「最大输入/最大输出」时按合计口径                                                                            | 旧代际小窗口未更新（如 128K → 1M）                         |
| `suggestedMaxTokens`       | 建议单次输出上限；保守取值，宁小勿大（超限被 400 拒绝）                                                                                           | 与官方新输出上限脱节（如 8K 硬上限已移除）                        |
| `consoleUrl`               | 「获取 API Key」跳转页是否仍有效                                                                                                     | 控制台改版后 404                                     |
| `needsSecret`/`needsAppId` | 鉴权方式是否变化（OAuth/HMAC 型厂商）                                                                                                 | —                                              |

## 嵌入预设字段核对清单（`embedding-providers.json`，厂商级）

| 字段              | 核对要点                                                                                     | 常见过期形态                                        |
| --------------- | ---------------------------------------------------------------------------------------- | --------------------------------------------- |
| `endpoint`      | 官方「OpenAI 兼容」base\_url 原文，**不带** `/embeddings`（适配器会补；带上也能识别，但表单反查厂商预设时会归一化比较）               | 兼容层迁移（如百炼新增 WorkspaceId 域名）、国内/国际站分拆            |
| `models[].model` | 官方模型 ID 原文（大小写、分隔符一致）；已退役的必须移除                                                           | 老代际模型下架后仍留在预设里                                |
| `models[].dimension` | **默认输出维度**（不是最大值）。服务端默认维度常与"可调范围"混淆，填错会让每次嵌入都在维度校验处失败                                    | 智谱 `embedding-3` 默认 2048（易被当成 1024）、Gemini 默认 3072 |
| `dimensionParam` | 该厂商**能否**在请求体里下发维度，参数名叫什么。已知：OpenAI/DashScope/智谱=`dimensions`；Voyage=`output_dimension`；Cohere 兼容层明确 unsupported；Ollama 无 | 把 `dimensions` 发给只认 `output_dimension` 的厂商   |
| `models[].maxBatch` | 单请求 `input` 数组条数上限。差异极大（OpenAI 2048、智谱 64、百炼 v4 仅 10），**宁小勿大**——估大整批 400 | 按 OpenAI 的 2048 给百炼设值                          |
| `needsApiKey`   | 本地服务（Ollama/LM Studio/vLLM）传 `false`，表单会提示"填任意占位值"                                        | 本地预设要求真实 Key，用户被卡在保存校验上                        |
| `verifiedSource`/`verifiedAt` | 官方文档 URL；`verifiedAt` 由工具自动写入，**不要手填**                                                  | —                                             |

注：适配器按**模型**反查预设（`find_embedding_preset_by_model`），预设未覆盖的模型回退到
域名/模型名启发式；因此厂商行里的 `dimensionParam` 只作用于该厂商预设中列出的模型。

## 流程

### 第 1 步：读当前基线

读取数据源 1（+开发机数据源 2/3），列出核对队列中每家供应商当前值，做成对照表。

### 第 2 步：逐家联网核对（官方文档优先）

* 优先访问官方文档域名，只把第三方聚合站（llm-stats、各博客）当线索，**结论必须落到官方页**；

* 每家核对顺序：模型列表/更新日志（退役与新模型）→ 快速开始（base\_url）→ 协议兼容文档
  （OpenAI 兼容 / Anthropic 兼容 / Responses）→ 模型卡（上下文窗口、最大输出）→ 控制台入口；

* 中文厂商直接搜官方中文文档站；搜索词模板：
  `{厂商} API 模型列表 base_url 兼容 {年份}`、`{厂商} API deprecation retired {年份}`；

* **陷阱速查**：

  * 上下文窗口 ≠ 最大输出（1M 上下文的模型单次输出可能只有 64K）；

  * `latest` 别名与带日期后缀的 ID 并存时，预设优先用 `latest` 别名（自动跟进版本）；

  * 国内站与国际站（cn/ai 域名、不同 Key 体系）注意区分，预设默认国内站；

  * 「上下文缓存/思考模式」等计费特性不影响预设字段，不要写进 mainModels。

### 第 3 步：修订数据（工具优先）

**首选：`update_provider_preset` 工具**（经 tool_search 精确加载）。每核对完一家就调一次：

* 参数是**完整预设行**——整行替换语义，所有要保留的字段都必须带上，漏传即丢失；
* `verifiedSource` 传本次核对依据的官方文档 URL；
* **不要传 `verifiedAt`**——工具用系统时钟自动写入核对日期，不要凭记忆报日期；
* 工具自动递增插件版本（防止下次应用升级播种覆盖本次修改），无需手工处理。

**兜底：工具不可用时**，手工编辑数据源 1 的 providers.json（逐字段修订，保持 JSON
合法；camelCase、`labelKey` 复用现有 i18n 键 `config.preset_*` / `config.proto_*`，
新增厂商无 i18n 键时用 `label` 字段直接给英文显示名），并在每个修订行补
`"verifiedAt": "<系统时间工具取当前日期>"` 与 `"verifiedSource": "<官方文档 URL>"`；
同时递增 plugin.json 的 version（严格大于当前磁盘值）。

**开发机完整路径额外同步**（运行时路径不做，注明待办即可）：

* 同步 `ConfigWindow.tsx` 的 `PROVIDER_PRESETS`（与插件数据保持一致）；

* 输出上限变化时同步 `factory.rs::work_model_default_max_tokens` 及其顶部分级注释；

* 把同一份修订**回写仓库内置插件源** `src-tauri/plugins/llm-providers/providers.json`
  与 `plugin.json`（含 version），使发版内置数据与核对成果一致；

* 新增供应商时检查 `PROVIDER_LOGOS` 是否需要补 logo（`public/icons/providers/<id>.svg`，
  缺失时卡片用首字母徽标兜底，可另联网获取）。

### 第 4 步：验证

* 走工具路径：工具返回的 JSON 即落盘结果（id / version / verifiedAt / endpoint / defaultModel），
  与核对结论逐字段核对一遍；

* 手工路径改了 JSON 的：读回文件确认解析合法；

* 前端：`npx tsc --noEmit -p tsconfig.json`（工作区既有无关错误可忽略，确认无本次新增）；

* 改了 Rust 时（仅开发机）：`cargo check`（src-tauri 目录）；

* 重开设置 → LLM 页，厂商卡片应完整显示、切换预设能正确填充端点/模型；

* 有 API Key 的供应商可用「一键检测」实测端点连通性。

### 第 5 步：收尾

* 开发机路径：同步 `README.md`（suggestedMaxTokens 分级、协议支持描述）与
  `CODE_WIKI.md`（输出预算表）；

* 运行时路径：向用户汇报本次核对各家结果（哪家改了什么、依据哪份官方文档、
  插件新版本号），并注明「源码侧同步属开发期待办」。

## 注意

* 插件文件在用户目录被手工修改过后，应用更新只会因「版本号更高」覆盖；`update_provider_preset`
  自动 bump 版本即为此设计。手工定制者应自行保持 version ≥ 内置版本；

* `id` 是预设的稳定标识（provider\_cache 凭据快照按 id 索引），**不要变更已有 id**；

* 「自定义」卡片（id=custom）是 UI 概念，不属于插件数据，永远排在最后；

* 不做后台定时自动核对——本技能由用户要求或对话明确触发，避免未经授权的联网行为。
