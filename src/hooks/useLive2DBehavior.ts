/**
 * Live2D 微存在感 + 自主行为
 *
 * - `setLive2DParam`：通用参数设置工具（默认 'manual' 层）
 * - `useMicroPresence`：综合自主行为驱动
 *
 * 通用参数（两模型都有）: ParamAngleX/Y/Z, ParamBodyAngleX/Y/Z,
 *   ParamEyeLOpen/ROpen, ParamEyeBallX/Y, ParamMouthOpenY, ParamMouthForm,
 *   ParamEyeLSmile/ParamEyeRSmile, ParamBrowLY, ParamBreath
 * 专属参数在其他模型上 setParameterValueById 静默无效，不会报错。
 *
 * 自主行为分层（避免与 EmotionFacs 冲突）：
 * - 'idle' 层（优先级 0）：ParamAngleX（头部左右）、ParamEyeBallX/Y（视线）、
 *   ParamBodyAngleX/Z（身体倾）—— EmotionFacs 不写入这些参数
 * - 'blink' 层（优先级 1.2）：ParamEyeLOpen/ROpen 程序化眨眼，覆盖 emotion 层
 * - 'instant' 层（优先级 1.5）：偶尔伸展动作（ParamAngleY + ParamMouthOpenY + ParamBodyAngleY）
 *
 * EmotionFacs 持续写入 'emotion' 层的 ParamAngleY/Z 和 ParamEyeLOpen/ROpen，
 * 因此自主行为不直接控制这些参数（避免被覆盖），仅在伸展时通过 'instant' 层短暂介入。
 */

import { useEffect, useRef } from 'react';
import type { MutableRefObject, RefObject } from 'react';
import type { Live2DModel } from 'pixi-live2d-display/cubism4';
import type { Live2DLipsync } from '../utils/Live2DLipsync';
import { useAppStore } from '../stores/useAppStore';
import {
  directSetParam,
  getMixer,
  type ParameterLayer,
} from '../utils/LayeredParameterMixer';

/* ==================== 参数设置工具 ==================== */

export function setLive2DParam(
  model: Live2DModel | null,
  id: string,
  value: number,
  weight = 1.0,
  layer: ParameterLayer = 'manual',
): void {
  if (!model) return;
  const mixer = getMixer(model);
  if (mixer) {
    mixer.setParameter(layer, id, value);
  } else {
    directSetParam(model, id, value, weight);
  }
}

/* ==================== 兴趣点定义 ==================== */

interface InterestPoint {
  x: number;
  y: number;
  weight: number;
}

/**
 * 视线兴趣点集合（虚拟注视目标）。
 * 模拟桌宠"看"不同方向：看手机（下方）、看任务栏、看窗外、看天空等。
 * weight 决定被选中的概率，下方兴趣点权重更高（符合真人习惯）。
 */
const GAZE_INTEREST_POINTS: InterestPoint[] = [
  { x: 0.0, y: -0.7, weight: 1.4 },  // 正下方（看手机/桌面，最频繁）
  { x: -0.55, y: -0.25, weight: 1.0 },  // 左下方
  { x: 0.55, y: -0.25, weight: 1.0 },  // 右下方
  { x: -0.35, y: 0.35, weight: 0.7 },  // 左上方
  { x: 0.35, y: 0.35, weight: 0.7 },  // 右上方
  { x: -0.85, y: 0.05, weight: 0.6 },  // 极左（看窗外）
  { x: 0.85, y: 0.05, weight: 0.6 },  // 极右
  { x: 0.0, y: 0.0, weight: 0.5 },  // 回中
];

/**
 * 头部转动兴趣点（粗粒度，4 个大方向 + 回中）。
 * 头部转动幅度小于视线（人体工程学：眼动多于头动）。
 */
const HEAD_INTEREST_POINTS: InterestPoint[] = [
  { x: 0.0, y: 0.0, weight: 0.5 },    // 正前
  { x: -0.5, y: 0.0, weight: 1.0 },   // 左转
  { x: 0.5, y: 0.0, weight: 1.0 },    // 右转
  { x: 0.0, y: -0.3, weight: 0.8 },   // 低头
  { x: 0.0, y: 0.2, weight: 0.5 },    // 抬头
];

