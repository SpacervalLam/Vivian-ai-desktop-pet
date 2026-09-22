/**
 * 角色状态机——决定"现在去哪个热点、站多久、走多快"。
 *
 * 暂不接后端情绪与世界时间（那是 P2/P3），先保证每个角色独立跑：
 *   idle → 选一个非自身当前热点 → 走过去 → 站 N 秒 → 回去 idle
 * 坐类热点多两拍：
 *   idle → 走到站立落点 → mount（坐下去，同时挪到坐面并抬到坐面高度）
 *        → stay → unmount（站起来退回站立落点）→ idle
 *
 * "走路"是位置插值 + 朝向旋转，不靠真走路动画（角色是静态 3D 网格，
 * 绑骨/动画在 P2 再加）。后续可以把 8 方向 sprite billboard 加上去做
 * 走路视觉，但位置本身是真 3D。
 */

import { HOTSPOTS, findHotspot, type Hotspot } from './hotspots';
import { findPath, buildGrid, buildObstacles, clearanceExactAt, type AvoidCircle, type NavPoint } from './navGrid';
import type { NavPortal, NavWorld, RouteLeg } from './navWorld';
import layoutData from '../dormLayout.json';

export type AgentState = 'idle' | 'walk' | 'mount' | 'stay' | 'unmount' | 'yield' | 'lift';

type Vec3 = { x: number; y: number; z: number };

/** 落座/起身的最短时长（秒）——太短会像瞬移，太长会像在滑行。 */
const MOUNT_MIN = 0.45;
const MOUNT_MAX = 1.2;

/**
 * 减速距离：对方进入这个距离就降速。
 */
const PEER_SLOW = 1.6;
/**
 * 让行判定距离——比减速远得多。
 *
 * 两人相对速度 1.6m/s，从 1.3m 到擦身只有 0.8s，而让行方「退到宽敞处」要走 1~3m、
 * 花 1~2s：等靠近了才判让行，永远来不及，只能眼睁睁看着对方从身上穿过去。
 * 提前到 2.6m（≈1.9s 提前量）才够走完退避动作。
 */
const PEER_YIELD = 2.6;
/** 查路时对方的排斥半径——绕开人，而不是擦身而过。 */
const PEER_CLEAR = 0.65;
/**
 * 让行的最长等待（秒）。
 * 让行方站定后，对方要走过「相遇点 + 再走远」约 2.5m，按 0.8m/s 就是 3s 出头；
 * 卡在 3s 会在对方走到身边时正好超时恢复，两边一起动 = 直接撞上。
 */
const YIELD_TIMEOUT = 6.0;
/** 让路点重估间隔（秒）——对方一直在动，钉死一个位置可能正好挡在它要走的线上。 */
const SIDE_RECHECK = 0.4;
/**
 * 让行方优先「退到宽敞处」而不是原地侧移。
 *
 * 1.2m 的走廊里两个角色各贴一侧也只有 0.68m 间距，而臂展 0.82——原地怎么让都会叠。
 * 而走廊两侧每隔一两米就是房间门洞，退进门口/房间里才是真的让开。找不到这种点
 * （比如正处在长走道中段）才退回侧移，至少不站在对方身上。
 */
const REFUGE_CLEAR = 0.7;
const REFUGE_RANGE = 3.0;
/**
 * 近处有人时的重规划间隔（秒）。
 * 0.25s：对方让路只要 0.6s 左右，间隔太长就会拿「对方还在半路上」的位置去算，
 * 算出来的绕行等于没算。A* 单次几毫秒，这个频率在 20Hz tick 下扛得住。
 */
const REPLAN_INTERVAL = 0.25;
/** 卡住判定：这么久没动过就重规划（0.05s × 0.8m/s = 0.04m/tick，正常不会误判）。 */
const STUCK_TIME = 1.2;
/**
 * 让行时的侧移距离（米）。
 *
 * 光站着不动解决不了问题：1.2m 走廊里两个角色的臂展方向（左右）有 0.82m，
 * 让行方杵在中线时对方只能擦着过去（中心距 0.34，躯干重叠）。往通道一侧挪
 * 半步，两个人的中心距就拉开到 0.7 上下——不必改房间，也不必把角色缩得不像人。
 */
const SIDE_STEP = 0.34;
/** 侧移速度（米/秒）——比走路慢一点，像是"挪个位置"。 */
const SIDE_SPEED = 0.55;
/** 路过让行方时自己靠边的幅度（米）——比让行方小，两边各让一半。 */
const DODGE_STEP = 0.26;
/** 两个角色碰撞体之间的硬性最小间距（米）：躯干直径 0.32 + 余量。见 separate()。 */
export const SEPARATION = 0.34;

export class PetAgent {
  readonly id: string;
  /** 当前世界坐标。y 站定时恒为 0，坐在家具上时抬到坐面高度。 */
  pos: Vec3;
  /** 当前朝向（弧度） */
  facing: number;
  state: AgentState = 'idle';
  /** 走路目标点列表（世界坐标） */
  path: NavPoint[] = [];
  /** 走到路径中的第几段 */
  pathIdx = 0;
  /** 当前热点 id（idle 时待在的地方） */
  currentHotspotId: string | null = null;
  /** stay 状态剩余秒数 */
  stayTimer = 0;
  /** idle 状态已等多久（秒）—— 防抖：选下一个热点前先停一下 */
  idleTimer = 0;
  /** 走路速度（米/秒） */
  walkSpeed = 0.8;
  /** Loaded sitting clip duration; zero for models without skeletal animation. */
  sitDuration = 0;
  /** 转向速度（弧度/秒） */
  turnSpeed = 3.0;

