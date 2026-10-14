export const comicTranslations: Record<string, Record<string, string>>;
export function comicLocale(language?: string): 'zh-CN' | 'en' | 'ja';
export function comicText(key: string, language?: string): string;
export function comicKeyForText(value: string): string | undefined;
