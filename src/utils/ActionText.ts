export interface ActionTextPart {
  text: string;
  action: boolean;
}

/** Keep narration in storage, but hide it from speech even before its closing bracket arrives. */
export function splitActionText(text: string): ActionTextPart[] {
  const parts: ActionTextPart[] = [];
  let start = 0;
  let depth = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '（' || text[i] === '(') {
      if (depth === 0) {
        if (i > start) parts.push({ text: text.slice(start, i), action: false });
        start = i;
      }
      depth++;
    } else if ((text[i] === '）' || text[i] === ')') && depth > 0) {
      depth--;
      if (depth === 0) {
        parts.push({ text: text.slice(start, i + 1), action: true });
        start = i + 1;
      }
    }
  }
  if (start < text.length) parts.push({ text: text.slice(start), action: depth > 0 });
  return parts;
}

export interface TextWithActions {
  text: string;
  actions: string[];
}

export function extractActions(text: string): TextWithActions {
  const parts = splitActionText(text);
  return {
    text: parts.filter((part) => !part.action).map((part) => part.text).join('').trim(),
    actions: parts.filter((part) => part.action).map((part) => part.text.replace(/^[（(]|[）)]$/g, '').trim()).filter(Boolean),
  };
}

export function stripActions(text: string): string {
  return extractActions(text).text;
}
