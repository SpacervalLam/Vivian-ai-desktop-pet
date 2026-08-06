# Vision Annotation Guidelines

> **Purpose**: Prevent LLM from misinterpreting desktop avatar as user screen content.
> **Usage**: Injected when screenshot is attached to vision-capable LLM.

---

## Avatar Overlay Annotation

When screenshot includes the desktop avatar, system may overlay:

```
这是 {{character}} 在桌面上的虚拟形象, 请 {{character}} 不要主动提及
This is {{character}}'s virtual avatar on the desktop, Please don't mention it, {{character}}
```

## LLM Instruction

```
Note: the screenshot may carry a small overlaid annotation reading
"This is <character>'s virtual avatar on the desktop, Please don't mention it, <character>".

This annotation only marks the avatar position — it is system metadata, NOT part of the user's screen content.

Ignore this annotation:
- Do NOT repeat it in your response
- Do NOT proactively mention the avatar position
- Focus on describing the actual user screen content
```

---

## Scenarios

- **Vision QA**: User asks "what's on my screen?" → describe user's apps/windows, ignore avatar
- **Context Understanding**: Avatar may be visible → it's decoration, not user action
- **Proactive**: Never start with "I see myself on your desktop" unless user explicitly asks

---

## Multi-language

| Field | Chinese | English |
|-------|---------|---------|
| Avatar annotation | 这是{{character}}在桌面上的虚拟形象, 请{{character}}不要主动提及 | This is {{character}}'s virtual avatar on the desktop, Please don't mention it, {{character}} |
| Ignore hint | 注：截图上可能叠加了一段小字...这只是系统元数据，请忽略，不要复述也不要主动提及。 | Note: the screenshot may carry a small overlaid annotation... Ignore it, do not repeat it, and do not bring it up. |