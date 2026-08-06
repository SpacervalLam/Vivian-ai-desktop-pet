/**
 * Live2D 嘴形联动工具    
 *
 * 由于 Tauri 版本的 Live2D 渲染在前端（PixiJS + pixi-live2d-display），
 * 后端 `commands/live2d_lipsync.rs` 只负责维护状态并通过事件通知前端，
 * 本工具负责监听 `lipsync:start` / `lipsync:update` / `lipsync:stop` 事件，
 * 实时驱动 Live2D 模型的 `ParamMouthOpenY` 参数。
 *
 *   - Speaking 时 target_open = 0.25
 *   - Idle 时 target_open = 0.0
 *   - 平滑插值：current += (target - current) * smoothSpeed
 *
 * 微存在感呼吸叠加：`setBreathOffset()` 由 `useMicroPresence` 每帧调用，
 * 将正弦波呼吸偏移叠加到目标开合度上（target_open += sin(t * breath_freq) * breath_amp）。
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Live2DModel } from 'pixi-live2d-display/cubism4';
import { getMixer, directSetParam } from './LayeredParameterMixer';

/** 嘴形联动状态（与后端 `LipsyncState` 对齐） */
export type LipsyncState = 'idle' | 'speaking' | 'manual';

/** 嘴形参数 ID（Cubism 4 标准参数 + 模型自定义 JawOpen 变体） */
const MOUTH_PARAM_ID = 'ParamMouthOpenY';
/** 所有控制嘴巴张开的参数 ID（不同模型用不同命名，全部设置以确保闭合） */
const MOUTH_OPEN_PARAM_IDS = ['ParamMouthOpenY', 'JawOpen', 'Jawopen'];

/** 状态常量 */
const TARGET_SPEAKING = 0.25;
const TARGET_IDLE = 0.0;
const MIN_OPEN = 0.0;
const MAX_OPEN = 1.0;

/** 平滑速度 */
const SMOOTH_SPEED = 0.2;

/** 动画帧间隔（毫秒） */
const ANIMATION_INTERVAL_MS = 16;
/** 空闲稳定后的降频间隔（毫秒） */
const IDLE_INTERVAL_MS = 100;

export interface LipsyncOptions {
  /** 平滑速度 [0.0, 1.0] */
  smoothSpeed?: number;
  /** 最小开合度 */
  minOpen?: number;
  /** 最大开合度 */
  maxOpen?: number;
}

interface LipsyncEventPayload {
  text?: string;
  target_open?: number;
  /** 后端 lipsync:update 事件推送的真实嘴形开合度（来自 WordBoundary 音素映射） */
  mouth_open?: number;
  /** 兼容字段：部分旧路径可能使用 open */
  open?: number;
  viseme?: string | null;
}

/**
 * Live2D 嘴形联动运行时。
 *
 * 使用方式：
 *   const lipsync = new Live2DLipsync(model);
 *   await lipsync.start();  // 开始监听后端事件
 *   ...
 *   lipsync.stop();         // 停止监听并释放资源
 */
export class Live2DLipsync {
  private model: Live2DModel | null = null;
  private unlisteners: UnlistenFn[] = [];
  private animationTimer: ReturnType<typeof setTimeout> | null = null;

  private state: LipsyncState = 'idle';
  private currentOpen = TARGET_IDLE;
  private targetOpen = TARGET_IDLE;

  /** 呼吸偏移量（由 useMicroPresence 每帧设置，叠加到 targetOpen 上） */
  private breathOffset = 0;

  private smoothSpeed: number;
  private minOpen: number;
  private maxOpen: number;

  constructor(model: Live2DModel | null, options: LipsyncOptions = {}) {
    this.model = model;
    this.smoothSpeed = options.smoothSpeed ?? SMOOTH_SPEED;
    this.minOpen = options.minOpen ?? MIN_OPEN;
    this.maxOpen = options.maxOpen ?? MAX_OPEN;
  }

  /** 绑定新的 Live2D 模型（模型重建时调用） */
  setModel(model: Live2DModel | null): void {
    this.model = model;
  }

  /** 当前状态 */
  getState(): LipsyncState {
    return this.state;
  }

  /** 当前开合度 [0.0, 1.0] */
  getCurrentOpen(): number {
    return this.currentOpen;
  }