  /**
   * 同伴（同一场景里的其他角色）——互相避让用。
   *
   * 避让拆成两层，因为一层解决不了：
   *   - 规划层：查路时把对方当排斥圆，路自然绕着走（静态的那份栅格塞不进会动的人）；
   *   - 执行层：1.2m 走廊里两个身宽 0.6 的角色头对头时，几何上就是错不开——规划层无解，
   *     只能靠让行规则裁决谁先过，另一方站着等。
   */
  peers: PetAgent[] = [];
  /** 让行已等多久（秒），以及距上次重估让路点过了多久。 */
  private yieldTimer = 0;
  private sideTimer = 0;
  /** 让行时挪到的位置（null = 还没算/不用挪）。 */
  private sideTarget: { x: number; z: number } | null = null;
  /** 路过正在让路的一方时，自己往边上靠的临时落点。 */
  private dodgeTarget: { x: number; z: number } | null = null;
  /** 本次行程的终点（热点落点）与所在层。让行会临时借用 path，所以目标单独留一份。 */
  private goal: NavPoint | null = null;
  private goalLayer = 'apt2';
  /** 让行期间「退避」用的临时路径：退到宽敞处等着，让完再返回原目标。 */
  private yieldPath: NavPoint[] | null = null;
  private yieldIdx = 0;
  /** 距上次重规划过了多久（秒）。 */
  private replanTimer = 0;
  /** 卡住计时与上一 tick 位置。 */
  private stuckTimer = 0;
  private lastStep = { x: 0, z: 0 };

  /** 坐姿抬升量（0 = 站着）。坐在家具上时 = 坐面相对本层地面的高度。 */
  seatLift = 0;

  /* ---------------- 跨层行程 ---------------- */
  /** 当前所在导航层。 */
  layerId = 'apt2';
  /** 本次行程的完整路线（跨层时不止一段）。 */
  private legs: RouteLeg[] = [];
  private legIdx = 0;
  /** 正在走的楼梯门户（非空时脚下的 y 由坡面 heightAt 给）。 */
  private onStair: NavPortal | null = null;
  /** 走完坡面之后要接上的那一段。 */
  private pendingLeg = 0;
  /** 电梯等待计时与目标层（电梯那段在真实楼梯系统里不驱动轿厢，见 tick 注释）。 */
  private liftTimer = 0;
  private liftDuration = 0;
  private liftToY = 0;

  /** 落座/起身插值进度 [0,1] 与时长（秒）。 */
  private mountT = 0;
  private mountDur = MOUNT_MIN;
  /** 插值起点：落座时是站立落点，起身时是坐面落点。 */
  private mountFrom: Vec3 = { x: 0, y: 0, z: 0 };
  private mountTo: Vec3 = { x: 0, y: 0, z: 0 };
  private mountFromFacing = 0;
  private mountToFacing = 0;

  /**
   * 热点占用表：热点 id → 角色 id。
   *
   * 两个角色共用一份热点表，没有占用登记时它们会各自挑到同一个沙发坐垫，
   * 然后一个坐在另一个身上。占位只在「选目标」时生效，抬起屁股（unmount 结束）才释放。
   */
  private static claims = new Map<string, string>();

  // 复用一份预计算的栅格。布局驱动：外墙整段实心，内墙按落地门洞断开。
  // nav:false 的家具（窗、地毯、门扇这类不该挡路的东西）建栅格前先剔掉。
  private static readonly gridRef = (() => {
    const layout = layoutData as any;
    return buildGrid(buildObstacles(layout), layout.room.bounds);
  })();

  /** 导航栅格（给验收脚本核对热点可走性用，只读）。 */
  static get navGrid() { return this.gridRef; }

  /**
   * 跨场景导航世界（多层栅格 + 门户）。**运行时注入**。
   *
   * 为空时角色退回只走 203 那张单层栅格——这条回退路径不是摆设：纯逻辑环境
   * （验收哨兵的仿真、单元测试）里没有 three、也没有场景装配，建不出 navWorld，
   * 角色仍要能跑。
   */
  private static world: NavWorld | null = null;
  static bindWorld(w: NavWorld | null) { this.world = w; }

  /** 场景重建时清空占用表——claims 是静态的，会跨挂载存活，旧占用会挡住新场景。 */
  static resetClaims() { this.claims.clear(); }

  private static get grid() { return this.gridRef; }

  /** 当前层的地面标高（世界坐标）。 */
  private get layerY(): number {
    return PetAgent.world?.layer(this.layerId)?.y ?? 0;
  }

