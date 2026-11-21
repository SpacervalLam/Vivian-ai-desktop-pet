import { ongoingTaskOf, type Character, type MemoryRecord, type OngoingTask } from './memoryPresentation';

export type AgreementRef = { character_id: string; id: string; title: string };
export type AgreementView = { id: string; owner: Character; task: OngoingTask & { revision: number } };
export type LinkedTodo = { id: string; title: string; description?: string; completed: boolean; agreement?: AgreementRef | null };
export const agreementWords = (lang: string, zh: string, en: string, ja = en) => lang.startsWith('en') ? en : lang.startsWith('ja') ? ja : zh;
export const agreementViews = (owner: Character, records: MemoryRecord[]): AgreementView[] => records.flatMap(item => {
  const task = ongoingTaskOf(item);
  const revision = (item.metadata?.ongoing_task as { revision?: unknown } | undefined)?.revision;
  return task && item.metadata?.memory_owner === owner && !item.consolidated && typeof revision === 'number' ? [{ id: item.id, owner, task: { ...task, revision } }] : [];
});
export const matchesAgreement = (todo: LinkedTodo, view: Pick<AgreementView, 'id' | 'owner'>): boolean =>
  todo.agreement?.id === view.id && todo.agreement.character_id === view.owner;
export const agreementHrefParams = (ref: AgreementRef) => ({ sub: 'agreements', agreementId: ref.id, agreementOwner: ref.character_id });
