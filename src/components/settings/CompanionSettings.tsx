import { NumberField, ToggleField, sectionTitleStyle } from './SettingsFields';

type SettingValue = boolean | number | string[];
export default function CompanionSettings({ language, get, set }: {
  language: string;
  get: <T extends SettingValue>(key: string, fallback: T) => T;
  set: (key: string, value: SettingValue) => void;
}) {
  const zh = language.startsWith('zh'), ja = language.startsWith('ja');
  const text = (cn: string, jp: string, en: string) => zh ? cn : ja ? jp : en;
  const toggles: Array<[string, boolean, string]> = [
    ['clipboard_hint', true, text('分享剪贴板按钮（点击才读取并分享文本）', 'クリップボード共有ボタン（クリック時のみテキストを共有）', 'Share clipboard button (read and share text only on click)')],
    ['weather_feedback', false, text('天气变化提醒', '天気変化のお知らせ', 'Weather change feedback')],
    ['music_feedback', false, text('曲目变化陪伴', '曲の変化をお知らせ', 'Music change companionship')],
    ['focus_mode', false, text('专注模式', '集中モード', 'Focus mode')],
    ['quiet_hours', false, text('夜间免打扰', '夜間の通知オフ', 'Quiet hours')],
    ['hourly_chime', false, text('整点报时', '毎時のお知らせ', 'Hourly chime')],
    ['sound', true, text('报时与提醒提示音', '時報・リマインダーの音', 'Chime and reminder sounds')],
    ['network_feedback', false, text('断网与恢复提醒', 'ネット切断・復帰のお知らせ', 'Network disconnect and recovery feedback')],
  ];
  return <>
    <div style={{ ...sectionTitleStyle, marginTop: 24 }}>{text('陪伴节奏', '通知のタイミング', 'Companion timing')}</div>
    <p>{text('免打扰暂停普通主动插话、报时及普通日程提醒；重要提醒仍会出现。手动对话始终可用。',
      '通知オフ中は通常の声掛けと時報・予定通知を保留します。重要通知と手動チャットは利用できます。',
      'Quiet mode holds ordinary proactive messages, chimes and reminders. Important reminders and manual chat remain available.')}</p>
    {toggles.map(([key, fallback, label]) => <ToggleField key={key} label={label}
      value={get(`companion.${key}`, fallback)} onChange={value => set(`companion.${key}`, value)} />)}
    {get('companion.quiet_hours', false) && <>
      <NumberField label={text('开始时间（小时）', '開始時刻', 'Start hour')} min={0} max={23} step={1}
        value={get('companion.quiet_start', 23)} onChange={value => set('companion.quiet_start', Math.min(23, Math.max(0, Math.round(value))))} />
      <NumberField label={text('结束时间（小时）', '終了時刻', 'End hour')} min={0} max={23} step={1}
        value={get('companion.quiet_end', 7)} onChange={value => set('companion.quiet_end', Math.min(23, Math.max(0, Math.round(value))))} />
    </>}
    <div style={{ ...sectionTitleStyle, marginTop: 24 }}>{text('环境感知来源', '環境情報の取得元', 'Environment information sources')}</div>
    <p>{text('后台自动观测交给环境智能体；桌宠主动或你指定桌宠的截图分析，直接回复该桌宠。采集本身不要求桌宠发言。当前应用信息读取失败或过期时保持未知；截图沿用现有视觉与权限设置，剪贴板正文可单独开启。',
      'バックグラウンドの観測は環境エージェントへ渡します。ペット自身が求めた画像分析や、指定したペットへの分析結果はそのペットに直接返します。取得だけで声掛けは発生せず、取得不能・期限切れは不明として扱います。画像は既存の視覚設定に従い、クリップボード本文は個別に有効化できます。',
      'Background observations go to the environment agent. Screenshot analysis requested by or addressed to a pet returns directly to that pet. Collection alone does not require speech; unavailable or expired app information stays unknown. Images follow existing vision permissions; clipboard text is a separate option.')}</p>
    {([
      ['enabled', true, text('启用扩展环境感知', '環境情報の拡張', 'Extended environment perception')],
      ['foreground_text', true, text('当前应用的文档、可读文本与选中内容', '現在のアプリの文書・テキスト・選択範囲', 'Current app documents, readable text and selection')],
      ['browser_context', true, text('当前浏览器地址栏（不读取浏览历史）', '現在のブラウザのアドレス欄（履歴は取得しない）', 'Current browser address bar (no browsing history)')],
      ['visible_windows', true, text('桌面可见窗口信息', '表示中のウィンドウ情報', 'Visible desktop window information')],
      ['clipboard_text', false, text('自动读取剪贴板正文', 'クリップボード本文を自動取得', 'Automatically read clipboard text')],
      ['power_display', true, text('电量、电源与显示器状态', '電池・電源・ディスプレイの状態', 'Battery, power and display status')],
      ['system_resources', true, text('系统资源情况供环境智能体参考（不直接提醒）', 'システム資源を環境エージェントへ（直接通知しない）', 'System resources for environment context (no direct reminders)')],
    ] as Array<[string, boolean, string]>).map(([key, fallback, label]) => <ToggleField key={key} label={label}
      value={get(`world.perception.${key}`, fallback)} onChange={value => set(`world.perception.${key}`, value)} />)}
    <NumberField label={text('窗口文本采样间隔（秒）', 'ウィンドウテキストの取得間隔（秒）', 'Window text sampling interval (seconds)')} min={10} max={120} step={5}
      value={get('world.perception.sample_seconds', 30)} onChange={value => set('world.perception.sample_seconds', Math.min(120, Math.max(10, Math.round(value))))} />
    <p>{text('默认不读取密码框或密码管理器的前台正文；不会采集具体按键，也不会保存新的截图文件。',
      'パスワード欄やパスワード管理アプリの本文は既定で取得しません。キーの内容を収集せず、新しい画像ファイルも保存しません。',
      'Password fields and password-manager foreground content are excluded by default. Specific keystrokes are not collected, and no new screenshot files are saved.')}</p>
    <ToggleField label={text('游戏陪伴', 'ゲームに寄り添う', 'Game companionship')}
      value={get('companion.game_companion', true)} onChange={value => set('companion.game_companion', value)} />
    <p>{text('自动识别并记住游戏，下次进入游戏时自动陪伴。游戏中减少无关打扰和演出，桌宠退到边缘并避开鼠标操作区域。环境智能体统计键鼠操作强度，激烈操作及短暂冷却期间暂缓主动插话。启用视觉功能后，最多每分钟分析一次游戏窗口画面；不保存具体按键内容或截图。',
      'ゲームを自動判別して記憶し、次回から自動で寄り添います。不要な演出を抑え、操作を避けて画面端に移動します。環境エージェントがキー・マウス操作の頻度を集計し、激しい操作と直後の間は声掛けを控えます。視覚機能が有効な場合は最大毎分1回画面を分析します。キーの内容や画像は保存しません。',
      'Automatically recognizes and remembers games. Reduces unrelated effects and moves the pet away from your cursor. The environment agent estimates keyboard/mouse activity and pauses unsolicited comments during intense input and a short cooldown. With vision enabled, analyzes the game window at most once a minute. Specific keystrokes and screenshots are not stored.')}</p>
  </>;
}
