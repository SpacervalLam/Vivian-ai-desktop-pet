import { NumberField, TextField, ToggleField, sectionTitleStyle } from './SettingsFields';

type SettingValue = boolean | number | string[];
export default function CompanionSettings({ language, get, set }: {
  language: string;
  get: <T extends SettingValue>(key: string, fallback: T) => T;
  set: (key: string, value: SettingValue) => void;
}) {
  const zh = language.startsWith('zh'), ja = language.startsWith('ja');
  const text = (cn: string, jp: string, en: string) => zh ? cn : ja ? jp : en;
  const toggles: Array<[string, boolean, string]> = [
    ['clipboard_hint', false, text('剪贴板变化图标（不读取正文）', 'クリップボード更新アイコン', 'Clipboard change icon (no text read)')],
    ['weather_feedback', false, text('天气变化提醒', '天気変化のお知らせ', 'Weather change feedback')],
    ['music_feedback', false, text('曲目变化陪伴', '曲の変化をお知らせ', 'Music change companionship')],
    ['focus_mode', false, text('专注模式', '集中モード', 'Focus mode')],
    ['quiet_hours', false, text('夜间免打扰', '夜間の通知オフ', 'Quiet hours')],
    ['game_quiet', true, text('前台游戏免打扰', 'ゲーム中の通知オフ', 'Quiet during foreground games')],
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
    <TextField label={text('游戏进程名（用英文逗号分隔）', 'ゲームのプロセス名（カンマ区切り）', 'Game process names (comma separated)')}
      value={get('companion.game_processes', ['League of Legends.exe', 'GenshinImpact.exe']).join(',')}
      onChange={value => set('companion.game_processes', value.split(','))} />
  </>;
}
