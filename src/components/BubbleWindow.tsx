/**
 * 气泡子窗口 - 在独立的 Tauri 窗口中渲染对话气泡。
 *
 * 主窗口通过 Tauri 事件驱动本窗口：
 * - `bubble:show`         显示活跃气泡（携带 text / position / duration）
 * - `bubble:update`       更新活跃气泡文本（流式场景）
 * - `bubble:hide`         隐藏活跃气泡
 * - `bubble:settled_add`  添加已结算气泡段（独立显示，不替换活跃气泡）
 * - `bubble:settled_remove` 移除已过期的已结算气泡段
 *
 * 本窗口自身透明、无边框、跳过任务栏、始终置顶，
 * 由主窗口负责定位（贴合主窗口上方/下方）。
 * 气泡内容根据 position 在窗口内贴底或贴顶渲染，
 * 使小尾巴始终指向桌宠方向。
 *
 * 多气泡布局：已结算气泡段堆叠在活跃气泡上方（position='top'）
 * 或下方（position='bottom'），各自独立淡出关闭。
 */

import { useEffect, useState, useCallback } from 'react';
import { listen, emit } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import MessageBubble, { type BubblePosition } from './MessageBubble';

interface BubbleShowPayload {
  text: string;
  position: BubblePosition;
  duration: number;
  character_id?: string;
  cross_character?: boolean;
  listener_name?: string;
}

interface BubbleUpdatePayload {
  text: string;
  character_id?: string;
  cross_character?: boolean;
  listener_name?: string;
}

interface BubbleHidePayload {
  character_id?: string;
}

interface SettledAddPayload {
  id: number;
  text: string;
  duration: number;
  character_id?: string;
}

interface SettledRemovePayload {
  id: number;
  character_id?: string;
}

interface SettledBubbleEntry {
  id: number;
  text: string;
  duration: number;
}

export default function BubbleWindow() {
  const [text, setText] = useState<string>('');
  const [position, setPosition] = useState<BubblePosition>('bottom');
  const [duration, setDuration] = useState<number>(0);
  const [visible, setVisible] = useState<boolean>(false);
  const [settledBubbles, setSettledBubbles] = useState<SettledBubbleEntry[]>([]);
  const [crossCharacter, setCrossCharacter] = useState<boolean>(false);
  const [listenerName, setListenerName] = useState<string | null>(null);

  const params = new URLSearchParams(window.location.search);
  const myCharId = params.get('character_id') ?? '';

  const removeSettled = useCallback((id: number) => {
    setSettledBubbles((prev) => prev.filter((b) => b.id !== id));
  }, []);

  useEffect(() => {
    let cancelled = false;
    const unlistens: Array<() => void> = [];

    // 气泡窗口一直穿透：不拦截鼠标事件，不影响下方 Live2D 窗口的交互
    void getCurrentWindow().setIgnoreCursorEvents(true).catch(() => {});

    void (async () => {
      const un1 = await listen<BubbleShowPayload>('bubble:show', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== myCharId) return;
        setText(e.payload.text);
        setPosition(e.payload.position);
        setDuration(e.payload.duration);
        setCrossCharacter(!!e.payload.cross_character);
        setListenerName(e.payload.listener_name ?? null);
        setVisible(true);
      });
      if (cancelled) { await un1(); return; }
      unlistens.push(un1);

      const un2 = await listen<BubbleUpdatePayload>('bubble:update', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== myCharId) return;
        setText(e.payload.text);
        if (e.payload.cross_character !== undefined) setCrossCharacter(!!e.payload.cross_character);
        if (e.payload.listener_name !== undefined) setListenerName(e.payload.listener_name ?? null);
      });
      if (cancelled) { await un2(); return; }
      unlistens.push(un2);

      const un3 = await listen<BubbleHidePayload>('bubble:hide', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== myCharId) return;
        setVisible(false);
        setText('');
        setSettledBubbles([]);
        setCrossCharacter(false);
        setListenerName(null);
      });
      if (cancelled) { await un3(); return; }
      unlistens.push(un3);

      // 已结算气泡段：添加（从流式气泡中分离的已完成段落）
      const un4 = await listen<SettledAddPayload>('bubble:settled_add', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== myCharId) return;
        setSettledBubbles((prev) => [
          ...prev,
          { id: e.payload.id, text: e.payload.text, duration: e.payload.duration },
        ]);
      });
      if (cancelled) { await un4(); return; }
      unlistens.push(un4);

      // 已结算气泡段：移除（到期自动关闭）
      const un5 = await listen<SettledRemovePayload>('bubble:settled_remove', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== myCharId) return;
        removeSettled(e.payload.id);
      });
      if (cancelled) { await un5(); return; }
      unlistens.push(un5);

      void emit('bubble:ready', { character_id: myCharId });
    })();

    return () => {
      cancelled = true;
      for (const un of unlistens) un();
    };
  }, [myCharId, removeSettled]);

  const hasContent = visible && text;
  const hasSettled = settledBubbles.length > 0;

  if (!hasContent && !hasSettled) return null;

  // position='top'：气泡在桌宠上方，尾巴朝下
  //   已结算气泡堆叠在上方（更早），活跃气泡在底部（最靠近桌宠）
  // position='bottom'：气泡在桌宠下方，尾巴朝上
  //   已结算气泡堆叠在上方（更早），活跃气泡在底部（最靠近桌宠）
  const containerStyle: React.CSSProperties = {
    position: 'absolute',
    left: 0,
    right: 0,
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'center',
    gap: 8,
    padding: 8,
  };

  // top 模式：从窗口底部开始排列（活跃气泡贴底，已结算气泡在上方）
  // bottom 模式：从窗口顶部开始排列（已结算气泡在上，活跃气泡贴顶/靠近桌宠）
  if (position === 'top') {
    containerStyle.bottom = 0;
  } else {
    containerStyle.top = 0;
  }

  // 已结算气泡在前（堆叠在上层），活跃气泡在后（最靠近桌宠）
  const settledNodes = settledBubbles.map((b) => (
    <MessageBubble
      key={b.id}
      text={b.text}
      duration={b.duration}
      position={position}
      characterId={myCharId}
    />
  ));

  const activeNode = hasContent ? (
    <MessageBubble
      text={text}
      duration={duration}
      position={position}
      characterId={myCharId}
      crossCharacter={crossCharacter}
      listenerName={listenerName ?? undefined}
    />
  ) : null;

  return (
    <div style={{ position: 'fixed', inset: 0, overflow: 'hidden', background: 'transparent' }}>
      <div style={containerStyle}>
        {settledNodes}
        {activeNode}
      </div>
    </div>
  );
}
