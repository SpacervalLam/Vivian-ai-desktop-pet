import i18n from 'i18next';

const strings = {
  'zh-CN': {
    view: '工作流视图', flow: '执行链路', events: '事件明细',
    title: '工作流', turns: '对话轮次', requests: '模型请求', toolsCount: '工具调用', llmTime: '模型累计耗时',
    empty: '发送一条消息后，在这里查看执行链路。', noCapture: '此轮没有请求快照。启用工作流后的新请求才会记录详情。',
    turn: '第 {{n}} 轮', selectTurn: '选择对话轮次', request: '请求', response: '响应', tool: '工具',
    running: '进行中', complete: '已完成', error: '失败', interrupted: '已中断', waiting: '等待请求',
    assistant: '任务执行', routing: '模式选择', summary: '中断总结', code: '程序生成', attempt: '第 {{n}} 次尝试',
    system: '系统提示词', messages: '消息历史', tools: '工具定义', reasoning: '推理偏好', arguments: '调用参数', result: '执行结果',
    snapshot: '智能体请求快照', captureNote: '记录于路由器处理之前；提供方转换、框架注入和路由回退可能改变最终请求或模型。模型名称是配置值。',
    input: '总输入', uncached: '未缓存输入', cacheRead: '缓存读取', cacheWrite: '缓存写入', output: '输出', unknownUsage: '模型未返回用量',
    inspect: '查看详情', close: '关闭', copy: '复制 JSON', copied: '已复制', loading: '正在读取…', retry: '重试',
    more: '再显示 {{n}} 项', expand: '展开全文', copyFailed: '复制失败：{{error}}', orphanTools: '其他已记录工具',
    showTurns: '展开轮次列表', hideTurns: '收起轮次列表', noResult: '尚未记录结果', requestFailed: '快照读取失败：{{error}}',
  },
  en: {
    view: 'Workflow view', flow: 'Execution flow', events: 'Event details',
    title: 'Workflow', turns: 'Turns', requests: 'Model requests', toolsCount: 'Tool calls', llmTime: 'Total model time',
    empty: 'Send a message to see its execution flow here.', noCapture: 'No request snapshot for this turn. New requests are captured after Workflow is enabled.',
    turn: 'Turn {{n}}', selectTurn: 'Select a turn', request: 'Request', response: 'Response', tool: 'Tool',
    running: 'Running', complete: 'Complete', error: 'Failed', interrupted: 'Interrupted', waiting: 'Awaiting request',
    assistant: 'Task', routing: 'Mode selection', summary: 'Interruption summary', code: 'Program generation', attempt: 'Attempt {{n}}',
    system: 'System prompts', messages: 'Message history', tools: 'Tool definitions', reasoning: 'Reasoning preference', arguments: 'Arguments', result: 'Result',
    snapshot: 'Agent request snapshot', captureNote: 'Captured before router processing. Provider conversion, framework injection and fallback may change the final request or model. The model label is the configured value.',
    input: 'Total input', uncached: 'Uncached input', cacheRead: 'Cache read', cacheWrite: 'Cache write', output: 'Output', unknownUsage: 'Provider did not report usage',
    inspect: 'Inspect', close: 'Close', copy: 'Copy JSON', copied: 'Copied', loading: 'Loading…', retry: 'Retry',
    more: 'Show {{n}} more', expand: 'Show full text', copyFailed: 'Copy failed: {{error}}', orphanTools: 'Other recorded tools',
    showTurns: 'Show turn list', hideTurns: 'Hide turn list', noResult: 'No result recorded yet', requestFailed: 'Snapshot could not be read: {{error}}',
  },
  ja: {
    view: 'ワークフロー表示', flow: '実行フロー', events: 'イベント詳細',
    title: 'ワークフロー', turns: 'ターン', requests: 'モデルリクエスト', toolsCount: 'ツール呼び出し', llmTime: 'モデル累積時間',
    empty: 'メッセージを送ると実行の流れが表示されます。', noCapture: 'このターンにはリクエスト記録がありません。有効化後の新しいリクエストが記録されます。',
    turn: 'ターン {{n}}', selectTurn: 'ターンを選択', request: 'リクエスト', response: '応答', tool: 'ツール',
    running: '実行中', complete: '完了', error: '失敗', interrupted: '中断', waiting: 'リクエスト待ち',
    assistant: 'タスク実行', routing: 'モード選択', summary: '中断のまとめ', code: 'プログラム生成', attempt: '試行 {{n}}',
    system: 'システムプロンプト', messages: 'メッセージ履歴', tools: 'ツール定義', reasoning: '推論設定', arguments: '引数', result: '実行結果',
    snapshot: 'エージェントのリクエスト記録', captureNote: 'ルーター処理前の記録です。提供元の変換・フレームワークの追加・フォールバックにより最終リクエストやモデルが変わる場合があります。モデル名は設定値です。',
    input: '総入力', uncached: '非キャッシュ入力', cacheRead: 'キャッシュ読取', cacheWrite: 'キャッシュ書込', output: '出力', unknownUsage: '提供元から使用量の報告がありません',
    inspect: '詳細', close: '閉じる', copy: 'JSON をコピー', copied: 'コピー済み', loading: '読込中…', retry: '再試行',
    more: 'さらに {{n}} 件', expand: '全文を表示', copyFailed: 'コピー失敗：{{error}}', orphanTools: 'その他のツール記録',
    showTurns: 'ターン一覧を表示', hideTurns: 'ターン一覧を閉じる', noResult: '結果はまだ記録されていません', requestFailed: '記録の読込失敗：{{error}}',
  },
};
for (const [language, values] of Object.entries(strings)) {
  i18n.addResourceBundle(language, 'translation', { workflow: values }, true, false);
}
