/**
 * 跨窗口 toast 内容去重的行为验证（无需启动应用、无需 WebView）。
 *
 * 为什么用「行为模型」而不是端到端：本机 CDP 无法驱动该应用的 WebView2，
 * 而这里要验证的是一条**判定规则**——两个窗口各自独立计算，最终必须收敛到
 * 「屏幕上只剩一条」。这类性质可以直接对规则建模来证伪。
 *
 * 模型忠实复刻 src/components/ToastWindow.tsx 的两个监听：
 *   - `toast:show`：归属判定 → 内容去重 → 同 key 原地更新 → 新建 → 广播认领
 *   - `toast:shown`：观察他人认领 → 优先级更低者撤下自己那条
 * 去重判定本身调用**真实实现**（src/utils/toastDedup.ts），不是副本。
 *
 * 运行：node --experimental-strip-types scripts/toast-dedup.test.ts
 */

import {
  DEDUP_WINDOW_MS,
  ToastDedupGate,
  ToastKeyTracker,
  toastFingerprint,
} from '../src/utils/toastDedup.ts';

/**
 * 生产环境的 payload 形状：一次性提示也自带一个「用一次就丢」的 key。
 * 曾经的判据（`typeof key === 'number'` 才豁免去重）会让**所有**这类提示绕过去重网，
 * 而这个文件此前的用例全都喂的是不带 key 的理想 payload，于是漏洞长期没被覆盖。
 */
const oneShotKey = () => Date.now() + Math.floor(Math.random() * 1000);

interface ToastShow {
  message: string;
  type?: string;
  key?: number | string;
  character_id?: string;
}

interface ToastShown {
  char_id: string;
  fp: string;
}

interface Item {
  key?: number | string;
  message: string;
  type: string;
  /** 已被同一个 key 刷新过 → 原地刷新条目，豁免内容去重与跨窗口让位 */
  inplace: boolean;
}

/** 一个 toast 窗口的行为模型 */
class Win {
  charId: string;
  isPrimary = false;
  gate: ToastDedupGate;
  keyTracker = new ToastKeyTracker();
  items: Item[] = [];
  /** 对照组开关：关掉去重网，用于证明「没有这道网时重复确实会发生」 */
  dedupDisabled = false;

  constructor(charId: string) {
    this.charId = charId;
    this.gate = new ToastDedupGate(charId);
  }

  /** 对应 ToastWindow 的 `toast:show` 监听 */
  onShow(p: ToastShow, now: number, bus: Bus): void {
    const owned = p.character_id ? p.character_id === this.charId : this.isPrimary;
    const toastType = p.type ?? 'info';
    // 真实规则：key **重复出现**才说明它是原地刷新条目，而不是"key 是数字就豁免去重"。
    // 一次性提示普遍自带 key: Date.now()，用后者当判据会把整张去重网关掉。
    const refresh = this.keyTracker.isRepeat(p.key, now);
    const dedupable = !refresh && !this.dedupDisabled;
    const fp = dedupable ? toastFingerprint(p.message, toastType) : '';

    if (dedupable && owned) {
      this.gate.prune(now);
      if (this.gate.shouldSkip(fp, now)) return;
    }

    // 同 key 原地更新（持久进度条目）
    if (p.key != null) {
      const idx = this.items.findIndex((it) => it.key === p.key);
      if (idx >= 0) {
        this.items[idx] = { ...this.items[idx], message: p.message, type: toastType, inplace: true };
        return;
      }
    }

    if (!owned) return;

    // 可见去重：上一条同内容还在屏幕上时，不再追加（兜底时间窗拦不住的「前一条 3s
    // 未关就来新一条」场景）。持久进度条目（数字 key）不参与可见去重。
    if (dedupable) {
      for (const it of this.items) {
        if (it.inplace) continue;
        if (toastFingerprint(it.message, it.type) === fp) return;
      }
    }

    this.items.push({ key: p.key, message: p.message, type: toastType, inplace: refresh });
    if (dedupable && owned) {
      this.gate.claim(fp, now);
      bus.emitShown({ char_id: this.charId, fp }, this);
    }
    this.keyTracker.note(p.key, now);
  }

  /** 对应 ToastWindow 的 `toast:shown` 监听 */
  onShown(e: ToastShown, now: number): void {
    if (this.dedupDisabled) return;
    if (e.char_id === this.charId) return;
    if (!this.gate.observe(e.char_id, e.fp, now)) return;
    this.items = this.items.filter(
      (it) => it.inplace || toastFingerprint(it.message, it.type) !== e.fp,
    );
  }

  /** 本窗口当前呈现的文案（用于断言） */
  messages(): string[] {
    return this.items.map((it) => it.message);
  }
}

/**
 * 广播总线。
 *
 * `toast:shown` 被**延后到本轮 toast:show 全部处理完之后**再投递，刻意模拟最坏时序：
 * 两个窗口都先各自决定要渲染、之后才看到对方的认领。真实并发下正是这个顺序，
 * 也是「乐观渲染 + 让位」这条路径必须成立的原因。
 */
