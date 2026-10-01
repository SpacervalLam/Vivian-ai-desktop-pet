import i18n from 'i18next';

const strings = {
  'zh-CN': {
    errorInfo: '错误信息', retry: '重试',
    attachFiles: '添加文件或图片', dropFiles: '松开以添加文件', dropFilesHint: '文件加入输入区，发送后再阅读；图片可预览',
    editedFiles: '已编辑 {{count}} 个文件', viewChanges: '查看变更', collapseFiles: '收起文件', expandFiles: '展开文件',
    navigation: '页面导航', inspector: '房间', focus: '专注阅读', focusExit: '退出专注阅读',
    dismiss: '关闭面板', details: '运行详情', exampleProject: '解释这个项目', exampleChanges: '检查最近改动', exampleFix: '修复一个问题',
    promptProject: '请介绍这个项目的结构、主要模块和运行方式。', promptChanges: '请检查当前工作区的最近改动，找出可能的问题并说明原因。', promptFix: '请帮我定位并修复这个问题：',
    failed: '有失败步骤', stopped: '已停止', pending: '等待结果', currentTool: '当前：{{tool}}', settings: '输入设置', summarySpace: '展开摘要需要更宽的阅读空间',
  },
  en: {
    errorInfo: 'Error details', retry: 'Retry',
    attachFiles: 'Attach files or images', dropFiles: 'Drop to attach files', dropFilesHint: 'Files are added to the composer; images can be previewed',
    editedFiles: '{{count}} edited files', viewChanges: 'View changes', collapseFiles: 'Collapse files', expandFiles: 'Expand files',
    navigation: 'Page navigation', inspector: 'Room', focus: 'Focus reading', focusExit: 'Exit focus reading',
    dismiss: 'Close panel', details: 'Run details', exampleProject: 'Explain this project', exampleChanges: 'Review recent changes', exampleFix: 'Fix a problem',
    promptProject: 'Explain the structure, main modules, and how to run this project.', promptChanges: 'Review recent changes in this workspace and explain any potential problems.', promptFix: 'Help me diagnose and fix this problem: ',
    failed: 'Some steps failed', stopped: 'Stopped', pending: 'Awaiting result', currentTool: 'Current: {{tool}}', settings: 'Composer settings', summarySpace: 'Widen the reading area to show the summary',
  },
  ja: {
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
