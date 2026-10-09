/**
 * 桌面助手的分区目录 —— 唯一真相源。
 *
 * 分区标题有两个消费方：chat window 头部（助手页自己已经不带标题栏了）和组件内部
 * 的兜底判断。两边各写一份标题就会漂移，所以目录放这里，界面只做渲染。
 */
export interface AssistantPanelMeta { zh: string; ja: string; en: string }

/** 非总览的分区。顺序即 chat 头部返回层级的依据之外的展示顺序。 */
export const ASSISTANT_PANELS: Record<string, AssistantPanelMeta> = {
  notes: { zh: '随手记', ja: 'メモ', en: 'Notes' },
  shortcuts: { zh: '快捷启动', ja: 'クイック起動', en: 'Shortcuts' },
  games: { zh: '小游戏', ja: 'ゲーム', en: 'Games' },
  preferences: { zh: '偏好', ja: '好み', en: 'Preferences' },
  voice: { zh: '语音诊断', ja: '音声診断', en: 'Voice diagnostics' },
};

/** 分组总览。它和上面的分区是两个层级：返回键在总览上离开助手，在分区上退回总览。 */
export const isAssistantOverview = (panel: string | null | undefined): boolean => panel === 'daily' || panel === 'tools';

export const isAssistantPanel = (panel: string | null | undefined): boolean =>
  !!panel && (isAssistantOverview(panel) || panel in ASSISTANT_PANELS);

/** 总览没有自己的标题（沿用 chat 那条"桌面助手"），所以这里返回 null 交给调用方兜底。 */
export function assistantPanelTitle(panel: string | null | undefined, language: string): string | null {
  const meta = panel ? ASSISTANT_PANELS[panel] : undefined;
  if (!meta) return null;
  return language.startsWith('zh') ? meta.zh : language.startsWith('ja') ? meta.ja : meta.en;
}