/** 加权随机选取兴趣点 */
function pickInterestPoint(points: InterestPoint[]): InterestPoint {
  const totalWeight = points.reduce((sum, p) => sum + p.weight, 0);
  let r = Math.random() * totalWeight;
  for (const p of points) {
    r -= p.weight;
    if (r <= 0) return p;
  }
  return points[points.length - 1];
}

/* ==================== 眨眼状态机 ==================== */

type BlinkState = 'idle' | 'closing' | 'closed' | 'opening' | 'double_blink_gap';

interface BlinkController {
  state: BlinkState;
  stateStart: number;  // 秒
  nextBlinkAt: number;  // 秒，下次开始眨眼的时刻
  isDoubleBlink: boolean;
}

function createBlinkController(now: number): BlinkController {
  return {
    state: 'idle',
    stateStart: now,
    nextBlinkAt: now + 4 + Math.random() * 4,  // 4-8s 间隔
    isDoubleBlink: false,
  };
}

/** 眨眼各阶段时长（秒） */
const BLINK_CLOSING_DURATION = 0.08;
const BLINK_CLOSED_DURATION = 0.05;
const BLINK_OPENING_DURATION = 0.12;
const BLINK_DOUBLE_GAP = 0.1;  // 双眨间隔
const BLINK_DOUBLE_PROBABILITY = 0.15;

/* ==================== 伸展动作 ==================== */

interface StretchState {
  active: boolean;
  start: number;  // 秒
  duration: number;  // 秒
}

const STRETCH_DURATION = 2.5;
const STRETCH_PROBABILITY = 0.06;  // 每次头部转动切换时 6% 概率触发伸展

/* ==================== useMicroPresence ==================== */

interface MicroPresenceOptions {
  /** Live2DLipsync 实例引用（用于设置呼吸偏移） */
  lipsyncRef: RefObject<Live2DLipsync | null>;
}

/**
 * 微存在感 hook —— 呼吸 + 身体微晃 + 视线游移 + 头部自主转动 +
 * 程序化眨眼 + 身体姿势变换 + 偶尔伸展。
 *
 * 参考 AG99live 的设计：多参数不同周期制造非周期感；
 * 视线/头部转动由"虚拟兴趣点"驱动而非纯随机，更具意图感。
 *
 * 休息/离线状态的闭眼 + 头部下垂由 App.tsx 的 presenceState useEffect 统一守护，
 * 此 hook 不再负责睡眠参数写入。
 *
 * 情绪数据从 `useAppStore` 的 `currentMood` 读取（valence/arousal/energy），
 * 无情绪数据时退化为基础微晃。
 */
