export const recordTranslations: Record<string, Record<string, string>>;
export function recordLocale(language?: string): 'zh-CN' | 'en' | 'ja';
export function recordText(key: string, language?: string): string;
export function recordKeyForText(value: string): string | undefined;