  /**
   * 切换到某一层：位置跟着该层地面走。
   * 角色坐标现在是**世界坐标**（挂 scene，不挂被抬高 3.4 的 unitGroup），
   * 所以「站在哪一层」这件事必须显式同步，否则会飘在楼板上面 3.4m。
   */
  setLayer(id: string) {
    this.layerId = id;
    this.seatLift = 0;
    const y = PetAgent.world?.layer(id)?.y;
    if (y !== undefined) this.pos.y = y;
  }

  /** 当前所在层的栅格：外廊/街上的让路与避让判定必须按那一层算，用室内那张会越界。 */
  private get gridNow() {
    return PetAgent.world?.layer(this.layerId)?.grid ?? PetAgent.grid;
  }

  constructor(id: string, startPos: [number, number, number], startFacing: number) {
    this.id = id;
    this.pos = { x: startPos[0], y: startPos[1], z: startPos[2] };
    this.facing = startFacing;
  }

  /** 房间窗口 tick——固定 dt 0.05s = 20Hz。 */
  tick(dt: number) {
    switch (this.state) {
      case 'idle':
        this.idleTimer += dt;
        // 至少停 1.5s，再换地方
        if (this.idleTimer >= 1.5) {
          this.pickNewDestination();
        }
        break;
      case 'walk': {
        if (this.pathIdx >= this.path.length) {
          this.advanceLeg();
          return;
        }
        // 迎面且对方没打算让 → 我让。id 序是固定规则，两边同时判也只有一个会停。
        const blocker = this.peerBlocking(PEER_YIELD);
        if (blocker && this.id > blocker.id) {
          this.beginYield(blocker);
          return;
        }
        // 近处有人：按距离减速 + 定期重规划（否则会照着「对方还没站过来」的旧路径撞上去）
        const near = this.nearestPeerDist();
        this.replanTimer += dt;
        if (near < PEER_SLOW * 1.7 && this.replanTimer >= REPLAN_INTERVAL) {
          this.replan();
        }
        this.trackStuck(dt);

        /**
         * 对方正在让路时自己也要往边上靠。
         *
         * 单靠让行方贴边，错身间距只有「让行方位移」那一点；两边各让一半，
         * 间距才撑得开。走廊里这零点几米就是「头发擦过」和「整个人穿过去」的区别。
         */
        const yielder = this.peers.find(p =>
          p.state === 'yield' &&
          Math.hypot(p.pos.x - this.pos.x, p.pos.z - this.pos.z) < PEER_SLOW
        );
        if (yielder) this.nudgeAside(dt, yielder);
        else this.dodgeTarget = null;

        const target = this.path[this.pathIdx];
        const dx = target.x - this.pos.x;
        const dz = target.z - this.pos.z;
        const dist = Math.hypot(dx, dz);
        // 先转向目标
        const desiredFacing = Math.atan2(dx, dz);
        const turnDiff = shortestAngleDiff(this.facing, desiredFacing);
        const turnAmt = Math.sign(turnDiff) * Math.min(Math.abs(turnDiff), this.turnSpeed * dt);
        this.facing += turnAmt;
        // 走完一段再切换。阈值要小：提前 5cm 就转下一个路点等于把每个拐角都啃掉 5cm，
        // 拉绳平滑后的路点很稀，被啃掉的正好是贴着门框/墙角的那一段。
        if (dist < 0.02) {
          this.pathIdx++;
          return;
        }
        const step = Math.min(dist, this.walkSpeed * this.speedScale(near) * dt);
        this.pos.x += (dx / dist) * step;
        this.pos.z += (dz / dist) * step;
        // 脚底高度：平地取当前层标高，楼梯上现算坡面高度——上下楼是连续插值，
        // 不是到了某一层突然跳一下（跳变在渲染上就是"人从半空掉下来"）。
        this.pos.y = this.onStair
          ? (this.onStair.heightAt?.(this.pos.x, this.pos.z) ?? this.pos.y)
          : (this.legs[this.legIdx]?.y ?? this.pos.y);
        break;
      }
      case 'lift': {
        /**
         * 乘梯：等轿厢来 → 上去 → 到站。
         *
         * 这里**不驱动轿厢模型**：真实那套电梯（apartmentLift.ts）是给第一人称写的
         * ——`selectFloor` 要求相机在轿厢里、到站时把相机传送到目标层，角色没法用。
         * 而在"相机不跟随"的前提下，角色坐电梯的全过程根本不在画面里，为它改一套
         * 双向可控的轿厢状态机是过度投入。所以这里按行程时长等待，到点直接换层。
         * 哪天要做相机跟随，再把这段换成真正呼叫轿厢。
         */
        this.liftTimer += dt;
        if (this.liftTimer >= this.liftDuration) {
          this.legIdx++;
          const next = this.legs[this.legIdx];
          this.pos.y = this.liftToY;
          if (!next) { this.beginMountOrStay(); break; }
          this.path = next.path;
          this.pathIdx = 0;
          this.layerId = next.layer;
          this.state = 'walk';
          this.stuckTimer = 0;
          this.lastStep = { x: this.pos.x, z: this.pos.z };
        }
        break;
      }
      case 'yield': {
        this.yieldTimer += dt;
        const b0 = this.nearestPeer();
        if (this.yieldPath && this.yieldPath.length) {
          // 退避中：像走路一样挪过去，到了就站着等
          const t = this.yieldPath[this.yieldIdx];
          if (!t || (Math.hypot(t.x - this.pos.x, t.z - this.pos.z) < 0.02 && ++this.yieldIdx >= this.yieldPath.length)) {
            this.yieldPath = null;   // 到位，站着等
          } else {
            const dx = t.x - this.pos.x, dz = t.z - this.pos.z;
            const d = Math.hypot(dx, dz);
            const want = Math.atan2(dx, dz);
            const diff = shortestAngleDiff(this.facing, want);
            this.facing += Math.sign(diff) * Math.min(Math.abs(diff), this.turnSpeed * dt);
            if (d > 0.02) {
              const step = Math.min(d, this.walkSpeed * 0.75 * dt);
              this.pos.x += (dx / d) * step;
              this.pos.z += (dz / d) * step;
            }
          }
        } else if (b0) {
          // 没找到退避处（长走道中段）：退回原地侧移，至少别站在对方身上
          this.sideTimer += dt;
          if (this.sideTimer >= SIDE_RECHECK) {
            this.sideTimer = 0;
            const next = this.pickSideStep(b0);
            // 滞回：新落点要明显更靠外才换，否则会在通道两边来回跳、对方跟着来回绕
            if (next && (!this.sideTarget || this.lateralScore(next, b0) > this.lateralScore(this.sideTarget, b0) * 1.15)) {
              this.sideTarget = next;
            }
          }
          this.stepAside(dt, b0);
        }
        if (this.peerAlreadyPassed() || this.yieldTimer >= YIELD_TIMEOUT) {
          // 对方过去了（或等太久兜底）就按它的新位置重规划，别照着旧路径继续走
          this.replanTimer = 0;
          this.replan();
          this.state = this.path.length ? 'walk' : 'idle';
          this.stuckTimer = 0;
          this.yieldTimer = 0;
          this.sideTarget = null;
          this.yieldPath = null;
        }
        break;
      }
      case 'mount':
      case 'unmount': {
        this.mountT = Math.min(1, this.mountT + dt / this.mountDur);
        const k = easeInOut(this.mountT);
        this.pos.x = this.mountFrom.x + (this.mountTo.x - this.mountFrom.x) * k;
        this.pos.z = this.mountFrom.z + (this.mountTo.z - this.mountFrom.z) * k;
        this.pos.y = this.mountFrom.y + (this.mountTo.y - this.mountFrom.y) * k;
        this.facing = this.mountFromFacing + shortestAngleDiff(this.mountFromFacing, this.mountToFacing) * k;
        if (this.mountT >= 1) {
          if (this.state === 'mount') {
            this.state = 'stay';
            this.seatLift = this.pos.y - this.layerY;
            this.stayTimer = 2 + Math.random() * 4; // 2-6 秒
            if (this.currentHotspotId) {
              const h = findHotspot(this.currentHotspotId);
              if (h?.animation === 'sit' && this.sitDuration > 0) {
                this.stayTimer = Math.max(this.stayTimer, this.sitDuration + 1);
              }
            }
          } else {
            // 起身完毕：退回站立落点，交还热点占用，进入 idle
            this.releaseClaim();
            this.seatLift = 0;
            this.state = 'idle';
            this.idleTimer = 0;
          }
        }
        break;
      }
      case 'stay': {
        this.stayTimer -= dt;
        /**
         * 站在通道里的角色也要让路。
         *
         * 不然对方只能从它身上穿过去——走廊才 1.2m，一个杵在中线的角色就把路堵死了。
         * 坐着的（沙发/床/椅子）不让：热点在家具上，本来就不在通道里。
         */
        if (!this.seatOf(this.currentHotspotId)) {
          const b = this.peerBlocking(PEER_SLOW * 0.8);
          if (b) this.stepAside(dt, b);
          else this.sideTarget = null;
        }
        if (this.stayTimer <= 0) {
          if (this.seatOf(this.currentHotspotId)) this.beginUnmount();
          else { this.state = 'idle'; this.idleTimer = 0; }
        }
        break;
      }
    }
  }

