import i18n from 'i18next';

const strings = {
  'zh-CN': {
    searchTitle: '搜索会话', searchShortcut: '搜索会话 (Ctrl+K)', searchPlaceholder: '搜索标题、消息或工作区…', searchRecent: '最近会话', searchResults: '搜索结果（{{count}}）', searchNoResults: '没有找到匹配的会话，试试其他关键词', searchNoSessions: '还没有会话', searchKeyboard: '↑ ↓ 选择 · Enter 打开 · Alt+1–9 快速打开',
    errorInfo: '错误信息', retry: '重试',
    attachFiles: '添加文件或图片', dropFiles: '松开以添加文件', dropFilesHint: '文件加入输入区，发送后再阅读；图片可预览',
    editedFiles: '已编辑 {{count}} 个文件', viewChanges: '查看变更', collapseFiles: '收起文件', expandFiles: '展开文件',
    navigation: '页面导航', inspector: '房间', focus: '专注阅读', focusExit: '退出专注阅读',
    dismiss: '关闭面板', details: '运行详情', exampleProject: '解释这个项目', exampleChanges: '检查最近改动', exampleFix: '修复一个问题',
    promptProject: '请介绍这个项目的结构、主要模块和运行方式。', promptChanges: '请检查当前工作区的最近改动，找出可能的问题并说明原因。', promptFix: '请帮我定位并修复这个问题：',
    failed: '有失败步骤', stopped: '已停止', pending: '等待结果', currentTool: '当前：{{tool}}', settings: '输入设置', summarySpace: '展开摘要需要更宽的阅读空间',
  },
  en: {
    searchTitle: 'Search chats', searchShortcut: 'Search chats (Ctrl+K)', searchPlaceholder: 'Search titles, messages or workspaces…', searchRecent: 'Recent chats', searchResults: 'Results ({{count}})', searchNoResults: 'No matching chats. Try another keyword.', searchNoSessions: 'No chats yet', searchKeyboard: '↑ ↓ Select · Enter Open · Alt+1–9 Quick open',
    errorInfo: 'Error details', retry: 'Retry',
    attachFiles: 'Attach files or images', dropFiles: 'Drop to attach files', dropFilesHint: 'Files are added to the composer; images can be previewed',
    editedFiles: '{{count}} edited files', viewChanges: 'View changes', collapseFiles: 'Collapse files', expandFiles: 'Expand files',
    navigation: 'Page navigation', inspector: 'Room', focus: 'Focus reading', focusExit: 'Exit focus reading',
    dismiss: 'Close panel', details: 'Run details', exampleProject: 'Explain this project', exampleChanges: 'Review recent changes', exampleFix: 'Fix a problem',
    promptProject: 'Explain the structure, main modules, and how to run this project.', promptChanges: 'Review recent changes in this workspace and explain any potential problems.', promptFix: 'Help me diagnose and fix this problem: ',
    failed: 'Some steps failed', stopped: 'Stopped', pending: 'Awaiting result', currentTool: 'Current: {{tool}}', settings: 'Composer settings', summarySpace: 'Widen the reading area to show the summary',
  },
  ja: {
    searchTitle: '会話を検索', searchShortcut: '会話を検索 (Ctrl+K)', searchPlaceholder: 'タイトル・メッセージ・作業領域を検索…', searchRecent: '最近の会話', searchResults: '検索結果（{{count}}）', searchNoResults: '一致する会話がありません。別のキーワードをお試しください。', searchNoSessions: '会話はまだありません', searchKeyboard: '↑ ↓ 選択 · Enter 開く · Alt+1–9 クイック選択',
    errorInfo: 'エラー情報', retry: '再試行',
    attachFiles: 'ファイルや画像を添付', dropFiles: 'ドロップしてファイルを添付', dropFilesHint: '入力欄に添付します。画像はプレビューできます',
    editedFiles: '{{count}} ファイルを編集', viewChanges: '変更を表示', collapseFiles: 'ファイルを折りたたむ', expandFiles: 'ファイルを展開',
    navigation: 'ページナビゲーション', inspector: '部屋', focus: '集中表示', focusExit: '集中表示を終了',
    dismiss: 'パネルを閉じる', details: '実行の詳細', exampleProject: 'プロジェクトを説明', exampleChanges: '最近の変更を確認', exampleFix: '問題を修正',
    promptProject: 'このプロジェクトの構成、主なモジュール、実行方法を説明してください。', promptChanges: '現在のワークスペースの最近の変更を確認し、潜在的な問題と理由を説明してください。', promptFix: '次の問題の原因を調べて修正してください：',
    failed: '失敗したステップあり', stopped: '停止済み', pending: '結果待ち', currentTool: '現在：{{tool}}', settings: '入力設定', summarySpace: '概要を表示するには閲覧領域を広げてください',
  },
};

for (const [language, values] of Object.entries(strings)) {
  i18n.addResourceBundle(language, 'translation', { workbench: values }, true, false);
}
