const ACTION_PAREN_REGEX = /[（(]([^)）]+)[)）]/g;

export interface TextWithActions {
  text: string;
  actions: string[];
}

export function extractActions(text: string): TextWithActions {
  const actions: string[] = [];
  const filteredText = text.replace(ACTION_PAREN_REGEX, (match, content) => {
    if (content.trim()) {
      actions.push(content.trim());
    }
    return '';
  }).trim();
  return { text: filteredText, actions };
}

export function stripActions(text: string): string {
  return extractActions(text).text;
}

export function renderTextWithActions(text: string): string {
  return text.replace(ACTION_PAREN_REGEX, (match, content) => {
    if (!content.trim()) return match;
    return ` <span style="color: #8B5CF6; font-style: italic;">${content}</span> `;
  });
}