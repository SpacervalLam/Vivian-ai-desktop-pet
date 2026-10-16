const zh = {
  title: '用量与上下文洞察', usage: '调用用量', context: 'Prompt 快照', switchView: '洞察视图',
  activity: '调用活动', eightWeeks: '最近 8 周', heatHelp: '点击日期，联动筛选下方全部用量统计', less: '少', more: '多', weekdays: ['一', '三', '五'], clearDay: '返回时间段',
  activeDays: '有记录天数', dailyAverage: '日均记录 Token', perCall: '每次调用平均', peak: '峰值日', noPeak: '暂无峰值',
  cacheTitle: '缓存分析', cacheHelp: '以记录的输入 Token 为分母，包含非缓存输入、缓存读取和缓存创建。', cacheModels: '各模型缓存读取占比', noCache: '暂无输入用量记录', viewModels: '查看模型明细',
  snapshotTitle: '最近 Prompt 组装', snapshotHelp: '查看本次应用运行中，各角色最近一次实际组装的 Prompt。分区 Token 为估算值，不等于 API 实际用量或完整上下文窗口占用。',
  snapshotEmpty: '还没有 Prompt 快照', snapshotEmptyHelp: '与角色对话后，实际组装记录会显示在这里。模板预览不会当作实际记录。', snapshotError: '无法读取 Prompt 快照',
  estimated: '估算 Token', characters: '字符', sections: '个分区', sectionSearch: '搜索分区名称或内容', sectionSort: '分区排序', size: 'Token 从多到少', original: '组装顺序', showContent: '查看实际内容',
  apiParams: '请求附加参数', apiHelp: '这些内容独立于 Prompt 分区展示，没有合并到分区 Token 估算中。', noSections: '没有匹配的已注入分区', snapshotWhen: '组装于', refresh: '刷新快照', loading: '正在读取快照…', retry: '重试', notInjected: '未注入',
};
const en: typeof zh = {
  title: 'Usage & context insights', usage: 'API usage', context: 'Prompt snapshots', switchView: 'Insights view',
  activity: 'Call activity', eightWeeks: 'Last 8 weeks', heatHelp: 'Select a day to filter all usage below', less: 'Less', more: 'More', weekdays: ['M', 'W', 'F'], clearDay: 'Back to range',
  activeDays: 'Days with records', dailyAverage: 'Recorded tokens / day', perCall: 'Tokens / recorded call', peak: 'Peak day', noPeak: 'No peak yet',
  cacheTitle: 'Cache insights', cacheHelp: 'Shares use recorded input, including uncached input, cache reads and cache creation.', cacheModels: 'Cache-read share by model', noCache: 'No input usage recorded', viewModels: 'View models',
  snapshotTitle: 'Latest assembled prompts', snapshotHelp: 'The latest actual prompt assembled for each character during this app run. Section token counts are estimates, not API usage or complete context-window occupancy.',
  snapshotEmpty: 'No prompt snapshots yet', snapshotEmptyHelp: 'Actual assembly records appear after a conversation. Template previews are not counted as actual records.', snapshotError: 'Cannot read prompt snapshots',
  estimated: 'Estimated tokens', characters: 'characters', sections: 'sections', sectionSearch: 'Search section names or content', sectionSort: 'Sort sections', size: 'Largest first', original: 'Assembly order', showContent: 'View actual content',
  apiParams: 'Additional request parameters', apiHelp: 'Shown separately; not included in section token estimates.', noSections: 'No matching injected sections', snapshotWhen: 'Assembled', refresh: 'Refresh snapshots', loading: 'Loading snapshots…', retry: 'Retry', notInjected: 'Not injected',
};
const ja: typeof zh = {
  ...en, title: '使用量とコンテキスト分析', usage: 'API 使用量', context: 'Prompt スナップショット', switchView: '分析ビュー',
  activity: '呼び出し履歴', eightWeeks: '直近 8 週間', heatHelp: '日付を選択すると下の使用量全体を絞り込みます', less: '少', more: '多', weekdays: ['月', '水', '金'], clearDay: '期間に戻る',
  activeDays: '記録のある日数', dailyAverage: '1日あたりの記録トークン', perCall: '呼び出しあたりのトークン', peak: '最大使用日', noPeak: '記録なし',
  cacheTitle: 'キャッシュ分析', cacheHelp: '非キャッシュ入力、キャッシュ読取、キャッシュ作成を含む記録済み入力で計算します。', cacheModels: 'モデル別キャッシュ読取割合', noCache: '入力の使用量記録がありません', viewModels: 'モデル詳細',
  snapshotTitle: '最近組み立てた Prompt', snapshotHelp: 'アプリの今回の実行中、各キャラクターが実際に組み立てた最新の Prompt です。分区のトークン数は推定であり、API の実測値やコンテキスト全体の占有率ではありません。',
  snapshotEmpty: 'スナップショットはまだありません', snapshotEmptyHelp: '会話すると実際の組み立て記録が表示されます。テンプレートのプレビューは含みません。', snapshotError: 'スナップショットを読み込めません',
  estimated: '推定トークン', characters: '文字', sections: '分区', sectionSearch: '分区名や内容を検索', sectionSort: '分区の並べ替え', size: 'トークンの多い順', original: '組み立て順', showContent: '実際の内容を表示',
  apiParams: '追加リクエストパラメータ', apiHelp: '分区とは別に表示し、分区のトークン推定には含みません。', noSections: '一致する注入済み分区がありません', snapshotWhen: '組み立て時刻', refresh: 'スナップショットを更新', loading: '読込中…', retry: '再試行', notInjected: '未注入',
};
export const usageInsightsCopy = { zh, en, ja };
export type UsageInsightsCopy = typeof zh;
