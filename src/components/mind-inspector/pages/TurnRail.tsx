/**
 * 对话定位轨（TurnRail）—— 贴在工作区左侧的一列小横杠，一轮对话一根。
 *
 * 交互：
 * - 默认：每轮一根灰色短横杠；当前视口停留的那轮为黑色略长的横杠
 * - 悬停：被指的那根变黑加长，上下邻近横杠按距离衰减变短（视觉"凸起"）
 * - 悬停呼出侧边预览：首行用户消息 + 其后智能体的最终总结（两者截断策略不同）
 * - 点击：平滑滚动定位到该轮开头
 *
 * 主题：预览卡默认是手账便签（暖纸 + 虚线 + 纸胶带），极简主题下由
 * `.mind-main[data-ui-style="minimal"]` 覆写成素卡片（见 CodeAgentPage.css）。
 *
 * **性能**：滚动驱动的状态全部收敛在本组件内部（自己监听 scrollRef 的滚动），
 * 只重渲染这一列横杠 —— 不把 activeTurn 提到父级，否则每次滚动都会重渲染整个消息列表。
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

/** 一轮对话：一条用户消息 + 其后到下一轮之前的全部内容。 */
export interface TurnRailItem {
  /** 该轮用户消息在 messages 中的下标，同时用作滚动锚点的标识 */
  index: number;
  /** 用户消息（已压平成单行） */
  user: string;
  /** 该轮智能体的最终总结（已转纯文本，可能为空） */
  summary: string;
  /** 该轮仍在进行（没有总结） */
  pending: boolean;
}

/** 首行预览截断：单行展示，太长了截断交给 CSS ellipsis，这里只做防御性上限。 */
const USER_MAX_CHARS = 140;
/** 总结预览截断：3 行左右，故按码点截到约 3 行的量。 */
const SUMMARY_MAX_CHARS = 320;

/** 按码点安全截断（避免把代理对切成两半），超出加省略号。 */
function clip(text: string, max: number): string {
  const chars = [...text];
  return chars.length > max ? `${chars.slice(0, max).join('')}…` : text;
}

/**
 * Markdown → 预览用纯文本。
 *
 * 预览卡只有 3 行空间，原样塞 Markdown 会被语法符号占掉大半（`##`、`**`、围栏里的代码），
 * 所以先把结构标记剥掉、把代码块整段丢弃 —— 代码在预览里没有信息量。
 */
