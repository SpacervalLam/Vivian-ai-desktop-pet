# Vivian 工作流集成

## 源码依据与实现选择

- [dsh-plugin-agent-workflow](https://github.com/xuanyuanzhifeng/dsh-plugin-agent-workflow)，本地审阅提交 `41b0cd52c43cd667bfcea33460e6a5f401184057`，版本 0.3.0，适配 DSH 0.2.0-rc.2。
- [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)，本地审阅提交 `571902028a75c64c1d767f4ae5fd099485f0ddf0`，版本 0.2.1-alpha.3。
- 参考插件的 `src/client/workflow-model.ts`、`WorkflowView.tsx`、`request-messages.ts` 与 DSH 的 `packages/core/session/src/surface.ts`、`packages/client/ui-trajectory/src/client/trajectory-event-projection.ts`。

该插件提供会话执行记录的只读视图。它的宿主入口 `apply()` 为空；浏览器入口需要 Cordis slots、sessions、locale 及 DSH 的事件投影。Vivian 的执行器为 Rust、界面为 React/Tauri，且本地 DSH 版本高于插件适配版本。因此在 Vivian 中原生实现相同的轮次→请求→响应→工具交互，复用现有工作智能体、消息和模型路由，不安装另一套 DSH 会话运行时。发布包无需访问两个本地源码目录，也没有新增 npm/Cargo 依赖。

这是一份针对 Vivian 数据结构的独立适配；两个上游项目均使用 MIT 许可，本次未复制其实现文件。后续如直接引入上游源码，需随分发保留对应版权及完整 MIT 许可。

## 数据流

右侧检查器只保留「工作流」入口。其内部「执行链路」展示请求/响应/工具卡片，「事件明细」复用 `TrajectoryPanel`，提供原有时间线、搜索、折叠和详情。工作台快照保存内部视图选择，历史 `rightTab: trajectory` 自动迁移为 `rightTab: workflow, workflowView: events`；旧会话即使没有请求快照也能查看事件明细。

`CodingAgentService` 在模式选择、standard/minimal 每次请求尝试、非流式恢复、code 程序生成和中断总结时创建 `Capture`。输入在调用 ModelRouter 前采集；完成、错误和流重试逐次结算，取消或提前退出由 guard 记录中断。每次尝试有独立 UUID；stream 重试不会覆盖前次记录，也不会执行额外模型或工具调用。

`coding_workflow.rs` 将每个请求拆成详情 JSON 和摘要 `.meta.json`，存放于用户数据目录 `coding_workflow/<会话 ID 的 UTF-8 十六进制>/`。详情包含 system、完整的智能体 messages、工具定义、推理偏好及组装后的响应；摘要仅含轮次消息索引、步骤、尝试、配置模型名、时间、状态、预览、工具调用 ID 与提供方上报的 usage。先原子写详情，再发布摘要；列表不会读取大请求体，启动和会话目录不加载这些快照。

`coding:workflow` 事件只广播摘要；`coding:workflow_error` 报告持久化失败。`coding_workflow_list` 列出指定现存会话的摘要，`coding_workflow_detail` 按会话 ID 和请求 ID 读取单条详情；命令在阻塞线程上读取磁盘。ID 编码后才能作为目录/文件名，不接受界面提供的任意路径。删除会话时清理快照，互斥提交和存在性检查防止迟到结算重新创建已删除的数据；fork 不复制原会话的请求记录。

前端先订阅事件再加载摘要，用请求 UUID 合并，并保护已完成事件不被较旧的初始列表覆盖。只在打开详情时读取请求正文。每个普通用户消息开启一轮；执行中的引导/插话保留在原轮次。记录同时保存原始用户消息索引和从零开始的轮次序号；界面用轮次序号关联请求，避免实时界面合并工具调用/结果导致消息索引与持久化历史不同。工具在轮次内按调用 ID 关联，保留同名和并行调用；运行中的单行替换及持久化的独立调用/结果记录得到同样的链路。code 模式的宿主执行工具归入对应程序生成请求。请求行使用虚拟滚动，执行链横向滚动，JSON 子树按展开挂载、每批 50 个成员加载。

## 准确性范围

- 快照标识为 `agent_input`。它准确记录智能体传给路由器的输入，不能称为最终 HTTP 请求体：路由器会修复消息，提供方可能转换协议、添加框架指令或回退到另一模型。界面明确显示采集阶段及“模型名为配置值”的说明。没有保存认证头、Provider 配置或 API Key 字段。
- 响应是应用收集的 content/reasoning/tool_calls，流式失败保留收到的部分文本；不是提供方的原始 SSE/HTTP 报文。usage 只展示提供方实际返回的值；缺失值显示“模型未返回用量”，不生成假 token 数。每次外层重试分别记录，ModelRouter 内部回退和自动压缩/后台记忆/子智能体的内部请求不在当前覆盖范围内。
- 旧会话不回填请求，不用目前的提示词推断历史。没有快照的轮次仍可展示已记录工具，并说明缺少请求记录。重启后仍处于 running 的快照在非运行会话中显示中断。
- 输入与输出快照保存在本地，包含任务上下文及可能存在的附件内联内容，具有与会话历史相同的敏感性。详情仅按需传给 WebView。快照目前随会话删除，不设置隐式数量截断；长期运行会增加磁盘用量。
- 工具结果正文沿用会话已经保存的结果摘要。工具耗时沿用当前消息流的 `duration_ms`；旧历史没有保存该字段时显示缺失，不从时间戳猜测耗时。请求耗时按本次尝试的单调时钟计算，不把各并行工具耗时相加成墙钟时间。

## 验证

运行 `node tests/workflow-model.test.mjs` 验证轮次、引导、同名/并行工具、重试、恢复、旧记录和长链路；`node tests/workflow-json.test.mjs` 验证延迟展开、分页、完整复制、长文本与剪贴板错误；`node tests/workbench-snapshot.test.mjs` 验证工作流标签恢复。`cargo test --locked --manifest-path src-tauri/runtime-contracts/Cargo.toml coding_workflow -j 1` 验证独立快照、元数据更新、损坏详情、标识编码、中断恢复和删除后的迟到结算。`npm run typecheck` 与 `cargo check --locked --manifest-path src-tauri/Cargo.toml --lib -j 1` 验证两侧接口。
