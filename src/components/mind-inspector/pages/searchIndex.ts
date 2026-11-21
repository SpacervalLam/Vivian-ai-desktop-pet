export interface SearchableSession {
  session_id: string;
  title: string;
  working_directory: string;
  updated_at: number;
  status: string;
  search_excerpt?: string;
  messages?: { role: string; content: string }[];
}

export function indexSessions<T extends SearchableSession>(sessions: T[]) {
  return sessions.map(session => {
    const messages = (session.messages ?? []).filter(message => ['user', 'assistant'].includes(message.role))
      .map(message => message.content.replace(/\s+/g, ' ').trim()).filter(Boolean);
    if (session.search_excerpt) messages.push(session.search_excerpt);
    return { session, messages, text: [session.title, session.working_directory, ...messages].join('\n').toLocaleLowerCase() };
  });
}

export function searchSessions<T extends SearchableSession>(index: ReturnType<typeof indexSessions<T>>, query: string) {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return index.filter(item => terms.every(term => item.text.includes(term)))
    .map(item => {
      const title = item.session.title.toLocaleLowerCase();
      const message = item.messages.find(text => terms.some(term => text.toLocaleLowerCase().includes(term))) ?? item.messages[item.messages.length - 1] ?? '';
      const offset = terms.length ? Math.max(0, message.toLocaleLowerCase().indexOf(terms.find(term => message.toLocaleLowerCase().includes(term)) ?? '') - 35) : 0;
      return { session: item.session, score: terms.length && terms.every(term => title.includes(term)) ? 1 : 0,
        snippet: `${offset ? '…' : ''}${message.slice(offset, offset + 160)}${message.length > offset + 160 ? '…' : ''}` };
    })
    .sort((a, b) => b.score - a.score || b.session.updated_at - a.session.updated_at)
    .slice(0, 50);
}
