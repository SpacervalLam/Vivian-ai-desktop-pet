# Output Budget Constraints

> **Purpose**: Control response length for auxiliary LLM calls (not the main chat).
> **Usage**: Injected by memory/assistant modules to prevent token explosion.

---

## General Rules

- Every LLM call has a **token budget** and **timeout**.
- **No fallback** to larger models when budget exceeded — failure is explicit.
- Budget is per-call, not cumulative across conversation.

---

## Budget Templates

### Image Description

```
Your image description must not exceed:
- 250 words (English)
- 250字 (Chinese)
Focus on main content and interesting details. Be concise.
```

### Search Keywords

```
Output exactly 3 search keywords.
- One keyword per line
- No numbers, punctuation, or explanations
- Each keyword: 2-6 words (EN) / 2-8字 (ZH)
```

### Memory Summary

```
Summarize in ≤ {{max_tokens}} tokens.
Capture key entities and events. Skip minor details.
```

### Fact Extraction

```
Extract ≤ {{max_facts}} facts from this conversation.
Each fact: one short sentence. No redundancy.
```

---

## Timeout Behavior

- LLM call timeout = hard limit, not a suggestion
- If timeout hits: task fails, no silent fallback
- Pipeline logs timeout and returns empty/error result
- Main chat loop receives "timed out" notification

---

## Input Budget

Before passing strings to LLM:
- Truncate with `truncate_to_tokens(str, max_tokens)`
- Use `truncate_head_tail_tokens(str, max_head, max_tail)` for long documents
- Magic constants defined in `config/__init__.py` §3.7

---

## Exemptions

The following do NOT require input budget capping:
- User-provided config strings (user's responsibility)
- OS window titles (external, not our content)
- Tool execution results (already bounded by tool implementation)

Add `# noqa: LLM_INPUT_BUDGET` comment if you must skip cap in code.