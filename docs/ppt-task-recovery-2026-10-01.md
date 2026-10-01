# PPT 任务失败与流式恢复修复

## 证据与原因

- 日志：`%APPDATA%/vivian/logs/vivian_2026-10-01.log`。
- 会话：`code-0da51fac968e4ce68dfa941096032ac2`，standard 模式，任务为生成带英文讲稿的 `ai_presentation.pptx`。
- 起初缺少 `pptx` 模块，随后安装 `python-pptx` 成功，导入验证输出 `ok 1.0.2`；因此依赖缺失并非最终终止原因。
- 未选择工作目录，`list_dir` 失败；模型随后查询用户目录与桌面，继续执行了依赖安装。
- 2026-10-01 12:47:19（北京时间），第三轮模型响应报告 `流读取失败: error decoding response body`。还没有执行写文件或生成 PPT 的命令。
- 原循环只对 `finish_reason=tool_calls` 且无调用数据重试。收到 `StreamEvent::Error` 立即停止，错误分类还将这条传输错误归为 unknown。
- 日志不足以确定底层断开来自代理、上游服务还是连接本身。日志里的 Tavily 搜索失败和模型名带空格的 400 属于后台热梗采集，不能据此认定它们导致了 PPT 任务终止。

## 修复行为

- 当前轮模型请求遇到网络、超时、服务故障、过载或限流错误时，最多尝试三次，重试前等待 1 / 2 秒。
- 流接收通道未经 Done 事件就关闭，也按传输中断处理。
- 只有完整流响应才进入工具执行；不执行中断响应中的半截工具参数，不重放之前轮次已完成的工具操作。
- 重试沿用当前轮请求和之前工具结果。缺失工具调用时才追加原有的 function calling 提醒。
- 前端通过 `coding:stream_reset` 清理中断尝试的文本和思考片段。
- 鉴权、余额不足、请求参数错误等永久错误不自动重试；达到尝试上限后给出具体类别的错误提示。
- ProviderBase 在构造时裁剪模型名首尾空白，修复日志中独立出现的 `deepseek-flash ` 无效模型名问题。

## 验证

- `cargo test --lib work_stream_`：验证日志中的真实错误分类、可重试错误、尝试上限与永久错误。
- `cargo test --lib provider_model_trims_config_whitespace`：验证模型名规范化。
- `npx tsc --noEmit`：验证前端事件处理类型。

实际结果：Rust 测试构建成功，以上新增的 3 个用例和 resilience 模块的 30 个现有用例全部通过；TypeScript 检查通过；`node tests/workbench-layout.test.mjs` 的 576 种布局组合与工具结果协调检查通过。构建中的 28 条警告位于其他模块。

以上验证不调用真实模型 API，也不代表已重新生成 PPT。需要重新构建运行桌宠，随后在原工作会话发送“继续”，才能使用修复后的执行逻辑。
