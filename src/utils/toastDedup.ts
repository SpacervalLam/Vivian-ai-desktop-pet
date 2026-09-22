/**
 * 跨窗口 toast 内容去重。
 *
 * toast 窗口按角色私有（label = `${charId}_toast`），而 `app.emit` 是全局广播，
 * 不带角色归属的事件会让每只桌宠的 toast 窗口各渲染一条同样的文案。
 *
 * 归属路由（payload 带 character_id + 接收侧按归属过滤）能消灭已知来源，但依赖
 * 「每个发送侧都记得标注归属」；本模块提供与来源无关的兜底，让「同一内容在屏幕上
 * 只存在一条」成为硬约束。
 *
 * 判定规则（两个窗口都按同一规则计算，故无需协商即可收敛）：
 * 1. 本窗口在时间窗内已呈现过同一指纹 → 放弃。
 * 2. 另一窗口已宣告同一指纹且其优先级**更高** → 让位（撤下自己那条）。
 *
 * 优先级用 `winsOver` 定义，必须是全序——否则两窗口会互相认为对方更低而各留一条。
 */

/**
 * toast 窗口的垂直堆叠次序（数组越靠前越贴近屏幕底部）。
 *
 * 每个角色、以及启动阶段，都跑在各自独立的透明窗口里，窗口之间互不可见，
 * 却又都锚定右下角、各自从底部堆叠——不做显式错开就会完全重叠。
 * 顺序写死即可保证稳定：startup 永远排最上，避免启动进度盖住角色 toast。
 */
export const STACK_ORDER: readonly string[] = ['vivian', 'nana', 'startup'];

/** 未列入 STACK_ORDER 的角色排在已知角色之后、startup 之前 */
export function stackRank(id: string): number {
  if (id === 'startup') return 1000;
  const i = STACK_ORDER.indexOf(id);
  return i >= 0 ? i : 500;
}

/**
 * 呈现优先级：rank 小者胜（越贴近屏幕底部的角色越先得），rank 相同则按 char_id
 * 字典序。返回值 true 表示 `a` 优先级**高于** `b`。
 */
export function winsOver(a: string, b: string): boolean {
  const ra = stackRank(a);
  const rb = stackRank(b);
  return ra !== rb ? ra < rb : a < b;
}

/**
 * 内容指纹：类型 + 正文。
 *
 * 只有正文完全相同的两条才可能被判为重复——「已添加待办：A」与「已添加待办：B」
 * 指纹不同，互不影响。类型参与指纹，是因为同一句话以 warning / error 出现
 * 属于两次不同语义的提示。
 */
export function toastFingerprint(message: string, type: string): string {
  return `${type}\u0000${message}`;
}

/** 跨窗口内容去重的时间窗（毫秒）：窗口期内同一内容只呈现一次 */
export const DEDUP_WINDOW_MS = 2000;

/**
 * 「同一个 key 还会再来」的判定窗口（毫秒）。
 *
 * 比内容去重窗宽得多：原地刷新的条目（进度条）可能连着刷新几十秒，中间任何一次刷新
 * 若被误判成「一次性提示」，就会被内容去重拦住、进度冻住。
 */
export const KEY_REPEAT_WINDOW_MS = 60_000;

/**
 * 判定「这次到达是原地刷新，还是一次性提示」——只看**同一个 key 是否重复出现**。
 *
 * 为什么不能用 key 的类型或有无来判：payload 里的 `key` 本意是「这条有身份，后续会用
 * 同一个值再来更新它」，但这个语义只有在**第二次见到同一个值**时才成立。而现实里一次性
 * 提示也普遍自带 `key: Date.now()`（用一次就丢，没人会拿它回来更新），于是「有 key 就豁免
 * 去重」等于把几乎所有一次性提示都排除在跨窗口去重之外——同一条文案就会在每个角色的
 * toast 窗口各弹一条。判据必须落在可观察的行为上（key 重复出现），而不是 key 的形式。
 *
 * 只登记**真正落屏**的 key：被去重拦下、根本没显示的那条不该让后续同名到达获得豁免。
 */
export class ToastKeyTracker {
  private seen = new Map<string | number, number>();

  /** 清理超窗记录，避免长期运行下无限增长 */
  prune(now: number): void {
    for (const [key, at] of this.seen) {
      if (now - at >= KEY_REPEAT_WINDOW_MS) this.seen.delete(key);
    }
  }

  /** 只读判定：这个 key 之前是否已经落过屏（true = 本次到达是原地刷新） */
  isRepeat(key: string | number | undefined | null, now: number): boolean {
    if (key == null) return false;
    this.prune(now);
    return this.seen.has(key);
  }

  /** 登记某个 key 已落屏 */
  note(key: string | number | undefined | null, now: number): void {
    if (key == null) return;
    this.seen.set(key, now);
  }

  /** 供测试断言内部状态 */
  debugState(): { tracked: number } {
    return { tracked: this.seen.size };
  }
}

/** 其他窗口宣告的认领记录 */
interface PeerClaim {
  charId: string;
  at: number;
}

/**
 * 单个 toast 窗口的去重闸门。
 *
 * 每个窗口各持一个实例，只保存与去重有关的两个表；组件侧不需要自己维护 Map，
 * 也便于在 Node 里直接对判定逻辑做行为验证（见 scripts/toast-dedup.test.ts）。
 */
export class ToastDedupGate {
  private myCharId: string;
  /** 本窗口已呈现过的指纹 → 时间戳 */
  private recent = new Map<string, number>();
  /** 其他窗口已宣告呈现的指纹 → 认领记录 */
  private peers = new Map<string, PeerClaim>();

  constructor(myCharId: string) {
    this.myCharId = myCharId;
  }

  /** 清理超出时间窗的记录，避免长期运行下无限增长 */
  prune(now: number): void {
    for (const [fp, at] of this.recent) {
      if (now - at >= DEDUP_WINDOW_MS) this.recent.delete(fp);
    }
    for (const [fp, claim] of this.peers) {
      if (now - claim.at >= DEDUP_WINDOW_MS) this.peers.delete(fp);
    }
  }

  /**
   * 本窗口是否应当放弃呈现这条内容。
   *
   * 返回 true 时调用方**不要**渲染，也不要广播认领——否则会把本该由别人呈现的
   * 内容标记成自己的，导致两个窗口都以为自己该让位、最终谁都不弹。
   */
  shouldSkip(fp: string, now: number): boolean {
    const mine = this.recent.get(fp);
    if (mine != null && now - mine < DEDUP_WINDOW_MS) return true;
    const peer = this.peers.get(fp);
    if (peer && now - peer.at < DEDUP_WINDOW_MS && winsOver(peer.charId, this.myCharId)) {
      return true;
    }
    return false;
  }

  /** 登记本窗口已呈现该内容（同步写入：同一 tick 内到达的第二条也能被拦住） */
  claim(fp: string, now: number): void {
    this.recent.set(fp, now);
  }

  /**
   * 观察其他窗口的认领。返回 true 表示本窗口应让位——调用方需撤下自己那条
   * 同样内容的提示，保证屏幕上只剩优先级更高的那个窗口的那条。
   */
  observe(peerCharId: string, fp: string, now: number): boolean {
    this.peers.set(fp, { charId: peerCharId, at: now });
    return winsOver(peerCharId, this.myCharId);
  }

  /** 供测试断言内部状态 */
  debugState(): { recent: number; peers: number } {
    return { recent: this.recent.size, peers: this.peers.size };
  }
}
