export const BUILTIN_CHARACTER_IDS = ['vivian', 'nana'];
export const CHARACTER_IDS = [...BUILTIN_CHARACTER_IDS];
const builtinNames = new Map([['vivian', 'Vivian'], ['nana', 'Nana']]);
let names = new Map(builtinNames);

export function registerCharacters(packages) {
  names = new Map(builtinNames);
  for (const { id, name } of packages) {
    if (!names.has(id)) names.set(id, name);
  }
  // Consumers retain references to this exported list across package reloads.
  CHARACTER_IDS.splice(0, CHARACTER_IDS.length, ...names.keys());
}

export function isBuiltinCharacterId(value) { return builtinNames.has(value); }
export function isCharacterId(value) { return names.has(value); }
export function characterName(id) { return names.get(id) || id || 'Vivian'; }