export function useMicroPresence(
  modelRef: RefObject<Live2DModel | null>,
  options: MicroPresenceOptions,
): MutableRefObject<() => void> {
  const mood = useAppStore((s) => s.currentMood);
  const moodRef = useRef(mood);
  moodRef.current = mood;
  const presenceState = useAppStore((s) => s.presenceState);
  const presenceRef = useRef(presenceState);
  presenceRef.current = presenceState;

  const tickRef = useRef<() => void>(() => {});

  useEffect(() => {
    const startTime = performance.now();

    // 视线游移状态
    let gazeX = 0;
    let gazeY = 0;
    let gazeFromX = 0;
    let gazeFromY = 0;
    let gazeToX = 0;
    let gazeToY = 0;
    let gazeMoveStart = 0;
    let gazeMoveDuration = 0;
    let gazeHoldUntil = 0;

    // 头部转动状态（ParamAngleX，左右）
    let headX = 0;
    let headFromX = 0;
    let headToX = 0;
    let headMoveStart = 0;
    let headMoveDuration = 0;
    let headHoldUntil = 0;

    // 身体姿势状态（ParamBodyAngleX，左右倾，慢速变换）
    let bodyPoseX = 0;
    let bodyPoseFromX = 0;
    let bodyPoseToX = 0;
    let bodyPoseMoveStart = 0;
    let bodyPoseMoveDuration = 0;
    let bodyPoseHoldUntil = 0;

    // 伸展动作状态
    const stretch: StretchState = { active: false, start: 0, duration: STRETCH_DURATION };

    // 眨眼控制器
    let blink = createBlinkController(0);

    const easeInOut = (x: number) => (x < 0.5 ? 2 * x * x : 1 - (-2 * x + 2) * (-2 * x + 2) / 2);

    const scheduleGaze = (t: number, stability: number) => {
      const p = pickInterestPoint(GAZE_INTEREST_POINTS);
      gazeFromX = gazeX;
      gazeFromY = gazeY;
      gazeToX = p.x;
      gazeToY = p.y;
      gazeMoveStart = t;
      gazeMoveDuration = 0.55 + stability * 0.5 + Math.random() * (0.9 + stability * 0.62);
      gazeHoldUntil = t + gazeMoveDuration + 0.8 + stability * 1.25 + Math.random() * (1.7 + stability * 1.8);
    };

    const scheduleHead = (t: number) => {
      const p = pickInterestPoint(HEAD_INTEREST_POINTS);
      headFromX = headX;
      headToX = p.x * 18;  // 头部转动幅度 ±18（ParamAngleX 范围约 ±30）
      headMoveStart = t;
      headMoveDuration = 1.5 + Math.random() * 1.0;  // 1.5-2.5s 平滑过渡
      headHoldUntil = t + headMoveDuration + 6 + Math.random() * 12;  // 保持 6-18s

      // 头部转动切换时，小概率触发伸展动作
      if (!stretch.active && Math.random() < STRETCH_PROBABILITY) {
        stretch.active = true;
        stretch.start = t;
        stretch.duration = STRETCH_DURATION;
      }
    };

    const scheduleBodyPose = (t: number) => {
      bodyPoseFromX = bodyPoseX;
      bodyPoseToX = (Math.random() * 2 - 1) * 4;  // ±4 小幅身体左右倾
      bodyPoseMoveStart = t;
      bodyPoseMoveDuration = 2.0 + Math.random() * 1.0;  // 2-3s 平滑过渡
      bodyPoseHoldUntil = t + bodyPoseMoveDuration + 25 + Math.random() * 35;  // 保持 25-60s
    };

    /** 应用伸展动作到 'instant' 层，返回是否正在伸展 */
    const applyStretch = (t: number, mixer: ReturnType<typeof getMixer>): boolean => {
      if (!stretch.active) {
        // 伸展结束后清除 instant 层相关参数
        return false;
      }
      const elapsed = t - stretch.start;
      if (elapsed >= stretch.duration) {
        stretch.active = false;
        // 清除伸展参数
        if (mixer) {
          mixer.clearLayerParam('instant', 'ParamAngleY');
          mixer.clearLayerParam('instant', 'ParamMouthOpenY');
          mixer.clearLayerParam('instant', 'JawOpen');
          mixer.clearLayerParam('instant', 'Jawopen');
          mixer.clearLayerParam('instant', 'ParamBodyAngleY');
        }
        return false;
      }
      // 伸展曲线：0-0.25 上升，0.25-0.75 保持，0.75-1.0 回落
      const progress = elapsed / stretch.duration;
      let envelope: number;
      if (progress < 0.25) {
        envelope = easeInOut(progress / 0.25);
      } else if (progress < 0.75) {
        envelope = 1.0;
      } else {
        envelope = easeInOut(1 - (progress - 0.75) / 0.25);
      }
      if (mixer) {
        mixer.setParameter('instant', 'ParamAngleY', 8 * envelope);        // 抬头
        mixer.setParameter('instant', 'ParamMouthOpenY', 0.35 * envelope);  // 张嘴（打哈欠感）
        mixer.setParameter('instant', 'JawOpen', 0.35 * envelope);
        mixer.setParameter('instant', 'Jawopen', 0.35 * envelope);
        mixer.setParameter('instant', 'ParamBodyAngleY', 4 * envelope);     // 身体后仰
      }
      return true;
    };

    /** 更新眨眼状态机，写入 'blink' 层 */
    const updateBlink = (t: number, mixer: ReturnType<typeof getMixer>, isSpeaking: boolean) => {
      // 说话时降低眨眼频率（注意力集中在对话）
      const intervalBase = isSpeaking ? 6 : 4;
      const intervalRange = isSpeaking ? 6 : 4;

      switch (blink.state) {
        case 'idle': {
          if (t >= blink.nextBlinkAt) {
            blink.state = 'closing';
            blink.stateStart = t;
            blink.isDoubleBlink = Math.random() < BLINK_DOUBLE_PROBABILITY;
          }
          break;
        }
        case 'closing': {
          // 闭眼阶段：ParamEyeLOpen/ROpen 从 1 → 0
          const progress = (t - blink.stateStart) / BLINK_CLOSING_DURATION;
          if (progress >= 1) {
            blink.state = 'closed';
            blink.stateStart = t;
          } else {
            const v = 1 - easeInOut(progress);
            if (mixer) {
              mixer.setParameter('blink', 'ParamEyeLOpen', v);
              mixer.setParameter('blink', 'ParamEyeROpen', v);
            }
          }
          break;
        }
        case 'closed': {
          // 完全闭眼保持
          if (mixer) {
            mixer.setParameter('blink', 'ParamEyeLOpen', 0);
            mixer.setParameter('blink', 'ParamEyeROpen', 0);
          }
          if (t - blink.stateStart >= BLINK_CLOSED_DURATION) {
            blink.state = 'opening';
            blink.stateStart = t;
          }
          break;
        }
        case 'opening': {
          // 睁眼阶段：ParamEyeLOpen/ROpen 从 0 → 1
          const progress = (t - blink.stateStart) / BLINK_OPENING_DURATION;
          if (progress >= 1) {
            // 睁眼完成
            if (mixer) {
              mixer.clearLayerParam('blink', 'ParamEyeLOpen');
              mixer.clearLayerParam('blink', 'ParamEyeROpen');
            }
            if (blink.isDoubleBlink) {
              blink.state = 'double_blink_gap';
              blink.stateStart = t;
            } else {
              blink.state = 'idle';
              blink.nextBlinkAt = t + intervalBase + Math.random() * intervalRange;
            }
          } else {
            const v = easeInOut(progress);
            if (mixer) {
              mixer.setParameter('blink', 'ParamEyeLOpen', v);
              mixer.setParameter('blink', 'ParamEyeROpen', v);
            }
          }
          break;
        }
        case 'double_blink_gap': {
          // 双眨间隔（短暂睁眼后再次闭眼）
          if (mixer) {
            mixer.setParameter('blink', 'ParamEyeLOpen', 1);
            mixer.setParameter('blink', 'ParamEyeROpen', 1);
          }
          if (t - blink.stateStart >= BLINK_DOUBLE_GAP) {
            blink.state = 'closing';
            blink.stateStart = t;
            blink.isDoubleBlink = false;  // 第二次不再双眨
          }
          break;
        }
      }
    };

    const microTick = () => {
      if (document.hidden) return;
      const ps = presenceRef.current;
      if (ps === 'rest' || ps === 'offline') return;
      const model = modelRef.current;
      if (!model) return;

      const t = (performance.now() - startTime) / 1000;
      const currentMood = moodRef.current;
      const lipsync = options.lipsyncRef.current;
      const isSpeaking = lipsync?.getState() === 'speaking';
      const focusLevel = isSpeaking ? 1 : 0;
      const mixer = getMixer(model);

      // === 呼吸（正弦波驱动 ParamMouthOpenY 偏移） ===
      let breathFreq = 0.8;
      let breathAmp = 0.015;
      if (currentMood) {
        const energy = currentMood.energy / 100.0;
        const arousal = currentMood.arousal;
        breathFreq = 0.8 + energy * 0.6 + arousal * 0.4;
        breathAmp = 0.015 + energy * 0.02;
      }
      const breath = Math.sin(t * breathFreq) * breathAmp;
      if (lipsync) {
        lipsync.setBreathOffset(breath);
      }

      // === 身体微晃（ParamBodyAngleZ，正弦波侧倾） ===
      let swayFreq = 0.4;
      let swayAmp = 0.8;
      if (currentMood) {
        const arousal = currentMood.arousal;
        swayFreq = 0.3 + arousal * 0.2;
        swayAmp = 0.5 + arousal * 2.0;
      }
      const bodyZ = Math.sin(t * swayFreq) * swayAmp;
      if (mixer) {
        mixer.setParameter('idle', 'ParamBodyAngleZ', bodyZ);
      } else {
        directSetParam(model, 'ParamBodyAngleZ', bodyZ);
      }

      // === 身体姿势变换（ParamBodyAngleX，慢速左右倾，30-60s 变换） ===
      if (!isSpeaking && t >= bodyPoseHoldUntil) {
        scheduleBodyPose(t);
      }
      if (bodyPoseMoveDuration > 0 && t < bodyPoseMoveStart + bodyPoseMoveDuration) {
        const local = (t - bodyPoseMoveStart) / bodyPoseMoveDuration;
        const eased = easeInOut(Math.max(0, Math.min(1, local)));
        bodyPoseX = bodyPoseFromX + (bodyPoseToX - bodyPoseFromX) * eased;
      }
      if (mixer) {
        mixer.setParameter('idle', 'ParamBodyAngleX', bodyPoseX);
      } else {
        directSetParam(model, 'ParamBodyAngleX', bodyPoseX);
      }

      // === 伸展动作（'instant' 层，覆盖 emotion） ===
      const stretching = applyStretch(t, mixer);

      // === 头部自主转动（ParamAngleX，左右） ===
      // 说话或伸展时不做头部自主转动（让位给情绪/伸展表达）
      if (!isSpeaking && !stretching && t >= headHoldUntil) {
        scheduleHead(t);
      }
      if (headMoveDuration > 0 && t < headMoveStart + headMoveDuration && !isSpeaking) {
        const local = (t - headMoveStart) / headMoveDuration;
        const eased = easeInOut(Math.max(0, Math.min(1, local)));
        headX = headFromX + (headToX - headFromX) * eased;
      } else if (isSpeaking) {
        // 说话时缓慢回中
        headX += (0 - headX) * 0.08;
      }
      if (mixer) {
        mixer.setParameter('idle', 'ParamAngleX', headX);
      } else {
        directSetParam(model, 'ParamAngleX', headX);
      }

      // === 视线游移（ParamEyeBallX/Y，兴趣点驱动） ===
      const stability = currentMood ? Math.max(0.2, 1 - currentMood.arousal * 0.7) : 0.72;
      if (focusLevel > 0.5) {
        // 说话时视线缓慢回中
        const recenterFactor = 0.06 + t * 0.0008;
        gazeX += (0 - gazeX) * Math.min(1, recenterFactor);
        gazeY += (0 - gazeY) * Math.min(1, recenterFactor);
      } else {
        if (t >= gazeHoldUntil) {
          scheduleGaze(t, stability);
        }
        if (gazeMoveDuration > 0 && t < gazeMoveStart + gazeMoveDuration) {
          const local = (t - gazeMoveStart) / gazeMoveDuration;
          const eased = easeInOut(Math.max(0, Math.min(1, local)));
          gazeX = gazeFromX + (gazeToX - gazeFromX) * eased;
          gazeY = gazeFromY + (gazeToY - gazeFromY) * eased;
        }
      }
      if (mixer) {
        mixer.setParameter('idle', 'ParamEyeBallX', gazeX);
        mixer.setParameter('idle', 'ParamEyeBallY', gazeY);
      } else {
        directSetParam(model, 'ParamEyeBallX', gazeX);
        directSetParam(model, 'ParamEyeBallY', gazeY);
      }

      // === 程序化眨眼（'blink' 层，覆盖 emotion 的 ParamEyeLOpen/ROpen） ===
      updateBlink(t, mixer, isSpeaking);
    };

    tickRef.current = microTick;

    return () => {
      const lipsync = options.lipsyncRef.current;
      if (lipsync) lipsync.setBreathOffset(0);
      // 清理 blink 层残留
      const model = modelRef.current;
      if (model) {
        const m = getMixer(model);
        if (m) {
          m.clearLayer('blink');
          m.clearLayerParam('instant', 'ParamAngleY');
          m.clearLayerParam('instant', 'ParamMouthOpenY');
          m.clearLayerParam('instant', 'JawOpen');
          m.clearLayerParam('instant', 'Jawopen');
          m.clearLayerParam('instant', 'ParamBodyAngleY');
        }
      }
      tickRef.current = () => {};
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return tickRef;
}