class Bus {
  wins: Win[] = [];
  private pending: Array<{ e: ToastShown; sender: Win }> = [];

  deliverShow(p: ToastShow, now: number): void {
    for (const w of this.wins) w.onShow(p, now, this);
    this.flush(now);
  }

  emitShown(e: ToastShown, sender: Win): void {
    this.pending.push({ e, sender });
  }

  private flush(now: number): void {
    const queue = this.pending;
    this.pending = [];
    for (const { e, sender } of queue) {
      for (const w of this.wins) {
        if (w !== sender) w.onShown(e, now);
      }
    }
  }

  totalToasts(): number {
    return this.wins.reduce((n, w) => n + w.items.length, 0);
  }

  windowsShowing(message: string): string[] {
    return this.wins.filter((w) => w.messages().includes(message)).map((w) => w.charId);
  }
}

// ── 断言工具 ────────────────────────────────────────────────
let failures = 0;

function check(name: string, actual: unknown, expected: unknown): void {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) {
    console.log(`  ok   ${name}`);
  } else {
    failures += 1;
    console.log(`  FAIL ${name}\n       期望 ${e}\n       实际 ${a}`);
  }
}

/** 建一个「vivian 为主角色、两个桌宠在线」的场景 */
function scenario(): { bus: Bus; vivian: Win; nana: Win } {
  const bus = new Bus();
  const vivian = new Win('vivian');
  const nana = new Win('nana');
  vivian.isPrimary = true;
  bus.wins = [vivian, nana];
  return { bus, vivian, nana };
}

const BALANCE = 'LLM 账户余额不足，请充值后再试';

console.log('场景 A：广播事件，两个角色窗口各自以自身身份 showToast（用户报的原始症状）');
{
  const { bus, vivian, nana } = scenario();
  // 旧版行为：llm:error 不带 character_id，两个角色窗口都通过守卫，
  // 各自 showToast → 各自以自身角色 emit
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'nana' }, 1002);
  check('屏幕上只剩一条', bus.totalToasts(), 1);
  check('胜出者是优先级更高的 vivian', bus.windowsShowing(BALANCE), ['vivian']);
  check('nana 那条已让位', nana.messages().length, 0);
  check('vivian 那条保留', vivian.messages().length, 1);
}

console.log('场景 B：有归属的事件只弹该角色窗口');
{
  const { bus } = scenario();
  bus.deliverShow({ message: '已添加待办：买牛奶', type: 'success', character_id: 'nana' }, 1000);
  check('只弹一条', bus.totalToasts(), 1);
  check('落在 nana 窗口', bus.windowsShowing('已添加待办：买牛奶'), ['nana']);
}

console.log('场景 C：无归属事件（共享窗口/全局后台）只弹主角色窗口');
{
  const { bus } = scenario();
  bus.deliverShow({ message: '正在重建向量索引 3/10', type: 'info' }, 1000);
  check('只弹一条', bus.totalToasts(), 1);
  check('落在主角色 vivian 窗口', bus.windowsShowing('正在重建向量索引 3/10'), ['vivian']);
}

console.log('场景 D：不同内容互不干扰（不能过度去重）');
{
  const { bus } = scenario();
  bus.deliverShow({ message: '已添加待办：买牛奶', type: 'success', character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: '已添加待办：遛狗', type: 'success', character_id: 'nana' }, 1001);
  check('两条都保留', bus.totalToasts(), 2);
}

console.log('场景 E：带数字 key 的持久进度条目不受内容去重影响');
{
  const { bus, vivian } = scenario();
  const KEY = 99002;
  bus.deliverShow({ message: '正在重建向量索引 1/10', type: 'info', key: KEY, character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: '正在重建向量索引 2/10', type: 'info', key: KEY, character_id: 'vivian' }, 1400);
  bus.deliverShow({ message: '正在重建向量索引 3/10', type: 'info', key: KEY, character_id: 'vivian' }, 1800);
  check('始终只有一条条目', vivian.items.length, 1);
  check('文案原地刷新', vivian.messages(), ['正在重建向量索引 3/10']);
  check('未产生额外窗口', bus.totalToasts(), 1);
}

console.log('场景 F：同一窗口内短时间内重复收到同一内容，只留一条');
{
  const { bus, vivian } = scenario();
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1050);
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1100);
  check('只留一条', vivian.items.length, 1);
}

console.log('场景 G：超出时间窗后同一内容可以再次提示（不能永久静音）');
{
  const { bus, vivian } = scenario();
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1000);
  vivian.items = []; // 模拟第一条已自动关闭
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1000 + DEDUP_WINDOW_MS + 1);
  check('时间窗后可再次提示', vivian.items.length, 1);
}

