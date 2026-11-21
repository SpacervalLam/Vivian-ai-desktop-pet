import { stripActions } from './ActionText';

/** Match the text ChatWindow actually renders, rather than the raw model output. */
export function hasVisibleChatText(content: string, role: string = 'assistant'): boolean {
  const text = role === 'user' ? content : stripActions(content).replace(/__/g, '');
  return text.trim().length > 0;
}

/** Media and active typing placeholders are valid even without a text caption. */
export function isVisibleChatMessage(message: {
  role: string;
  content: string;
  streaming?: boolean;
  imageDataUrl?: string;
  imagePath?: string;
  sticker?: unknown;
  linkCard?: unknown;
  fileMeta?: unknown;
  voice?: unknown;
}): boolean {
  return !!(message.streaming || message.sticker || message.imageDataUrl || message.imagePath
    || message.linkCard || message.fileMeta || message.voice)
    || hasVisibleChatText(message.content, message.role);
}