  private seatOf(id: string | null) {
    if (!id) return null;
    return findHotspot(id)?.seat ?? null;
  }

  /* ---------------- 互相避让 ---------------- */

  /** 场景装配后互绑，让行与绕人才生效。 */
  static bindPeers(list: PetAgent[]) {
    for (const a of list) a.peers = list.filter(b => b !== a);
  }

  /**
   * 分离约束：两个角色的碰撞体不许互相插入（每个 tick 调一次）。
   *
   * 为什么要有这一层硬约束：A* 绕人 + 让行 + 侧移都是"尽量不撞"的启发式，
   * 而 1.2m 走廊里两个臂展 0.82 的角色在几何上就错不开，启发式时好时坏。
   * 分离约束是确定性的：距离不足就把两者沿连线各推开一半，永远收敛。
   * 代价是会把角色挤离 A* 路径——下一 tick 的寻路又会拉回去，视觉上就是"侧身挤过去"。
   *
   * 门槛 0.34 取躯干直径（两角色各 0.16 半宽 = 0.32）加一点余量。
   * 想连头发/裙摆都不交叠需要 0.82，走廊只有 1.2m 宽——那个目标要么缩角色到 1.1m 以下，
   * 要么把走廊加宽，不是算法能补的。
   */
  static separate(list: PetAgent[]) {
    for (let i = 0; i < list.length; i++) {
      for (let k = i + 1; k < list.length; k++) {
        const a = list[i], b = list[k];
        // 坐在家具上的（沙发/床/椅子）不参与推挤：热点位置是数据给的，推了就不坐在上面了
        if (a.seatLift > 0.01 || b.seatLift > 0.01) continue;
        const dx = b.pos.x - a.pos.x, dz = b.pos.z - a.pos.z;
        const d = Math.hypot(dx, dz);
        if (d >= SEPARATION || d < 1e-4) continue;
        const ux = dx / d, uz = dz / d;
        const push = (SEPARATION - d) / 2;
        const ax = a.pos.x - ux * push, az = a.pos.z - uz * push;
        const bx = b.pos.x + ux * push, bz = b.pos.z + uz * push;
        // 推完贴墙就放弃那一半：宁可两个角色挤一点，也不能把人推进墙里
        const ga = PetAgent.world?.layer(a.layerId)?.grid ?? PetAgent.grid;
        const gb = PetAgent.world?.layer(b.layerId)?.grid ?? PetAgent.grid;
        if (clearanceExactAt(ax, az, ga) >= ga.radius - 0.02) { a.pos.x = ax; a.pos.z = az; }
        if (clearanceExactAt(bx, bz, gb) >= gb.radius - 0.02) { b.pos.x = bx; b.pos.z = bz; }
      }
    }
  }

