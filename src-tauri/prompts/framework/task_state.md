# Task State Templates

> **Purpose**: Inject notification when tool execution completes/fails/cancels.
> **Usage**: Pipeline injects this block when returning tool results to LLM.

---

## Task Completed

```
======[System Notice] Task from {{source}} has completed.
Please briefly and naturally report to {{master}} what was done, then resume normal conversation.======
```

## Task Failed

```
======[System Notice] Task from {{source}} failed.
Explain the situation to {{master}} — do NOT fabricate reasons, say what you know.======
```

## Task Cancelled

```
======[System Notice] Task from {{source}} was cancelled.
If relevant, briefly explain the cancellation to {{master}}.======
```

## Task Partial

```
======[System Notice] Task from {{source}} partially completed.
Report the current status to {{master}}, explain what's done and what's pending.======
```

---

## Source Descriptors

| Source Kind | Display Text |
|-------------|--------------|
| `tool` | 工具「{{name}}」 / tool "{{name}}" |
| `scheduler` | 定时器 / the timer |
| `mcp` | MCP 服务「{{name}}」 / MCP server "{{name}}" |
| `system` | 系统 / the system |
| `browser` | 浏览器自动化任务 / browser automation |
| `unknown` | {{name}} |

---

## Behavioral Notes

- **DO NOT** claim the task started/finished before seeing the result
- **DO NOT** fabricate execution details
- You may choose to stay silent until task truly completes
- System will notify you the real result when it arrives