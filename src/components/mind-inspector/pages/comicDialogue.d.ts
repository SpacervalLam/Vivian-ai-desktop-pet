import type { ComicCharacter } from './ComicOverview';
export type DialogueCategory='mood'|'emotions'|'needs'|'relations';
export function createDialogueSelector():(role:'vivian'|'nana',category:DialogueCategory,character:ComicCharacter,visit?:number,language?:string)=>string;
export const selectComicDialogue:ReturnType<typeof createDialogueSelector>;