  /** 其他角色当前位置 → A* 的排斥圆。 */
  private avoidList(): AvoidCircle[] {
    return this.peers.map(p => ({ x: p.pos.x, z: p.pos.z, r: PEER_CLEAR }));
  }

  private nearestPeerDist(): number {
    let best = Infinity;
    for (const p of this.peers) {
      const d = Math.hypot(p.pos.x - this.pos.x, p.pos.z - this.pos.z);
      if (d < best) best = d;
    }
    return best;
  }

  private nearestPeer(): PetAgent | null {
    let best: PetAgent | null = null;
    let bestD = Infinity;
    for (const p of this.peers) {
      const d = Math.hypot(p.pos.x - this.pos.x, p.pos.z - this.pos.z);
      if (d < bestD) { bestD = d; best = p; }
    }
    return best;
  }

  /**
   * 对方是否已经越过我。
   *
   * 用「相对我朝目标的连线」判前后，而不是「离我多远」——按距离判会让让行方
   * 等对方走到 1.5m 外，而对方恰恰是在我身边经过后才走远，等待时间被白白拉长。
   * 只看 2.5m 内的对象：远处的同伴和这次错身无关。
   */
  private peerAlreadyPassed(): boolean {
    const goal = this.goal ?? this.path[this.path.length - 1];
    if (!goal) return true;
    const gx = goal.x - this.pos.x, gz = goal.z - this.pos.z;
    const gd = Math.hypot(gx, gz) || 1;
    if (gd < 0.4) return true;   // 我马上就到终点了，没必要再等
    for (const p of this.peers) {
      const dx = p.pos.x - this.pos.x, dz = p.pos.z - this.pos.z;
      if (Math.hypot(dx, dz) > 2.5) continue;
      if ((dx * gx + dz * gz) / gd < -0.3) return true;
    }
    return false;
  }

  /**
   * 迎面而来的同伴：两个人都朝对方走。
   *
   * 判据故意用「各自朝向在连线上的投影」而不是「对方在我正前方 90° 内」——
   * 后者在侧向擦身时会漏判，而侧向擦身恰恰是最容易穿模的那一刻。
   * 同向跟随 / 一方停着都不算迎面：那种情况绕一下或减速就过去了，停下来更怪。
   */
  private peerBlocking(range = PEER_YIELD): PetAgent | null {
    for (const p of this.peers) {
      const dx = p.pos.x - this.pos.x, dz = p.pos.z - this.pos.z;
      const d = Math.hypot(dx, dz);
      if (d > range || d < 1e-4) continue;
      const iApproach = (dx * Math.sin(this.facing) + dz * Math.cos(this.facing)) / d;
      const peerApproach = (-dx * Math.sin(p.facing) - dz * Math.cos(p.facing)) / d;
      if (iApproach > 0.25 && peerApproach > 0.25) return p;
    }
    return null;
  }