  /** 开始监听后端 lipsync 事件并启动动画循环 */
  async start(): Promise<void> {
    this.stop();
    this.unlisteners.push(
      await listen<LipsyncEventPayload>('lipsync:start', (event) => {
        this.state = 'speaking';
        this.targetOpen = clamp(
          event.payload.target_open ?? TARGET_SPEAKING,
          this.minOpen,
          this.maxOpen,
        );
      }),
    );
    this.unlisteners.push(
      await listen<LipsyncEventPayload>('lipsync:update', (event) => {
        // 真音素级唇形同步：后端 WordBoundary 事件驱动真实 mouth_open 值
        // 优先使用 mouth_open（来自 EdgeTts WordBoundary 音素映射），
        // 兼容旧路径的 open 字段
        const payload = event.payload;
        const nextOpen = payload.mouth_open ?? payload.open ?? this.targetOpen;
        this.state = 'manual';
        this.targetOpen = clamp(nextOpen, this.minOpen, this.maxOpen);
      }),
    );
    this.unlisteners.push(
      await listen<LipsyncEventPayload>('lipsync:stop', (event) => {
        this.state = 'idle';
        this.targetOpen = clamp(
          event.payload.target_open ?? TARGET_IDLE,
          this.minOpen,
          this.maxOpen,
        );
      }),
    );
    this.startAnimationLoop();
  }

  /** 停止监听并释放资源 */
  stop(): void {
    for (const un of this.unlisteners) {
      try {
        un();
      } catch {
        /* ignore */
      }
    }
    this.unlisteners = [];
    if (this.animationTimer !== null) {
      clearTimeout(this.animationTimer);
      this.animationTimer = null;
    }
    this.state = 'idle';
    this.targetOpen = TARGET_IDLE;
  }

  /** 手动触发嘴形（不通过事件，供前端直接调用） */
  manualUpdate(openAmount: number): void {
    this.state = 'manual';
    this.targetOpen = clamp(openAmount, this.minOpen, this.maxOpen);
  }

  /** 重置为空闲状态 */
  resetToIdle(): void {
    this.state = 'idle';
    this.targetOpen = TARGET_IDLE;
  }

  /**
   * 设置呼吸偏移量（由 useMicroPresence 每帧调用）。
   * 偏移量会叠加到 targetOpen 上，再经平滑插值写入模型，
   */
  setBreathOffset(offset: number): void {
    this.breathOffset = offset;
  }

  private startAnimationLoop(): void {
    if (this.animationTimer !== null) return;
    this.scheduleTick();
  }

  private scheduleTick(): void {
    this.animationTimer = setTimeout(() => {
      this.tick();
      this.scheduleTick();
    }, this.getTickInterval());
  }

  /** 根据状态动态选择间隔：空闲且嘴形已稳定时降频，减少无效参数写入 */
  private getTickInterval(): number {
    if (this.state === 'idle' && Math.abs(this.currentOpen - this.targetOpen) < 0.01) {
      return IDLE_INTERVAL_MS;
    }
    return ANIMATION_INTERVAL_MS;
  }

  private tick(): void {
    if (!this.model) return;

    // 平滑插值：current += (effectiveTarget - current) * smoothSpeed
    // effectiveTarget = targetOpen + breathOffset
    const effectiveTarget = clamp(
      this.targetOpen + this.breathOffset,
      this.minOpen,
      this.maxOpen,
    );
    const delta = effectiveTarget - this.currentOpen;
    if (Math.abs(delta) < 0.001) {
      this.currentOpen = effectiveTarget;
    } else {
      this.currentOpen += delta * this.smoothSpeed;
    }
    this.currentOpen = clamp(this.currentOpen, this.minOpen, this.maxOpen);

    this.applyToModel();
  }

  private applyToModel(): void {
    const model = this.model;
    if (!model) return;
    const mixer = getMixer(model);
    if (mixer) {
      for (const id of MOUTH_OPEN_PARAM_IDS) {
        mixer.setParameter('speech', id, this.currentOpen);
      }
    } else {
      for (const id of MOUTH_OPEN_PARAM_IDS) {
        directSetParam(model, id, this.currentOpen);
      }
    }
  }
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

export default Live2DLipsync;
