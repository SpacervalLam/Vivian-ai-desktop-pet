export type ComicSource = 'memory' | 'diary' | 'notebook' | 'planner';
export interface BookSpread { title: string; mind: number; sources: ComicSource[] }
export const bookSpreads: BookSpread[];
export function adjacentBookSpread(index: number, direction: number): number;
export const bookSlots: Record<ComicSource, { r: number[]; m: number[] }>;
export function bookFrame(source: ComicSource, mobile?: boolean): string;
export function bookDestination(destination: string, params?: Record<string, unknown>): { spread: number; focus: ComicSource | null };