  /**
   * 挑一个「挪开半步」的落点：垂直于对方来向的两侧各探一次，取净空更好的那侧。
   * 走廊里两侧通常差不多，房间口则明显偏向空的那边——不写死左右，让净空说话。
   */
  private pickSideStep(peer: PetAgent): { x: number; z: number } | null {
    const from = Math.atan2(this.pos.x - peer.pos.x, this.pos.z - peer.pos.z);
    const px = Math.sin(peer.facing), pz = Math.cos(peer.facing);  // 对方行进方向
    let best: { x: number; z: number; score: number } | null = null;
    /**
     * 距离分三档递退：走廊里"贴到自己那侧墙边"正好是硬半径的临界值，
     * 只试一档会因为差几毫米被净空判掉，结果角色原地不动、对方从身上穿过去。
     */
    for (const dist of [SIDE_STEP, SIDE_STEP * 0.72, SIDE_STEP * 0.45]) {
      for (const side of [Math.PI / 2, -Math.PI / 2]) {
        const ang = from + side;
        const x = this.pos.x + Math.sin(ang) * dist;
        const z = this.pos.z + Math.cos(ang) * dist;
        // 精确净空：栅格值是「最近格中心」的，半格误差会把贴墙 5cm 的位置判成能站
        const clear = clearanceExactAt(x, z, this.gridNow);
        // 允许 2cm 越界：这是临时让路，擦一点墙远好过站在原地被穿过去
        if (clear < this.gridNow.radius - 0.02) continue;
        /**
         * 打分看「离对方的行进轴线多远」而不是离对方本人多远。
         * 只按距离选，两边几乎一样时容易挑中对方正要走的那侧——两个人都往同一边让，
         * 结果还是撞上。横向错开对方的行进轴线，才是真的错开。
         */
        const lateral = Math.abs(px * (z - peer.pos.z) - pz * (x - peer.pos.x));
        const score = lateral * 2 + Math.min(clear, 1) * 0.5 - dist * 0.3;
        if (!best || score > best.score) best = { x, z, score };
      }
    }
    return best ? { x: best.x, z: best.z } : null;
  }

  /** 进入让行：先找宽敞处退避，找不到才原地侧移。 */
  private beginYield(peer: PetAgent) {
    this.state = 'yield';
    this.yieldTimer = 0;
    this.sideTimer = 0;
    this.sideTarget = null;
    this.yieldPath = null;
    this.yieldIdx = 0;
    const refuge = this.pickRefuge(peer);
    if (refuge) {
      const p = findPath({ x: this.pos.x, z: this.pos.z }, refuge, this.gridNow, this.avoidList());
      if (p && p.length) { this.yieldPath = p; return; }
    }
    this.sideTarget = this.pickSideStep(peer);
  }

  /**
   * 找一个「退到那儿就不挡路」的宽敞点。
   *
   * 判据三条：净空比通道宽（REFUGE_CLEAR）、离对方够远、横向躲开对方的行进轴线。
   * 全格扫描只有在进入让行时跑一次，几千格的量级，摊到 20Hz 上可以忽略。
   */
  private pickRefuge(peer: PetAgent): NavPoint | null {
    const g = this.gridNow;
    const px = Math.sin(peer.facing), pz = Math.cos(peer.facing);
    let best: { x: number; z: number; score: number } | null = null;
    for (let i = 0; i < g.res; i++) {
      for (let j = 0; j < g.resZ; j++) {
        if (g.blocked[i][j]) continue;
        const clear = g.clearance[i * g.resZ + j];
        if (clear < REFUGE_CLEAR) continue;
        const x = g.x0 + (i + 0.5) * g.cell;
        const z = g.z0 + (j + 0.5) * g.cell;
        const d = Math.hypot(x - this.pos.x, z - this.pos.z);
        if (d > REFUGE_RANGE) continue;
        if (d < 0.35) continue;                                  // 原地不动等于没让
        if (Math.hypot(x - peer.pos.x, z - peer.pos.z) < 1.2) continue;  // 别退到对方身上
        const lateral = Math.abs(px * (z - peer.pos.z) - pz * (x - peer.pos.x));
        const score = lateral * 1.5 + Math.min(clear, 1.2) * 0.6 - d;
        if (!best || score > best.score) best = { x, z, score };
      }
    }
    return best ? { x: best.x, z: best.z } : null;
  }

  /**
   * 路过让行方时自己靠边半步。
   * 落点按「离对方本人更远」挑（对方在让路时基本不动，用它的朝向定轴线没有意义）。
   */
  private nudgeAside(dt: number, peer: PetAgent) {
    if (!this.dodgeTarget) {
      const from = Math.atan2(this.pos.x - peer.pos.x, this.pos.z - peer.pos.z);
      let best: { x: number; z: number; score: number } | null = null;
      for (const side of [Math.PI / 2, -Math.PI / 2]) {
        const ang = from + side;
        const x = this.pos.x + Math.sin(ang) * DODGE_STEP;
        const z = this.pos.z + Math.cos(ang) * DODGE_STEP;
        if (clearanceExactAt(x, z, this.gridNow) < this.gridNow.radius - 0.02) continue;
        const d = Math.hypot(x - peer.pos.x, z - peer.pos.z);
        if (!best || d > best.score) best = { x, z, score: d };
      }
      if (!best) return;
      this.dodgeTarget = { x: best.x, z: best.z };
    }
    const t = this.dodgeTarget;
    const dx = t.x - this.pos.x, dz = t.z - this.pos.z;
    const d = Math.hypot(dx, dz);
    if (d <= 0.03) return;
    const step = Math.min(d, this.walkSpeed * 0.7 * dt);
    this.pos.x += (dx / d) * step;
    this.pos.z += (dz / d) * step;
  }