function plainText(src: string): string {
  return src
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^\s{0,3}#{1,6}\s+/gm, '')
    .replace(/^\s{0,3}>\s?/gm, '')
    .replace(/^\s{0,3}[-*+]\s+/gm, '')
    .replace(/^\s{0,3}\d+\.\s+/gm, '')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/__([^_]+)__/g, '$1')
    .replace(/(^|[^*])\*([^*]+)\*/g, '$1$2')
    .replace(/^\s{0,3}([-*_])\1{2,}\s*$/gm, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

/**
 * 把消息切成「轮」。
 *
 * 「智能体的最终总结」取该轮**最后一条有正文的 assistant 消息**：一轮里助手可能先发一句
 * "我先看看代码" 再调工具、最后才给总结，取最后一条才是结论。
 */
export function buildTurns(
  messages: { role: string; content: string }[],
  running: boolean,
): TurnRailItem[] {
  const out: TurnRailItem[] = [];
  let current: TurnRailItem | null = null;
  messages.forEach((m, i) => {
    if (m.role === 'user') {
      if (current) out.push(current);
      current = {
        index: i,
        user: clip(plainText(m.content), USER_MAX_CHARS),
        summary: '',
        pending: true,
      };
      return;
    }
    if (current && m.role === 'assistant') {
      const text = clip(plainText(m.content), SUMMARY_MAX_CHARS);
      if (text) current.summary = text;
    }
  });
  if (current) out.push(current);
  // 只有「最后一轮 + 会话仍在跑」才算进行中：历史轮没总结（如报错中断）不该显示"进行中"
  return out.map((turn, i) => ({
    ...turn,
    pending: i === out.length - 1 && (running || !turn.summary),
  }));
}

export interface TurnRailProps {
  /** 消息滚动容器（`.codex-chat`）的 ref —— 本组件自行读取其滚动位置 */
  scrollRef: React.RefObject<HTMLDivElement | null>;
  turns: TurnRailItem[];
}

/** 悬停时邻近横杠的宽度衰减：距离越远越接近基准宽度，形成"凸起"。 */
function falloff(distance: number): number {
  return 1 / (1 + 0.55 * distance * distance);
}

const IDLE_WIDTH = 9;
const ACTIVE_WIDTH = 18;
const PEAK_WIDTH = 27;
/** 预览卡延迟出现，避免扫过一列横杠时闪成一串卡片 */
const POP_DELAY_MS = 90;

const TurnRail: React.FC<TurnRailProps> = ({ scrollRef, turns }) => {
  const { t } = useTranslation();
  const [active, setActive] = useState(0);
  const [hovered, setHovered] = useState<number | null>(null);
  const [popOpen, setPopOpen] = useState(false);
  /** 预览卡的纵向位置 = 被悬停横杠在轨道内的 offsetTop */
  const [popTop, setPopTop] = useState(0);
  const popTimer = useRef<number | null>(null);
  const frame = useRef<number | null>(null);
  /** 与 popOpen 同步的 ref：进入相邻横杠时要判断"是否已经开着"，读 state 会拿到旧值 */
  const popOpenRef = useRef(false);
  popOpenRef.current = popOpen;

  const clearPopTimer = useCallback(() => {
    if (popTimer.current !== null) {
      window.clearTimeout(popTimer.current);
      popTimer.current = null;
    }
  }, []);

  /**
   * 重新判定「当前停留的轮」。
   *
   * 判据是滚动位置而非 IntersectionObserver：小横杠需要的是"我在这轮里"这种
   * 单调递进的位置感，用锚点顶边与视口上方探针比较最直接，也不会因消息高度
   * 剧烈变化（工具组展开、图片加载）而反复进出触发。
   */
  const measure = useCallback(() => {
    const container = scrollRef.current;
    if (!container) return;
    const anchors = container.querySelectorAll<HTMLElement>('[data-turn-anchor]');
    if (anchors.length === 0) return;
    const containerTop = container.getBoundingClientRect().top;
    // 探针放在视口上方偏下一点：滚动到某轮开头时它已算作"当前轮"
    const probe = containerTop + 28;
    let next = 0;
    for (let i = 0; i < anchors.length && i < turns.length; i++) {
      if (anchors[i].getBoundingClientRect().top <= probe) next = i;
    }
    setActive((prev) => (prev === next ? prev : next));
  }, [scrollRef, turns.length]);

  /** 滚动只挂一个 rAF 节流：测量本身不写 DOM，一帧一次足够且不会抖动 */
  useEffect(() => {
    const container = scrollRef.current;
    if (!container) return;
    const onScroll = () => {
      if (frame.current !== null) return;
      frame.current = window.requestAnimationFrame(() => {
        frame.current = null;
        measure();
      });
    };
    container.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onScroll);
    onScroll();
    return () => {
      container.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', onScroll);
      if (frame.current !== null) window.cancelAnimationFrame(frame.current);
      frame.current = null;
    };
  }, [scrollRef, measure]);

  // 轮次变化（新消息 / 切会话 / 工具组展开导致高度变化）后重新定位
  useEffect(() => {
    measure();
  }, [measure, turns]);

  useEffect(() => clearPopTimer, [clearPopTimer]);

  const handleEnter = useCallback(
    (i: number, el: HTMLElement) => {
      setHovered(i);
      setPopTop(el.offsetTop + el.offsetHeight / 2);
      // 已经开着就别重启延迟：在相邻横杠之间移动时，重启会让预览卡闪一下
      if (popOpenRef.current) return;
      clearPopTimer();
      popTimer.current = window.setTimeout(() => setPopOpen(true), POP_DELAY_MS);
    },
    [clearPopTimer],
  );

  const handleLeave = useCallback(() => {
    clearPopTimer();
    setHovered(null);
    setPopOpen(false);
  }, [clearPopTimer]);

  /** 平滑跳转到该轮开头（-12px 让标题不贴顶）。 */
  const jumpTo = useCallback(
    (i: number) => {
      const container = scrollRef.current;
      const turn = turns[i];
      if (!container || !turn) return;
      const anchor = container.querySelector<HTMLElement>(
        `[data-turn-anchor="${turn.index}"]`,
      );
      if (!anchor) return;
      const top =
        anchor.getBoundingClientRect().top -
        container.getBoundingClientRect().top +
        container.scrollTop;
      container.scrollTo({ top: Math.max(0, top - 12), behavior: 'smooth' });
    },
    [scrollRef, turns],
  );

  // 单轮时一根孤零零的横杠不表达"位置"，也不好看 —— 两轮起才出现
  if (turns.length < 2) return null;

  const hovering = hovered !== null;
  const current = turns[hovered ?? active];

  return (
    <div
      className={`codex-rail${
        turns.length >= 90 ? ' dense2' : turns.length >= 40 ? ' dense' : ''
      }`}
      role="navigation"
      aria-label={t('mind_inspector.code_rail_label')}
    >
      <div className="codex-rail-col" onMouseLeave={handleLeave}>
        {turns.map((turn, i) => {
          const width = hovering
            ? Math.max(
                IDLE_WIDTH + (PEAK_WIDTH - IDLE_WIDTH) * falloff(Math.abs(i - hovered)),
                i === active ? ACTIVE_WIDTH : 0,
              )
            : i === active
              ? ACTIVE_WIDTH
              : IDLE_WIDTH;
          const emphasized = i === hovered || (hovered === null && i === active);
          return (
            <button
              key={turn.index}
              type="button"
              className={`codex-rail-tick${emphasized ? ' on' : ''}`}
              style={{ width }}
              /* 不进 Tab 顺序：一轮一个按钮，几十个 tab 停靠会把用户卡在输入框之前。
                 内容本身可用普通滚动到达，这里只作为鼠标用的快捷导航。 */
              tabIndex={-1}
              onMouseEnter={(e) => handleEnter(i, e.currentTarget)}
              onFocus={(e) => handleEnter(i, e.currentTarget)}
              onBlur={handleLeave}
              onClick={() => jumpTo(i)}
              aria-label={
                turn.user
                  ? t('mind_inspector.code_rail_jump', { text: turn.user })
                  : t('mind_inspector.code_rail_jump_untitled')
              }
            />
          );
        })}
      </div>

      {popOpen && current && (
        <div className="codex-rail-pop" style={{ top: popTop }} aria-hidden>
          <div className="codex-rail-pop-user">
            {current.user || t('mind_inspector.code_rail_jump_untitled')}
          </div>
          <div className="codex-rail-pop-summary">
            {current.summary
              || (current.pending
                ? t('mind_inspector.code_rail_pending')
                : t('mind_inspector.code_rail_no_summary'))}
          </div>
        </div>
      )}
    </div>
  );
};

export default TurnRail;
