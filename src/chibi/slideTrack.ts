/**
 * 窗口滑动的时间轴：把「起点 → 终点 + 总时长」拆成一串等时采样点。
 *
 * 采样密度只决定滑动看起来连不连续，不参与走动节奏的推导（那是 walkPlan 的事）：
 * 计划采样间隔由总时长和步数推导；忙时跳过错过的采样，不补发积压的位置请求。
 * 长距离靠更多采样点覆盖，而不是把单步拉粗。
 *
 * 智能避让与自主漫步共用这一条时间轴。
 */

/** 采样间隔（ms）：只决定窗口定位的密度。 */
const MOVE_STEP_MS = 32;
/** 采样点下限：再短的挪动也要有这么多帧定位，否则看上去是瞬移。 */
const MIN_POSITION_STEPS = 24;

const wait = (durationMs: number) => new Promise<void>((resolve) => {
  window.setTimeout(resolve, durationMs);
});

/** 两端慢、中间快：起步与收尾都不突兀，中段走满速度。 */
function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

export interface SlideTrackOptions {
  fromX: number;
  fromY: number;
  toX: number;
  toY: number;
  /** 权威时长（ms）：一般来自 walkPlan，走动与滑动据此收尾同时发生。 */
  durationMs: number;
  /** 下发一个采样点（含终点）；等待 IPC 完成再继续，失败交给调用方处理。 */
  apply: (x: number, y: number) => void | Promise<unknown>;
  /** 每一帧下发前问一次；返回 true 表示当帧中止。省略则永不主动中止。 */
  shouldAbort?: () => boolean;
}

/**
 * 按时间轴滑动窗口。返回是否走完全程——被中止时窗口停在半途，
 * 由调用方决定怎么收尾（避让会回落到 idle，漫步交给接管者）。
 */
export async function runSlide(options: SlideTrackOptions): Promise<boolean> {
  const { fromX, fromY, toX, toY, durationMs, apply, shouldAbort } = options;
  if (![fromX, fromY, toX, toY, durationMs].every(Number.isFinite) || durationMs < 0) {
    throw new RangeError('Slide coordinates and duration must be finite; duration must be nonnegative');
  }
  const steps = Math.max(MIN_POSITION_STEPS, Math.round(durationMs / MOVE_STEP_MS));
  const stepMs = durationMs / steps;
  const started = performance.now();
  while (true) {
    // Use elapsed time rather than accumulating timer delays. A busy WebView skips
    // missed samples instead of moving after its walking animation has ended.
    const elapsed = performance.now() - started;
    if (elapsed < durationMs) {
      const nextSampleAt = Math.min(durationMs, (Math.floor(elapsed / stepMs) + 1) * stepMs);
      await wait(Math.max(1, nextSampleAt - elapsed));
    }
    if (shouldAbort?.()) return false;
    const progress = durationMs === 0 ? 1 : Math.min(1, (performance.now() - started) / durationMs);
    const eased = easeInOutCubic(progress);
    await apply(
      Math.round(fromX + (toX - fromX) * eased),
      Math.round(fromY + (toY - fromY) * eased),
    );
    // Cancellation during the final IPC is still an interrupted movement.
    if (shouldAbort?.()) return false;
    if (progress >= 1) return true;
  }
}
