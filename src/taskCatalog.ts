import base from '../src-tauri/prompts/routing/task_catalog.json';
import { CHARACTER_IDS, BUILTIN_CHARACTER_IDS, characterName } from './characters';

/** Called after the startup character snapshot is installed. */
export function characterTaskCatalog() {
  const tasks = [...base.tasks];
  const template = tasks.find(t => t.id === 'companion_vivian')!;
  for (const id of CHARACTER_IDS.filter(id => !BUILTIN_CHARACTER_IDS.some(b => b === id))) {
    tasks.push({ ...template, id: `companion_${id}`, labelKey: characterName(id),
      helpKey: `${characterName(id)} · model override`, instruction: tasks.find(t => t.id === 'companion')!.instruction });
  }
  return { ...base, tasks };
}