  /** 侧移让路：往通道一侧挪。返回是否还在挪。 */
  private stepAside(dt: number, peer: PetAgent): boolean {
    if (!this.sideTarget) this.sideTarget = this.pickSideStep(peer);
    const st = this.sideTarget;
    if (!st) return false;
    const dx = st.x - this.pos.x, dz = st.z - this.pos.z;
    const d = Math.hypot(dx, dz);
    if (d <= 0.03) { this.sideTarget = null; return false; }
    this.pos.x += (dx / d) * Math.min(d, SIDE_SPEED * dt);
    this.pos.z += (dz / d) * Math.min(d, SIDE_SPEED * dt);
    const want = Math.atan2(dx, dz);
    const diff = shortestAngleDiff(this.facing, want);
    this.facing += Math.sign(diff) * Math.min(Math.abs(diff), this.turnSpeed * dt);
    return true;
  }

  /** 某个候选点相对同伴行进轴线的横向距离——重估让路点时的比较用。 */
  private lateralScore(p: { x: number; z: number }, peer: PetAgent): number {
    const px = Math.sin(peer.facing), pz = Math.cos(peer.facing);
    return Math.abs(px * (p.z - peer.pos.z) - pz * (p.x - peer.pos.x));
  }

  /**
   * 近距减速：越近越慢；下限 0.3 保证不会彻底站死。
   * 对方正在让路时再压低一档——错身本来就要它先挪到位，抢那零点几秒只会撞上去。
   */
  private speedScale(nearest: number): number {
    if (nearest > PEER_SLOW) return 1;
    const base = Math.max(0.3, nearest / PEER_SLOW);
    const yielding = this.peers.some(p => {
      if (p.state !== 'yield') return false;
      return Math.hypot(p.pos.x - this.pos.x, p.pos.z - this.pos.z) < 2.5;
    });
    return yielding ? Math.min(base, 0.45) : base;
  }

  private trackStuck(dt: number) {
    const moved = Math.hypot(this.pos.x - this.lastStep.x, this.pos.z - this.lastStep.z);
    this.lastStep = { x: this.pos.x, z: this.pos.z };
    this.stuckTimer = moved < 0.005 ? this.stuckTimer + dt : 0;
    if (this.stuckTimer >= STUCK_TIME) {
      this.stuckTimer = 0;
      this.replan();
    }
  }

  /**
   * 按同伴当前位置重算剩余路径。
   * 失败就保留原路径——宁可照着旧路线慢慢挪，也不能把角色晾在原地。
   */
  private replan() {
    this.replanTimer = 0;
    // 楼梯/电梯途中不重规划：那段没有"从当前位置到终点"的常规路线，重算会把行程打断
    if (this.onStair || this.state === 'lift') return;
    const goal = this.goal;
    if (!goal) return;
    this.planRoute(this.goalLayer, goal.x, goal.z);
  }

  /**
   * 当前这一段走完：处理门户、换层、接上下一段。
   *
   * 三种门户三种处理：
   *   - door：两侧都是平地，直接接下一段
   *   - stair：中间要**沿坡面走**，不是瞬移——先把坡顶设成临时目标，脚底高度在
   *            walk 里由 heightAt 现算，走到坡顶才接下一段
   *   - lift：等轿厢（见 tick 的 'lift' 分支）
   */
  private advanceLeg() {
    // 刚走完坡面：接上被楼梯隔开的那一段
    if (this.onStair) {
      this.onStair = null;
      this.legIdx = this.pendingLeg;
      const next = this.legs[this.legIdx];
      if (!next) { this.beginMountOrStay(); return; }
      this.path = next.path;
      this.pathIdx = 0;
      this.layerId = next.layer;
      this.pos.y = next.y;
      return;
    }

    const portal = this.legs[this.legIdx]?.portal;
    if (!portal) { this.beginMountOrStay(); return; }   // 没有门户 = 最后一段，到终点了

    if (portal.kind === 'lift') {
      const toLayer = portal.from.layer === this.layerId ? portal.to.layer : portal.from.layer;
      this.liftToY = PetAgent.world?.layer(toLayer)?.y ?? this.pos.y;
      // 行程时长沿用电梯那套曲线（起步 1.2s + 每层 1.5s），再算上等梯与开关门
      this.liftDuration = 2.6 + Math.abs(this.liftToY - this.pos.y) * 1.5;
      this.liftTimer = 0;
      this.state = 'lift';
      return;
    }

    if (portal.kind === 'stair') {
      const top = portal.from.layer === this.layerId ? portal.to : portal.from;
      this.pendingLeg = this.legIdx + 1;
      this.onStair = portal;
      this.path = [{ x: top.x, z: top.z }];
      this.pathIdx = 0;
      return;
    }

    this.legIdx++;
    const next = this.legs[this.legIdx];
    if (!next) { this.beginMountOrStay(); return; }
    this.path = next.path;
    this.pathIdx = 0;
    this.layerId = next.layer;
    this.pos.y = next.y;
  }

