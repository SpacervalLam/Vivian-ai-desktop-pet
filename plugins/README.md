# Vivian 扩展开发与安装

设置 → 外部连接管理 MCP；设置 → 插件管理完整能力包。MCP 工具和插件工具会进入陪伴侧工具发现，以及工作侧 standard、code、minimal 模式。工作侧每轮刷新工具清单；在工具设置里禁用的能力不会重新暴露。

## MCP 连接

支持本地 stdio 和 Streamable HTTP（JSON 或 SSE 响应）。HTTP 认证可配置请求头，例如 `Authorization=Bearer …`；当前没有交互式 OAuth 登录流程，也不实现旧版 HTTP+SSE 双端点传输、resources/prompts 等协议能力。

本地连接需要启动命令，支持工作目录与环境变量。启动参数推荐使用 JSON 字符串数组，避免 Windows 路径的引号与反斜杠歧义：

```json
["-y", "your-mcp-package", "G:\\My Files"]
```

环境变量与 HTTP 请求头可填 JSON 对象或每行 `KEY=VALUE`。这些值保存在本机 MCP 配置文件中；敏感配置查询和连接管理只接受设置窗口调用。

插件提供的连接也可编辑，用户设置独立保留，重载、更新或重启不会覆盖凭据和启停选择。“恢复插件默认”撤销该连接的用户覆盖并重载来源插件；彻底移除这类连接请禁用或删除对应插件。

“测试连接”会真实启动进程或初始化远程会话，发现工具后关闭，不保存配置或注册长期工具。“保存”保留配置，包括禁用或连接失败的条目。页面显示具体错误、工具清单、重试次数，支持编辑、停用和手动重连。后台按指数退避重试，连续五次重连失败后停止自动重试，手动重连会重新开始。

工具输入保留 `inputSchema` 的必填字段；工具分页完整读取。`isError: true` 会作为执行失败返回，业务失败不会被当作连接掉线；文本、图片、资源块及 `structuredContent` 保留在工具结果中。多模态块的可视化取决于消费端，不代表所有模型均能直接理解所有内容类型。

## 插件包

通过插件页选择 ZIP 或目录，预览名称、版本、依赖、脚本、MCP 声明后安装。ZIP 根目录或唯一子目录必须有 `plugin.json`。导入不会执行插件；新包或内容变更的包需要再次信任。更新复核源文件和当前安装版本的指纹，避免预览后内容变化。安装替换失败时尝试恢复旧文件；极端文件锁冲突下保留恢复目录并返回路径。

包内不允许符号链接、越界路径、Windows 设备文件名或重复文件；最多 10,000 个条目、512 MiB 解压内容。公寓仍使用它的独立安装流程，通用导入不能覆盖内置插件。

```json
{
  "api": 1,
  "name": "my-extension",
  "version": "1.0.0",
  "description": "我的扩展",
  "skills": ["skills/*.md"],
  "tools": ["tools/*.json"],
  "js": ["js/*.js"],
  "mcp_servers": [],
  "requirements": {
    "commands": ["node"],
    "plugins": {"another-extension": "1.0.0"}
  }
}
```

`requirements.commands` 检查 PATH 中的命令或包内相对路径；`requirements.plugins` 使用插件目录名与最低版本号。缺少依赖仍可安装，但加载会阻止并显示具体原因。依赖安装好后重载。禁用会撤销技能、工具、MCP 和 JS 运行时；禁用状态独立保存，重新启动也有效。

状态区分已安装未装载、等待信任、缺少依赖、部分可用、已装载和已禁用。附带技能是按需读取的使用指导，不会独立执行操作或获得额外权限。

## JavaScript 宿主

每个插件有独立 Boa 运行时。`ctx.apiVersion` 为宿主 API 版本；`ctx.tool`、`ctx.skill`、`ctx.mcp`、`ctx.provider`、`ctx.on`、`ctx.log` 是注册面。它不是 Node.js 环境，没有 `require`、文件系统或 `fetch` 接口。外部 I/O 应注册 MCP 或声明式 PowerShell 工具。工具处理函数必须同步返回 JSON 可序列化值；返回 Promise 会明确报错。

```js
if (ctx.apiVersion !== 1) throw Error('Unsupported plugin API');
ctx.tool('normalize_label', '整理标签文本', {
  type: 'object',
  properties: { text: { type: 'string' } },
  required: ['text']
}, function (args) {
  return { text: args.text.trim().replace(/\s+/g, ' ') };
});
```

运行 `npm run test:extensions` 验证协议、回执、恢复、工作侧工具发现和插件生命周期。Rust 测试使用独立用户数据目录。
