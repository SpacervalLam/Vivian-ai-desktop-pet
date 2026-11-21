/**
 * 「戳烦了」的账本。
 *
 * 点击本身不携带力度，能测的只有次数和时间——所以「太频繁」和「太粗暴」是同一个账本上的
 * 两种计法：每戳一下记一笔，与上一戳贴得极近（猛戳）再记一笔。
 * 只统计最近 {@link TAP_ANNOY_WINDOW_MS} 内的账，于是慢慢戳永远攒不满，停手就自动清账。
 *
 * 攒够 {@link TAP_ANNOY_THRESHOLD} 就生气：清空账本、进入气头上，这期间怎么戳都是生气
 * （每戳一次把气头续期），停手满 {@link TAP_ANNOY_HOLD_MS} 才消气、从零重新攒。
 *
 * 账本把「还在气头上」和「这一下刚点着」分开报（{@link TapVerdict}）：前者只续期，
 * 后者才是该**当场**给出反应的那一下——调用方据此决定要不要重播表情、要不要逃跑。
 */

/** 记账窗口（ms）：只统计最近这段时间内的点击，于是慢慢戳永远攒不满，停手就自动清账。 */
export const TAP_ANNOY_WINDOW_MS = 5_000;
/** 两戳间隔短于此值即视为「猛戳」，额外记一笔。 */
export const TAP_ROUGH_INTERVAL_MS = 350;
/** 攒够这么多分就生气。 */
export const TAP_ANNOY_THRESHOLD = 7;
/** 气头上的持续时长（ms）：每戳一次续期，停手满这么久才消气。 */
export const TAP_ANNOY_HOLD_MS = 2_500;

export interface TapVerdict {
  /** 这一下算不算「戳烦了」：气头上算，把气头点着的那一下也算。 */
  annoyed: boolean;
  /** 这一下是否把气头点着了（本口气的第一次）。只有它为真时才该重播表情 / 逃跑。 */
  onset: boolean;
}

/**
 * 账本状态。
 *
 * 调用方持有一个实例（组件里放在 ref 上）；`note` 是唯一的写入口，于是「什么算戳烦了」
 * 这条判定只有一份实现，改规则不会漏掉某个调用点。
 */
export class TapAngerLedger {
  /** 最近这几次点击的时刻（只保留 {@link TAP_ANNOY_WINDOW_MS} 内的）。 */
  private taps: number[] = [];
  /** 气头上到什么时候；早于此值前的点击都算「还没消气」。 */
  private annoyedUntil = 0;

  /** 现在是否还在气头上。只读、不记账也不续期——供「按下时要不要抹掉生气脸」这类询问使用。 */
  isAnnoyed(now: number): boolean {
    return now < this.annoyedUntil;
  }

  /** 记一笔点击，回报这一下该怎么定性。 */
  note(now: number): TapVerdict {
    // 气头上：照旧算生气，并把气头往后续（只要还在戳就不消气）
    if (now < this.annoyedUntil) {
      this.annoyedUntil = now + TAP_ANNOY_HOLD_MS;
      return { annoyed: true, onset: false };
    }

    const recent = this.taps.filter((at) => now - at < TAP_ANNOY_WINDOW_MS);
    recent.push(now);
    // 基准分=窗口中戳了几下；猛戳额外记一笔——「又戳一次」和「连着猛戳」不是一回事
    let score = recent.length;
    for (let index = 1; index < recent.length; index += 1) {
      if (recent[index] - recent[index - 1] < TAP_ROUGH_INTERVAL_MS) score += 1;
    }
    if (score < TAP_ANNOY_THRESHOLD) {
      this.taps = recent;
      return { annoyed: false, onset: false };
    }
    // 攒够了：清账再生气，退出气头后从零重新攒，不会因为一笔旧账一直气下去
    this.taps = [];
    this.annoyedUntil = now + TAP_ANNOY_HOLD_MS;
    return { annoyed: true, onset: true };
  }
}
