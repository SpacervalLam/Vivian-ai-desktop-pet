export type BuiltinCharacterId = 'vivian' | 'nana';
export type CharacterId = string;
export const BUILTIN_CHARACTER_IDS: readonly ['vivian', 'nana'];
export const CHARACTER_IDS: string[];
export function registerCharacters(packages: {id: string; name: string}[]): void;
export function isBuiltinCharacterId(value: unknown): value is BuiltinCharacterId;
export function isCharacterId(value: unknown): value is CharacterId;
export function characterName(id: string | null | undefined): string;
