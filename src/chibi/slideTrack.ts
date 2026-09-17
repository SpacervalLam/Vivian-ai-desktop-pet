/**
 * 窗口滑动的时间轴：把「起点 → 终点 + 总时长」拆成一串等时采样点。
 *
 * 采样密度只决定滑动看起来连不连续，不参与走动节奏的推导（那是 walkPlan 的事）：
 * 采样间隔固定、步数随时长增长，于是最短的挪动也至少有 MIN_POSITION_STEPS 次定位，
 * 长距离靠更多采样点覆盖，而不是把单步拉粗。
 *
 * 智能避让与自主漫步共用这一条时间轴。此前两处各写了一份采样循环（一份 32ms 采样、
 * 一份写死 48 步），步长与缓动曲线都不一致，同样的位移在两处会呈现不同的加速度。
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
  /** 下发一个采样点（含终点）。IPC 失败由实现方自行处理。 */
  apply: (x: number, y: number) => void;
  /** 每一帧下发前问一次；返回 true 表示当帧中止。省略则永不主动中止。 */
  shouldAbort?: () => boolean;
}

/**
 * 按时间轴滑动窗口。返回是否走完全程——被中止时窗口停在半途，
 * 由调用方决定怎么收尾（避让会回落到基调姿态，漫步交给接管者）。
 */
export async function runSlide(options: SlideTrackOptions): Promise<boolean> {
  const { fromX, fromY, toX, toY, durationMs, apply, shouldAbort } = options;
  const steps = Math.max(MIN_POSITION_STEPS, Math.round(durationMs / MOVE_STEP_MS));
  const stepMs = durationMs / steps;
  for (let i = 1; i <= steps; i += 1) {
    if (shouldAbort?.()) return false;
    const eased = easeInOutCubic(i / steps);
    apply(
      Math.round(fromX + (toX - fromX) * eased),
      Math.round(fromY + (toY - fromY) * eased),
    );
    if (i < steps) await wait(stepMs);
  }
  return true;
}