  /**
   * 规划到某个落点的路线。
   *
   * 有 navWorld 时走跨层路线（楼梯/电梯/门），没有就退回室内那张单层栅格——
   * 纯逻辑环境（验收哨兵）没有场景装配，建不出 navWorld。
   */
  private planRoute(layer: string, tx: number, tz: number): boolean {
    const W = PetAgent.world;
    if (!W) {
      const path = findPath({ x: this.pos.x, z: this.pos.z }, { x: tx, z: tz }, this.gridNow, this.avoidList());
      if (!path || !path.length) return false;
      this.legs = [{ layer: this.layerId, y: 0, path }];
      this.legIdx = 0;
      this.path = path;
      this.pathIdx = 0;
      return true;
    }
    const legs = W.route({
      from: { layer: this.layerId, x: this.pos.x, z: this.pos.z },
      to: { layer, x: tx, z: tz },
    });
    if (!legs || !legs.length) return false;
    this.legs = legs;
    this.legIdx = 0;
    this.path = legs[0].path;
    this.pathIdx = 0;
    this.layerId = legs[0].layer;
    this.onStair = null;
    return true;
  }

  /** 走完路：有坐点就落座，否则原地站一会。 */
  private beginMountOrStay() {
    const h = this.currentHotspotId ? findHotspot(this.currentHotspotId) : undefined;
    const seat = h?.seat;
    if (!seat) {
      this.state = 'stay';
      this.stayTimer = 2 + Math.random() * 4;
      if (h) this.facing = h.facing;
      return;
    }
    this.mountFrom = { x: this.pos.x, y: this.pos.y, z: this.pos.z };
    // seat.pos[1] 是相对楼板的抬升量，加上本层标高才是世界高度
    this.mountTo = { x: seat.pos[0], y: this.layerY + seat.pos[1], z: seat.pos[2] };
    this.mountFromFacing = this.facing;
    this.mountToFacing = seat.facing ?? h!.facing;
    // 落座位移按走路速度折算时长：走过去多远，坐下去就花多久，别出现滑行
    const dist = Math.hypot(this.mountTo.x - this.mountFrom.x, this.mountTo.z - this.mountFrom.z);
    this.mountDur = Math.min(MOUNT_MAX, Math.max(MOUNT_MIN, dist / this.walkSpeed));
    this.mountT = 0;
    this.state = 'mount';
  }

  private beginUnmount() {
    const h = this.currentHotspotId ? findHotspot(this.currentHotspotId) : undefined;
    if (!h || !h.seat) { this.state = 'idle'; return; }
    this.mountFrom = { x: this.pos.x, y: this.pos.y, z: this.pos.z };
    this.mountTo = { x: h.pos[0], y: this.layerY, z: h.pos[2] };
    this.mountFromFacing = this.facing;
    this.mountToFacing = h.facing;
    const dist = Math.hypot(this.mountTo.x - this.mountFrom.x, this.mountTo.z - this.mountFrom.z);
    this.mountDur = Math.min(MOUNT_MAX, Math.max(MOUNT_MIN, dist / this.walkSpeed));
    this.mountT = 0;
    this.state = 'unmount';
  }

  private pickNewDestination() {
    const candidates = HOTSPOTS.filter(h => {
      if (h.id === this.currentHotspotId) return false;
      const owner = PetAgent.claims.get(h.id);
      return owner === undefined || owner === this.id;
    });
    if (candidates.length === 0) { this.idleTimer = 0; return; }
    // 随机选一个
    const next = candidates[Math.floor(Math.random() * candidates.length)];
    this.goal = { x: next.pos[0], z: next.pos[2] };
    this.goalLayer = next.layer ?? 'apt2';
    if (!this.planRoute(this.goalLayer, next.pos[0], next.pos[2])) {
      // 不可达：跳过，再选
      this.idleTimer = 0;
      return;
    }
    this.releaseClaim();
    this.claim(next);
    this.state = 'walk';
    this.currentHotspotId = next.id;
    this.idleTimer = 0;
    this.replanTimer = 0;
    this.stuckTimer = 0;
    this.lastStep = { x: this.pos.x, z: this.pos.z };
  }

  private claim(h: Hotspot) { PetAgent.claims.set(h.id, this.id); }

  private releaseClaim() {
    if (this.currentHotspotId && PetAgent.claims.get(this.currentHotspotId) === this.id) {
      PetAgent.claims.delete(this.currentHotspotId);
    }
  }

  /** 调试用：手动指定去某个热点。 */
  goTo(hotspotId: string) {
    const h = findHotspot(hotspotId);
    if (!h) return;
    this.goal = { x: h.pos[0], z: h.pos[2] };
    this.goalLayer = h.layer ?? 'apt2';
    if (!this.planRoute(this.goalLayer, h.pos[0], h.pos[2])) return;
    this.releaseClaim();
    this.claim(h);
    this.state = 'walk';
    this.replanTimer = 0;
    this.stuckTimer = 0;
    this.lastStep = { x: this.pos.x, z: this.pos.z };
    this.currentHotspotId = hotspotId;
  }

  get stateLabel(): string {
    return `${this.state}${this.currentHotspotId ? `@${this.currentHotspotId}` : ''}`;
  }
}

function shortestAngleDiff(from: number, to: number): number {
  let d = to - from;
  while (d >  Math.PI) d -= Math.PI * 2;
  while (d < -Math.PI) d += Math.PI * 2;
  return d;
}

/** 缓入缓出：起步和落座都要慢，中间快，否则像被弹簧弹上去。 */
function easeInOut(t: number): number {
  return t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
}
