/**
 * 角色状态机——决定"现在去哪个热点、站多久、走多快"。
 *
 * 暂不接后端情绪与世界时间（那是 P2/P3），先保证每个角色独立跑：
 *   idle → 选一个非自身当前热点 → 走过去 → 站 N 秒 → 回去 idle
 *
 * "走路"是位置插值 + 朝向旋转，不靠真走路动画（角色是静态 3D 网格，
 * 绑骨/动画在 P2 再加）。后续可以把 8 方向 sprite billboard 加上去做
 * 走路视觉，但位置本身是真 3D。
 */

import { HOTSPOTS, findHotspot, type Hotspot } from './hotspots';
import { findPath, buildGrid, buildObstacles, type NavPoint } from './navGrid';
import layoutData from '../dormLayout.json';

export type AgentState = 'idle' | 'walk' | 'stay';

type Vec3 = { x: number; y: number; z: number };

export class PetAgent {
  readonly id: string;
  /** 当前世界坐标 (y 始终 0，渲染层做轻微 bob) */
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
  /** 转向速度（弧度/秒） */
  turnSpeed = 3.0;

  // 复用一份预计算的栅格。布局驱动：外墙整段实心，内墙按落地门洞断开。
  // nav:false 的家具（窗、地毯、门扇这类不该挡路的东西）建栅格前先剔掉。
  private static grid = (() => {
    const layout = layoutData as any;
    return buildGrid(buildObstacles(layout), layout.room.bounds);
  })();

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
          this.state = 'stay';
          this.stayTimer = 2 + Math.random() * 4; // 2-6 秒
          if (this.currentHotspotId) {
            const h = findHotspot(this.currentHotspotId);
            if (h) this.facing = h.facing;
          }
          return;
        }
        const target = this.path[this.pathIdx];
        const dx = target.x - this.pos.x;
        const dz = target.z - this.pos.z;
        const dist = Math.hypot(dx, dz);
        // 先转向目标
        const desiredFacing = Math.atan2(dx, dz);
        const turnDiff = shortestAngleDiff(this.facing, desiredFacing);
        const turnAmt = Math.sign(turnDiff) * Math.min(Math.abs(turnDiff), this.turnSpeed * dt);
        this.facing += turnAmt;
        // 走完一段再切换
        if (dist < 0.05) {
          this.pathIdx++;
          return;
        }
        const step = Math.min(dist, this.walkSpeed * dt);
        this.pos.x += (dx / dist) * step;
        this.pos.z += (dz / dist) * step;
        break;
      }
      case 'stay':
        this.stayTimer -= dt;
        if (this.stayTimer <= 0) {
          this.state = 'idle';
          this.idleTimer = 0;
        }
        break;
    }
  }

  private pickNewDestination() {
    const candidates = HOTSPOTS.filter(h => h.id !== this.currentHotspotId);
    // 随机选一个
    const next = candidates[Math.floor(Math.random() * candidates.length)];
    const path = findPath(
      { x: this.pos.x, z: this.pos.z },
      { x: next.pos[0], z: next.pos[2] },
      PetAgent.grid
    );
    if (!path || path.length === 0) {
      // 不可达：跳过，再选
      this.idleTimer = 0;
      return;
    }
    this.path = path;
    this.pathIdx = 0;
    this.state = 'walk';
    this.currentHotspotId = next.id;
    this.idleTimer = 0;
  }

  /** 调试用：手动指定去某个热点。 */
  goTo(hotspotId: string) {
    const h = findHotspot(hotspotId);
    if (!h) return;
    const path = findPath(
      { x: this.pos.x, z: this.pos.z },
      { x: h.pos[0], z: h.pos[2] },
      PetAgent.grid
    );
    if (!path || path.length === 0) return;
    this.path = path;
    this.pathIdx = 0;
    this.state = 'walk';
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
