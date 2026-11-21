import { useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { emit } from '@tauri-apps/api/event';
import { BriefcaseBusiness, MessageCircle, Moon, NotebookPen, Pin, Rocket, ScanLine, Move, ArrowDown } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import './ChatEdgeMenu.css';

export type EdgeMenuAction = 'screen' | 'notes' | 'shortcuts' | 'chat' | 'office' | 'dnd' | 'movement_mode';
/** 与后端 `desktop_menu_policy::MovementMode::id()` 一一对应。 */
export type MovementMode = 'gravity' | 'avoid' | 'still';
interface Props {
  edge: { mode: 'menu' | 'folding'; width: number };
  target: RefObject<HTMLButtonElement>;
  onAction: (action: EdgeMenuAction) => Promise<void>;
  onFoldEnd: () => void;
  quiet?: boolean;
  movement?: MovementMode;
}

export default function ChatEdgeMenu({ edge, target, onAction, onFoldEnd, quiet = false, movement = 'still' }: Props) {
  const rail = useRef<HTMLDivElement>(null);
  const busyRef = useRef(false);
  const [busy, setBusy] = useState(false);
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  // 避让 / 重力 / 静止合成一个三态按钮：图标与文案始终反映当前态，
  // tooltip 提示下一次点击会切到哪里（与后端 cycle_movement 的顺序一致）。
  const movementStates = {
    gravity: { icon: ArrowDown, text: label('重力', '重力', 'Gravity'), title: label('重力 · 点击切换为避让', '重力 · クリックで自動回避', 'Gravity · click to switch to Avoid') },
    avoid: { icon: Move, text: label('避让', '自動回避', 'Avoid'), title: label('避让 · 点击切换为静止', '自動回避 · クリックで静止', 'Avoid · click to switch to Still') },
    still: { icon: Pin, text: label('静止', '静止', 'Still'), title: label('静止 · 点击切换为重力', '静止 · クリックで重力', 'Still · click to switch to Gravity') },
  } as const;
  const currentMovement = movementStates[movement];
  const items = [
    { id: 'chat', icon: MessageCircle, text: label('手机', 'スマホ', 'Phone'), title: label('打开聊天窗口', 'チャットを開く', 'Open chat window'), active: false, pressed: undefined },
    { id: 'screen', icon: ScanLine, text: label('截屏', '画面', 'Scan'), title: label('截屏分析', '画面分析', 'Analyze screen'), active: false, pressed: undefined },
    { id: 'office', icon: BriefcaseBusiness, text: label('办公', '仕事', 'Work'), title: label('办公', '仕事', 'Work'), active: false, pressed: undefined },
    { id: 'dnd', icon: Moon, text: label('勿扰', '集中', 'Quiet'), title: label('勿扰', '集中', 'Quiet'), active: quiet, pressed: quiet },
    { id: 'movement_mode', icon: currentMovement.icon, text: currentMovement.text, title: currentMovement.title, active: movement !== 'still', pressed: movement !== 'still' },
    { id: 'notes', icon: NotebookPen, text: label('随手记', 'メモ', 'Notes'), title: label('随手记', 'メモ', 'Notes'), active: false, pressed: undefined },
    { id: 'shortcuts', icon: Rocket, text: label('启动', '起動', 'Launch'), title: label('快捷启动', 'クイック起動', 'Quick launch'), active: false, pressed: undefined },
  ] as const;

  useLayoutEffect(() => {
    if (edge.mode !== 'folding' || !rail.current) return;
    const element = rail.current;
    const from = element.getBoundingClientRect();
    const to = target.current?.getBoundingClientRect();
    const x = (to ? to.left + to.width / 2 : window.innerWidth - 26) - (from.left + from.width / 2);
    const y = (to ? to.top + to.height / 2 : 78) - (from.top + from.height / 2);
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      onFoldEnd();
      return;
    }
    const animation = element.animate([
      { transform: 'translate(0, 0) scale(1, 1)', opacity: 1, borderRadius: '22px' },
      { transform: `translate(${x}px, ${y}px) scale(${28 / from.width}, ${28 / from.height})`, opacity: 0, borderRadius: '50px' },
    ], { duration: 420, easing: 'cubic-bezier(.22, 1, .36, 1)', fill: 'forwards' });
    let active = true;
    void animation.finished.then(() => {
      if (!active) return;
      // A brief pulse makes the destination of the disappearing menu clear.
      target.current?.animate([{ background: 'var(--wx-bg-active)' }, { background: 'transparent' }], { duration: 260 });
      onFoldEnd();
    }).catch(() => {});
    return () => { active = false; animation.cancel(); };
  }, [edge.mode, target, onFoldEnd]);

  const run = async (action: EdgeMenuAction) => {
    // Recognition can take a while; keep navigation available during that request.
    if (action !== 'screen') {
      try { await onAction(action); }
      catch (error) { void emit('toast:show', { message: String(error), type: 'error', duration: 4000, key: Date.now() }); }
      return;
    }
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true);
    try { await onAction(action); }
    catch (error) { void emit('toast:show', { message: String(error), type: 'error', duration: 4000, key: Date.now() }); }
    finally { busyRef.current = false; setBusy(false); }
  };
  return <div className="chat-edge-layer" style={{ width: edge.width }}>
    <div ref={rail} className={`chat-edge-rail${edge.mode === 'folding' ? ' is-folding' : ''}`} role="toolbar" aria-label={label('桌面快捷菜单', 'クイックメニュー', 'Desktop shortcuts')} aria-orientation="vertical">
      {items.map(({ id, icon: Icon, text, title, active, pressed }) => <button key={id} type="button" className={`chat-edge-button${active ? ' is-active' : ''}`} title={title} aria-label={title} aria-pressed={pressed} disabled={(busy && id === 'screen') || edge.mode !== 'menu'} onClick={() => void run(id)}>
        <Icon size={20} strokeWidth={1.8} /><span>{text}</span>
      </button>)}
    </div>
  </div>;
}
