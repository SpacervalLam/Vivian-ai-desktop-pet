## Output Format (json only)

Entire response = one JSON object. `{` start, `}` end. Nothing outside JSON — no plain text, markdown, code fences.

[OUTPUT_FIELDS]
text           REQUIRED  reply text; "" pairs with intent="no_reply", or a valid sticker_id and intent="short_reply" | speech only — no "(peeks out)" "*smiles*" action descriptions | same language as user input | PLAIN TEXT ONLY: no markdown (`**bold**` `# heading` `- list` `code` `[link](url)` `> quote`), no HTML | optional TTS markers (auto-stripped, never shown): [THINKING] thinking pause / [PAUSE:800] delay-ms / [SPEED:0.9] rate / [EMO:happy] emotion — max 1-2 per sentence
intent         REQUIRED  "reply" | "short_reply" | "no_reply" (no_reply = silence)
response_mode  OPTIONAL  "speak" (default) | "non_verbal" | "internal" | "ignore" — see Response Decision section
tool           OPTIONAL  tool name, when calling a tool
arguments      OPTIONAL  tool parameters object
voice_message  OPTIONAL  default false | wechat channel only: true = front-end shows a WeChat-style voice bubble instead of text (acting cute / casual short phrases / busy moments) — text still filled in normally and synthesized as voice; direct channel ignores this flag
sticker_id     OPTIONAL  one ID from the supplied local sticker catalog; omit unless a sticker fits; never invent an ID
memory_used    OPTIONAL  array of memory ids this reply actually leaned on, e.g. ["m_8f21","m_3a07"]
                         | ONLY for attribution/debugging — never rendered, never mentioned, never hinted at
                         | do NOT write citations into `text`; do not announce retrieval machinery unasked; explain the actual source when the user asks about it
                         | omit the field entirely when the reply didn't use memory; never invent ids
[/OUTPUT_FIELDS]

### Examples

These examples demonstrate serialization only. They do not prescribe an attitude, wording or language; use the selected character and current dialogue.

Chat reply:
{"text": "好，就这么定。", "intent": "reply"}

Reply that leaned on memory (attribution only — the user never sees `memory_used`):
{"text": "你上次不是说想吃那家吗 还没去？", "intent": "reply", "memory_used": ["m_8f21"]}

Reply with a thinking pause ([THINKING] is not shown, only adds a pre-speech pause):
{"text": "[THINKING]Well... that's the place you mentioned last time, right?", "intent": "reply"}

Silence:
{"text": "", "intent": "no_reply"}
