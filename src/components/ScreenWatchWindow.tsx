import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { synchronizeWindowLanguage } from '../utils/windowLanguage';
import './ScreenWatchWindow.css';

interface Status {
  id: string; phase: 'ready' | 'watching' | 'paused'; reason: string;
  width: number; height: number;
  sampled: number; kept: number; submitted: number; description: string;
  scope: 'window' | 'desktop';
}
export default function ScreenWatchWindow() {
  const { i18n } = useTranslation();
  const language = i18n.language.startsWith('zh') ? 'zh' : i18n.language.startsWith('ja') ? 'ja' : 'en';
  const copy = {
    zh: { title: '区域陪看', ready: '准备分享', watching: '正在分享选区', paused: '分享已暂停',
      privacy: '仅选区画面将发送给你配置的视觉模型。云端模型按服务商规则处理数据；请避开密码、私聊等敏感内容。',
      limits: '本应用不保存陪看图片或记忆，不采集声音。只按画面变化挑帧，静止时约 20 秒给一张确认帧；单张最多 4 帧。不限时，可随时暂停或停止。',
      boundary: '窗口切到后台、移动、缩放、最小化或隐藏都不会暂停；只有窗口关闭、桌宠离线或视觉被禁用时才停止。内容可能在标题不变时更新，请在不看时主动暂停或停止。',
      scopeLabel: '范围', scopeWindow: '当前窗口', scopeDesktop: '整个桌面',
      desktopNote: '整个桌面（当前显示器）会按变化挑帧发送；本应用窗口已自动排除。其他应用的密码、私聊、通知仍可能进入画面，请先暂停或停止。',
      start: '开始分享选区', resume: '继续分享', pause: '暂停', stop: '停止分享', loading: '正在准备…',
      stats: '采样 / 留帧 / 已提交', idle: '等待选区变化…', changed: '目标窗口已关闭，或桌面显示器发生变化；请停止并重新框选。',
      capture: '当前采集范围无法继续，请停止并重新框选。', model: '视觉模型调用失败，已暂停；请检查模型配置后继续。',
      permission: '陪看已暂停：视觉不可用、权限变化，或陪看的桌宠已离线。请重新选择。',
      expired: '本次分享已结束，请停止并重新框选。', manual: '已暂停并清空待发帧。',
    },
    en: { title: 'Watch region', ready: 'Ready to share', watching: 'Sharing selected region', paused: 'Sharing paused',
      privacy: 'Only the selected region is sent to your configured vision model. Cloud providers handle data under their own policies. Keep passwords and private chats outside the region.',
      limits: 'This app saves no watch images or memories and captures no audio. Frames are selected by visible change; a static view gets one confirmation frame about every 20 seconds, up to 4 frames per sheet. No time limit; pause or stop at any time.',
      boundary: 'Backgrounding, moving, resizing, minimizing, or hiding the window does not pause it; only closing the window, the companion going offline, or vision being disabled stops it. Content can change without a new title: pause or stop when you step away.',
      scopeLabel: 'Scope', scopeWindow: 'This window', scopeDesktop: 'Whole desktop',
      desktopNote: 'The current monitor is sampled by visible change; this app\'s own windows are excluded. Passwords, private chats, and notifications from other apps can still appear, so pause or stop before exposing them.',
      start: 'Start sharing region', resume: 'Resume sharing', pause: 'Pause', stop: 'Stop sharing', loading: 'Preparing…',
      stats: 'Sampled / kept / sent', idle: 'Waiting for changes…', changed: 'The target window was closed, or the desktop display changed. Stop and select again.',
      capture: 'The current capture source cannot continue. Stop and select again.', model: 'Vision request failed. Check your model settings before resuming.',
      permission: 'Watch paused: vision is unavailable, permissions changed, or the selected companion went offline. Select again.',
      expired: 'Sharing ended. Stop and select a new region.', manual: 'Paused; pending frames cleared.',
    },
    ja: { title: 'エリア共有', ready: '共有の準備完了', watching: '選択エリアを共有中', paused: '共有を一時停止',
      privacy: '選択エリアのみ設定済みの画像モデルに送信します。クラウド側は提供元の規約に従います。パスワードや個人チャットを避けてください。',
      limits: '本アプリは共有画像や記憶を保存せず、音声も取得しません。画面の変化に応じてフレームを選び、静止時は約20秒ごとに確認用の1枚、1枚あたり最大4フレーム。時間制限なし。いつでも一時停止・終了できます。',
      boundary: 'ウィンドウが背面・移動・サイズ変更・最小化・非表示になっても停止しません。ウィンドウを閉じた場合、キャラクターがオフライン、または視覚機能が無効になった場合のみ停止します。タイトルが変わらなくても内容は変わるため、見ていないときは停止してください。',
      scopeLabel: '範囲', scopeWindow: 'このウィンドウ', scopeDesktop: 'デスクトップ全体',
      desktopNote: '現在のモニターを変化に応じて送信します。本アプリのウィンドウは自動的に除外されますが、他のアプリのパスワード・個人チャット・通知は映り得ます。必要なら一時停止または終了してください。',
      start: 'エリア共有を開始', resume: '共有を再開', pause: '一時停止', stop: '共有を終了', loading: '準備中…',
      stats: '取得 / 保持 / 送信', idle: '変化を待っています…', changed: '対象ウィンドウが閉じられた、またはデスクトップの表示が変更されました。終了して選択し直してください。',
      capture: '現在の取得範囲を続けられません。終了して選択し直してください。', model: '画像モデルの呼び出しに失敗しました。設定を確認してください。',
      permission: '共有を一時停止しました。視覚機能が無効、権限が変更、または選択したキャラクターがオフラインです。もう一度選択してください。',
      expired: '共有は終了しました。終了して再選択してください。', manual: '一時停止し、未送信画像を破棄しました。',
    },
  }[language];
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const sync = synchronizeWindowLanguage(lng => i18n.changeLanguage(lng));
    const poll = async () => {
      try {
        const next = await invoke<Status>('screen_watch_status');
        if (!disposed) setStatus(next);
      } catch {
        if (!disposed) void getCurrentWindow().close();
      } finally {
        if (!disposed) timer = setTimeout(() => void poll(), 1000);
      }
    };
    void poll();
    return () => { disposed = true; clearTimeout(timer); sync.dispose(); };
  }, []);
  const control = async (action: 'start' | 'pause' | 'stop') => {
    if (!status || busy) return;
    setBusy(true); setError('');
    try {
      await invoke('screen_watch_control', { sessionId: status.id, action });
      if (action !== 'stop') setStatus(await invoke<Status>('screen_watch_status'));
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const reason = !status?.reason ? '' : status.reason === 'manual' ? copy.manual
    : status.reason === 'stopped' ? copy.expired : status.reason === 'model_error' ? copy.model
      : status.reason === 'permission_changed' ? copy.permission
        // Fullscreen capture failure is announced by a toast with a one-click
        // switch action, so the control window does not duplicate it inline.
        : status.reason === 'capture_fullscreen' ? ''
          : status.reason.startsWith('capture_') ? copy.capture : copy.changed;
  return <main className="screen-watch">
    <header><span className={`watch-dot ${status?.phase === 'watching' ? 'is-sharing' : ''}`} />
      <strong>{copy.title}</strong>
    </header>
    <h1>{status ? copy[status.phase] : copy.loading}</h1>
    {status && <div className="watch-dimensions">{status.width} × {status.height} · {copy.scopeLabel} {status.scope === 'desktop' ? copy.scopeDesktop : copy.scopeWindow} · {copy.stats} {status.sampled} / {status.kept} / {status.submitted}</div>}
    <p>{copy.privacy}</p><p className="watch-small">{copy.limits}</p><p className="watch-small">{copy.boundary}</p>
    {status?.scope === 'desktop' && <p className="watch-small watch-desktop">{copy.desktopNote}</p>}
    {reason && <p className="watch-reason" role="status">{reason}</p>}
    {status?.phase === 'watching' && <p className="watch-description" aria-live="polite">{status.description || copy.idle}</p>}
    {error && <p className="watch-error" role="alert">{error}</p>}
    <footer>
      {status?.phase === 'watching' ? <button disabled={busy} onClick={() => void control('pause')}>{copy.pause}</button>
        : <button className="watch-start" disabled={busy || !status || status.reason === 'stopped'} onClick={() => void control('start')}>{status?.phase === 'ready' ? copy.start : copy.resume}</button>}
      <button className="watch-stop" disabled={busy || !status} onClick={() => void control('stop')}>{copy.stop}</button>
    </footer>
  </main>;
}
