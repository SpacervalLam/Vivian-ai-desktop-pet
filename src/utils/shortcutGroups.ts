/**
 * 快捷启动的分组归并与搜索 —— 纯函数，界面只做渲染。
 *
 * 分组不做成独立实体：挂在每个快捷方式的 `group` 字段上（空串即「未分组」）。
 * 独立分组表会带来第二个真相源（删组要不要删条目、组序与条目序谁说了算），
 * 而这里只需要「按名字把条目聚在一起」，用字段派生就够了。
 */

export interface ShortcutLike {
  id: string;
  name: string;
  kind: string;
  target: string;
  /** 空串或缺失即未分组（旧配置没有这个字段）。 */
  group?: string;
}

export interface ShortcutSection<T extends ShortcutLike = ShortcutLike> {
  /** 渲染用的稳定 key：未分组不能直接用空串当 key。 */
  key: string;
  /** 空串表示未分组，渲染时换成当前语言的标签。 */
  name: string;
  items: T[];
}

export const UNGROUPED = '';

const groupOf = (item: ShortcutLike) => (item.group ?? '').trim();

/** 关键词按空白拆词，逐词都要命中（名称 / 目标 / 分组都能搜）。 */
export function shortcutMatches(item: ShortcutLike, query: string): boolean {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return true;
  const haystack = [item.name, item.target, groupOf(item)].join('\n').toLowerCase();
  return terms.every((term) => haystack.includes(term));
}

/**
 * 按分组归并：分组顺序取首次出现顺序（用户添加快捷方式的顺序），
 * 未分组固定排最后；搜索时丢弃没有命中的分组。
 */
export function groupShortcuts<T extends ShortcutLike>(items: T[], query = ''): Array<ShortcutSection<T>> {
  const buckets = new Map<string, T[]>();
  for (const item of items) {
    if (!shortcutMatches(item, query)) continue;
    const name = groupOf(item);
    const bucket = buckets.get(name);
    if (bucket) bucket.push(item); else buckets.set(name, [item]);
  }
  // Array.prototype.sort 是稳定排序，同秩分组保持首次出现顺序
  return [...buckets.entries()]
    .sort(([a], [b]) => Number(a === UNGROUPED) - Number(b === UNGROUPED))
    .map(([name, bucket]) => ({ key: name || '__ungrouped', name, items: bucket }));
}

/** 已用分组名（首次出现顺序），供表单输入时的候选。 */
export function shortcutGroupNames(items: ShortcutLike[]): string[] {
  const names: string[] = [];
  for (const item of items) {
    const name = groupOf(item);
    if (name && !names.includes(name)) names.push(name);
  }
  return names;
}

/**
 * 只有一个分组且就是「未分组」时不画分组头：没使用分组的用户看到的列表
 * 应该和没有这个功能时一样，而不是平白多一行标题。
 */
export function showsGroupHeaders(sections: Array<ShortcutSection>): boolean {
  return sections.some((section) => section.name !== UNGROUPED) || sections.length > 1;
}
