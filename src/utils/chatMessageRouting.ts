/** Decide where an incoming message belongs without mixing private conversations. */
export function routeAssistantMessage(
  view: 'home' | 'private' | 'group' | 'details' | 'assistant',
  privateCharacter: string | null | undefined,
  character: string | undefined,
  channel: string | undefined,
): { append: 'private' | 'group' | null; unread?: string; preview?: string } {
  if (channel === 'wechat_group' || (!channel && view === 'group')) {
    return view === 'group' ? { append: 'group' } : { append: null, unread: 'group' };
  }
  if (channel && channel !== 'wechat') return { append: null };
  const viewing = view === 'private' && (!character || character === privateCharacter);
  return {
    append: viewing ? 'private' : null,
    preview: character,
    unread: viewing ? undefined : character,
  };
}
