/** Replace only this stream's provisional paragraphs, keeping other turns in place. */
export function settleChatStream<T extends { id: string }>(
  previous: T[], paragraphIds: readonly string[], finalMessages: T[],
): T[] {
  const owned = new Set(paragraphIds);
  const first = previous.findIndex(message => owned.has(message.id));
  if (first < 0) return [...previous, ...finalMessages];
  const provisional = previous.filter(message => owned.has(message.id));
  const settled = finalMessages.map((message, index) => ({
    ...message, id: provisional[index]?.id ?? message.id,
  }));
  return [
    ...previous.slice(0, first), ...settled,
    ...previous.slice(first).filter(message => !owned.has(message.id)),
  ];
}
