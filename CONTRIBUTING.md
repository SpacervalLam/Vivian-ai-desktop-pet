# 贡献指南

本文档说明参与本项目的开发流程与约定。

---

## 开发环境准备

### 必需工具

| 工具 | 版本 | 说明 |
|------|------|------|
| Rust | 1.75+（stable） | `rustup default stable` |
| Node.js | 18+ | 推荐 LTS |
| Windows | 10 / 11 | 当前仅完整支持 Windows |

### 初始化

```bash
git clone <repo-url>
cd vivian-rs
npm install
```

### 验证环境

```bash
cd src-tauri && cargo check     # 应无错误
npx tsc --noEmit                # 应无错误
npm run tauri:dev               # 应能正常启动
```

---

## 开发流程

### 1. 选择或创建 Issue

- 优先处理已有 Issue
- 新功能请先开 Issue 讨论设计方向，避免做无用功
- Bug 修复可直接提交 PR

### 2. 分支命名

```
<type>/<short-description>

# 示例
fix/window-not-opening
feature/proactive-stress-monitor
docs/readme-rewrite
refactor/memory-pipeline
```

`type` 取值：`fix` / `feature` / `docs` / `refactor` / `perf` / `test` / `chore`

### 3. 编码

- 代码注释统一使用**中文**
- 遵循 [.editorconfig](file:///g:/vivian-rs/.editorconfig) 与 [.gitattributes](file:///g:/vivian-rs/.gitattributes) 的格式约定
- 不要为简单改动添加多余注释、文档字符串或类型注解
- 不引入未被使用的依赖

### 4. 提交前自检

```bash
cd src-tauri && cargo check
npx tsc --noEmit
```

确保两者均无错误后再提交。

### 5. 提交信息（Commit Message）

使用约定式提交（Conventional Commits）：

```
<type>(<scope>): <subject>

<body>

<footer>
```

- `type`：`feat` / `fix` / `docs` / `refactor` / `perf` / `test` / `chore` / `style` / `build` / `ci`
- `scope`（可选）：受影响的模块，如 `brain` / `memory` / `pipeline` / `live2d` / `tools` 等
- `subject`：祈使句，简洁描述改动

示例：

```
feat(proactive): 加入压力监控触发器
fix(window): 修复子窗口在 Windows 上不可见的问题
docs(readme): 按实际结构重写项目说明
```

### 6. 提交 Pull Request

- PR 标题与提交信息保持一致风格
- 在 PR 描述中说明：改了什么、为什么改、如何验证
- 关联相关 Issue（`Fixes #123` / `Ref #456`）
- 等待 CI 通过与 reviewer 反馈

---

## 代码风格

### Rust

- 缩进 4 空格
- 遵循 `rustfmt` 默认配置
- 公共 API 加中文 doc 注释（`///`）
- 错误使用 `VivianResult<T>`，不直接 `unwrap` / `expect`（测试代码除外）
- 模块顶部加 `//!` 模块级文档注释，说明职责

### TypeScript / React

- 缩进 2 空格
- 函数组件 + Hooks，不使用 class component
- 全局状态使用 Zustand，不引入 Redux
- 与后端交互通过 `@tauri-apps/api` 的 `invoke`，不直接访问文件系统
- 类型定义集中在 [src/types/index.ts](file:///g:/vivian-rs/src/types/index.ts)，与 Rust 结构对齐

---

## 架构约束

以下约束来自项目演进过程中的决策，提交前请确认不违反：

1. **LLM 分类在写入路径完成**：记忆类型分类必须由 LLM 在写入时完成，`MemoryFilter` 读取路径不得调用 LLM，分类结果从 `memory.metadata["classification"]` 读取
2. **中文分词用 jieba**：`memory/filter.rs` 中文关键词提取必须使用 `jieba-rs`，不能用 `split()`
3. **关系情绪用用户情绪**：关系情感分析必须使用用户输入情绪，不使用 AI 响应情绪
4. **BrainChatChain 用 Runnable**：必须使用 `AIResponseGenerationRunnable + ResponseParsingRunnable`，不用 `GenerationStep`（简化版不解析 JSON）
5. **主 LLM 必须配置**：主 LLM API（api_key / endpoint / model）必须完整配置，否则终止后续流程并提示 toast
6. **沉默标记处理**：`intent="no_reply"` 由 `ResponseParsingRunnable` 识别并清空 `text`
7. **动作映射**：LLM 输出 `motion="umbrella_close"` 时，`ResponseParsingRunnable` 必须映射为 `expression="umbrella_close"` 并置 `motion="idle"`
8. **亲密度增量**：正向情绪的亲密度增量计算必须用 `(intensity * 2.0).floor() + 1.0`
9. **启动问候**：LLM 调用返回 `Option<String>`，失败或空结果时不发问候（不用模板兜底）
10. **情绪字段**：`user_emotion` / `ai_emotion` 由 LLM 在 JSON 返回中给出，不再用关键词匹配模块代理；`ai_emotion` 仅用于 emotion_score 显示与记忆持久化，LLM 路径无 fallback 表情映射
11. **死字段**：`user_emotion_confidence`、`metadata["emotion"]`、`metadata["emotion_source"]` 已删除，不要恢复
12. **MemoryItem 字段同步**：`MemoryItem` 新增字段后，须在 4 处 struct literal 补齐默认值：`manager.rs` seed1 / seed2 / merge_eviction + `retriever.rs` rerank placeholder
13. **proactive 浮点字面量类型标注**：`compute_overall_cooling()` 中的 match/if 浮点字面量须显式 `: f64` 标注，否则类型推断失败
14. **Personality↔Expression 双向同步**：`PersonaProfile` 变更须调用 `to_expression_hint()` 产出表情暗示；`CharacterExpression` 变更须调用 `sync_from_persona_hint()` 低权 blend 回人格，避免单向漂移
15. **反驳 grace period**：证据系统的 `rebuttal_grace_remaining` 字段提供 3 tick 宽限期，防止单次正信号立即恢复被反驳的记忆可信度
16. **记忆整合软归档**：巩固流水线使用 `consolidated` 布尔字段标记已整合记忆（soft-archive），不使用 hard-delete，保留审计轨迹
17. **统一 turn boundary**：`apply_turn_boundary()` 统一 3 条写入路径（chat_chain / proactive / cross_character），确保心理状态、对话历史、记忆写入在同一边界对齐
18. **主题连续性回退**：pipeline 话题检测使用 keyword Jaccard 相似度做回退判断，避免 LLM 话题分类延迟导致的误切
19. **世界知识上下文过滤**：`world_knowledge.rs` 使用关键词 + 锚点策略过滤无关世界知识注入，避免 token 浪费
20. **LLM 活动↔窗口分类交叉验证**：`brain.rs` 中 LLM 提取的活动信息与 `SmartAppClassifier` 窗口分类结果交叉验证，不一致时降低置信度
21. **多消息/tick 限制**：主动对话编排每 tick 最多发送 `MAX_TICK_MESSAGES=2` 条消息，防止连续刷屏
22. **跨角色认知传播**：`roommate_cognitive_text()` 仅传播行为印象（注意力、活动、目标、社交意愿），不暴露原始认知结构，维护 "Public State vs Private Mind" 边界
23. **人格文件外观分离**：`soul.md` / `nana_soul.md` 只定义人格内核，外观描述独立到 `appearance_vivian.md` / `appearance_nana.md`，由 `render_identity_block()` 拼接
24. **反面样本约束**：`canon_quotes.md` / `nana_canon_quotes.md` 第三节包含反面样本（客服味 / 小作文 / 假热情），LLM 应按反面样本规避自身默认助手模式

---

## 测试

- 当前测试覆盖率有限，新增核心逻辑应附单元测试
- 测试代码放在对应模块的 `#[cfg(test)] mod tests` 中
- 不要为显而易见的行为写测试

---

## 提交 Issue

### Bug 报告

请使用 [Bug 报告模板](file:///g:/vivian-rs/.github/ISSUE_TEMPLATE/bug_report.md)，包含：

- 复现步骤
- 预期 / 实际行为
- 环境信息（Windows 版本 / Vivian 版本 / 是否 release 构建）
- 相关日志（`%APPDATA%\Vivian\logs\`）

### 功能建议

请使用 [功能建议模板](file:///g:/vivian-rs/.github/ISSUE_TEMPLATE/feature_request.md)，说明：

- 使用场景
- 期望行为
- 已考虑的替代方案

---

## 维护者

如对项目方向有疑问，可在 Issue 中 `@` 维护者讨论。重大架构变更请先开 RFC Issue。

感谢你的贡献。