console.log('场景 H：三个角色同时重复（扩展性）');
{
  const bus = new Bus();
  const vivian = new Win('vivian');
  const nana = new Win('nana');
  const chloe = new Win('chloe');
  vivian.isPrimary = true;
  bus.wins = [vivian, nana, chloe];
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'nana' }, 1001);
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'chloe' }, 1002);
  check('三个窗口只留一条', bus.totalToasts(), 1);
  check('胜出者 vivian', bus.windowsShowing(BALANCE), ['vivian']);
}

console.log('场景 I：间隔超过 2s 时间窗但前一条还在屏幕上（可见去重兜底）');
{
  // 用户的真实症状：余额不足连续触发，前一条 3s 自动关闭尚未到期时新一条又来了。
  // 时间窗已经过期，可见去重仍必须拦住——否则屏幕上又会叠出第二条。
  const { bus, vivian } = scenario();
  const t0 = 1000;
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, t0);
  // t0 + 2500ms：已超出 2s 时间窗，前一条仍未被关闭（toast 默认 3s 自动关闭）
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, t0 + 2500);
  // t0 + 2900ms：再次重发
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, t0 + 2900);
  check('前一条还在屏幕时新一条被拦下', vivian.items.length, 1);
  // 关闭前一条后再来：应该能再弹（不能让错误提示永久静音）
  vivian.items = [];
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, t0 + 6000);
  check('前一条已关闭后能再次提示', vivian.items.length, 1);
}

console.log('场景 J：真实 payload 形状——一次性提示自带 key: Date.now()');
{
  // 这是**当初漏掉这条 bug 的原因**：前面的用例都喂不带 key 的理想 payload，
  // 而线上每一条 toast 都带着 showToast 自动补的 key: Date.now()。旧判据据此
  // 把「带 key 的条目」整体豁免，等于去重网全线关闭 → 两只桌宠各弹一条。
  // 这里用生产形状重跑一遍最关键的几个场景。
  const { bus, vivian, nana } = scenario();
  bus.deliverShow({ message: BALANCE, type: 'error', key: oneShotKey(), character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: BALANCE, type: 'error', key: oneShotKey(), character_id: 'nana' }, 1002);
  check('两个窗口各自以自身身份发出（带一次性 key）仍只剩一条', bus.totalToasts(), 1);
  check('落点仍是优先级更高的 vivian', bus.windowsShowing(BALANCE), ['vivian']);
}
{
  const { bus, vivian } = scenario();
  bus.deliverShow({ message: '坐标已更新', type: 'success', key: oneShotKey() }, 1000);
  bus.deliverShow({ message: '坐标已更新', type: 'success', key: oneShotKey() }, 1010);
  check('无归属 + 一次性 key：同一内容也只留一条', bus.totalToasts(), 1);
  check('落在主角色窗口', vivian.messages(), ['坐标已更新']);
}
{
  const bus = new Bus();
  const vivian = new Win('vivian');
  const nana = new Win('nana');
  const chloe = new Win('chloe');
  vivian.isPrimary = true;
  bus.wins = [vivian, nana, chloe];
  bus.deliverShow({ message: BALANCE, type: 'error', key: oneShotKey(), character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: BALANCE, type: 'error', key: oneShotKey(), character_id: 'nana' }, 1001);
  bus.deliverShow({ message: BALANCE, type: 'error', key: oneShotKey(), character_id: 'chloe' }, 1002);
  check('三个角色同时（带一次性 key）仍只留一条', bus.totalToasts(), 1);
}

console.log('场景 K：key 重复出现 = 原地刷新条目，不受内容去重影响');
{
  const { bus, vivian } = scenario();
  const KEY = 4242;
  bus.deliverShow({ message: '正在重建向量索引 1/3', type: 'info', key: KEY, character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: '正在重建向量索引 2/3', type: 'info', key: KEY, character_id: 'vivian' }, 1400);
  bus.deliverShow({ message: '正在重建向量索引 3/3', type: 'info', key: KEY, character_id: 'vivian' }, 1800);
  check('始终只有一条条目', vivian.items.length, 1);
  check('文案原地刷新（没有被去重冻住）', vivian.messages(), ['正在重建向量索引 3/3']);
  check('确实被识别为原地刷新条目', vivian.items[0]?.inplace, true);
}

console.log('对照组：关掉去重网后，重复确实会重现（证明上面的断言不是空转）');
{
  const bus = new Bus();
  const vivian = new Win('vivian');
  const nana = new Win('nana');
  vivian.isPrimary = true;
  vivian.dedupDisabled = true;
  nana.dedupDisabled = true;
  bus.wins = [vivian, nana];
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'vivian' }, 1000);
  bus.deliverShow({ message: BALANCE, type: 'error', character_id: 'nana' }, 1002);
  check('无去重网时两条并存（复现用户症状）', bus.totalToasts(), 2);
  check('两个窗口各一条', bus.windowsShowing(BALANCE), ['vivian', 'nana']);
}

console.log('');
if (failures === 0) {
  console.log('全部通过：同一内容在屏幕上只会存在一条。');
} else {
  console.log(`${failures} 项失败。`);
  process.exit(1);
}
