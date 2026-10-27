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
    ['resource_feedback', true, text('持续高负载提醒', '高負荷のお知らせ', 'Sustained resource pressure feedback')],
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
    <ToggleField label={text('游戏陪伴', 'ゲームに寄り添う', 'Game companionship')}
      value={get('companion.game_companion', true)} onChange={value => set('companion.game_companion', value)} />
    <p>{text('自动识别并记住游戏，下次进入游戏时自动陪伴。游戏中减少无关打扰和演出，桌宠退到边缘并避开鼠标操作区域。环境智能体统计键鼠操作强度，激烈操作及短暂冷却期间暂缓主动插话。启用视觉功能后，最多每分钟分析一次游戏窗口画面；不保存具体按键内容或截图。',
      'ゲームを自動判別して記憶し、次回から自動で寄り添います。不要な演出を抑え、操作を避けて画面端に移動します。環境エージェントがキー・マウス操作の頻度を集計し、激しい操作と直後の間は声掛けを控えます。視覚機能が有効な場合は最大毎分1回画面を分析します。キーの内容や画像は保存しません。',
      'Automatically recognizes and remembers games. Reduces unrelated effects and moves the pet away from your cursor. The environment agent estimates keyboard/mouse activity and pauses unsolicited comments during intense input and a short cooldown. With vision enabled, analyzes the game window at most once a minute. Specific keystrokes and screenshots are not stored.')}</p>
  </>;
}
