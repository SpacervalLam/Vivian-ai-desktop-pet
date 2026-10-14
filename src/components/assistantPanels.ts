/**
 * 聊天窗口工具子页（随手记 / 快捷启动）的分区目录 —— 唯一真相源。
 *
 * 分区标题有两个消费方：chat window 头部（子页自己不带标题栏）和组件内部的兜底判断。
 * 两边各写一份标题就会漂移，所以目录放这里，界面只做渲染。
 *
 * 注意：这两个子页没有「总览」层。头部三点下拉与桌宠侧边栏（`ChatEdgeMenu` 的
 * `menu_panel`）各自直接进入某一页，所以这里不再有 overview 的概念。
 */
export interface AssistantPanelMeta { zh: string; ja: string; en: string }

/** 子页目录。键名即后端 `menu_panel` / URL `assistant_panel` 使用的标识。 */
export const ASSISTANT_PANELS: Record<string, AssistantPanelMeta> = {
  notes: { zh: '随手记', ja: 'メモ', en: 'Notes' },
  shortcuts: { zh: '快捷启动', ja: 'クイック起動', en: 'Shortcuts' },
};

export const isAssistantPanel = (panel: string | null | undefined): boolean =>
  !!panel && panel in ASSISTANT_PANELS;

/** 未知分区返回 null，交给调用方兜底。 */
export function assistantPanelTitle(panel: string | null | undefined, language: string): string | null {
  const meta = panel ? ASSISTANT_PANELS[panel] : undefined;
  if (!meta) return null;
  return language.startsWith('zh') ? meta.zh : language.startsWith('ja') ? meta.ja : meta.en;
}
