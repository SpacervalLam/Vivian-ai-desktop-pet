/**
 * 第一人称控制器 —— PointerLockControls + WASD 移动
 *
 * 操作：
 *   WASD          移动
 *   鼠标          视角
 *   Ctrl          跑步
 *   右键按住       跑步（与 Ctrl 等价，右手操作更方便）
 *   双击右键按住   高速冲刺
 *   Shift         蹲下（视高降低 + 减速）
 *   Space         跳跃（带重力）
 *
 * 移动模型对齐 Source/Quake 的手感机制（业界公认最"跟手"的 FPS 移动），
 * 但速度/尺度换算成米制，适配本项目的真实米制房间：
 *
 *   - 加速：向"期望速度 = 归一化方向 × 上限"平滑逼近（1-exp），
 *     不存在硬 clamp——斜向移动天然不超速，180° 掉头有自然急停感
 *   - 停止：松开按键后走 Quake 式地面摩擦，速度低于 stopspeed 时
 *     进入强停区快速归零，避免"滑冰/漂移"（这是手感"干脆"的关键）
 *   - 空中：无摩擦保持惯性，但仍可小幅调整方向（弱 air control）
 *   - head bob：步频随速度联动，相位不累积进相机逻辑高度
 *
 * 用法：
 *   const fps = new FPSControls(camera, domElement);
 *   fps.init();  // 绑定事件
 *   fps.requestLock();  // 进入第一人称（由调用方在 Enter 等用户手势里触发）
 *   // 每帧调用 fps.update(delta, collisionBoxes)
 *   fps.dispose();
 */

import * as THREE from 'three';

/**
 * 双击右键的判定窗口（ms）：两次按下的间隔在此之内才算"双击"。
 * 比系统双击阈值（500ms）紧一档——游戏里要的是干脆的两下，不是"按住前的抖动"。
 */
const RIGHT_DOUBLE_TAP_MS = 300;

/* ============================================================================
 * 贴墙透视 —— 两层防护：① near 自适应收小  ② 眼位外推（eye standoff）
 * ============================================================================
 *
 * 症状：第一人称贴着墙走，某些角度下墙被"切开"，看到墙体内部/背后的东西。
 *
 * ── 为什么会切 ──────────────────────────────────────────────────────────
 *
 * 近裁面不是一条线，而是一块**距离相机 near、有面积的矩形**
 * （半宽 w = near·tan(hfov/2)，半高 h = near·tan(vfov/2)）。一块离相机 D、
 * 法线 n 的平面会被切开，当且仅当近裁面有任何一个角越过了它：
 *
 *     D < near·|f·n| + w·|r·n| + h·|u·n|
 *
 * 右边是 (near, w, h) 在 (f, r, u) 基下的**支撑函数**，其最大值就是模长：
 *
 *     D_req = near · √(1 + tan²(hfov/2) + tan²(vfov/2))
 *
 * 竖直墙（n 水平）吃不到 u 项、地板/吊顶（n 竖直）吃不到 r 项，各自只到
 * √(1+tan²H) 与 √(1+tan²V)；但**斜坡、倒角、斜吊顶**三项都吃得到。取完整模长
 * 同时覆盖前两者的最坏情况，代价只是 near 再小约 7% —— 值得，因为漏掉它的
 * 后果正是"偶尔从楼梯斜面上看穿出去"。
 *
 * 相机贴墙能贴多近？碰撞是「半径 playerRadius 的圆 vs AABB」，圆心（= 相机）
 * 最少离碰撞盒 playerRadius；再算上 head bob 的横向摆动（±BOB_LATERAL，且 bob
 * 是在碰撞解算**之后**加上的，不受碰撞约束），实际最小距离是
 *
 *     CLEAR_EFF = EYE_STANDOFF − BOB_LATERAL
 *
 * ── 第①层：near 自适应 ────────────────────────────────────────────────
 *
 *     near = clamp((clearance − MARGIN) / k, NEAR_MIN, NEAR_MAX)
 *
 * clearance 是相机到最近「眼高阻挡盒」的距离，k 就是上面那个模长。两边同乘 k：
 * **D_req = clearance − MARGIN**，含义直白——**相机离任何可见面的距离恒 ≥
 * MARGIN**，MARGIN 直接就是「多厚的饰面/线脚还能保证不被切」。
 *
 * 但 MARGIN 有个**硬上限**，因为相机总能站到离碰撞盒 CLEAR_EFF 处：
 *
 *     MARGIN ≤ CLEAR_EFF − NEAR_MIN·k
 *
 * 可见面一旦比它的碰撞盒外凸 ≥ CLEAR_EFF，相机就**站在那个面里面**了 —— 这时
 * 无论 near 多小都救不了（D 可以是 0）。本项目碰撞半径只有 0.15（Q 版 1.45m
 * 角色 + 0.85m 门洞，半径不能再大），于是这个上限只有 ~0.083，而实测最大的
 * 外凸是 **0.132 m**（浴室内墙包管、走道墙饰面、便利店门头细节、裙房石材墙面…）。
 *
 * ── 第②层：眼位外推 ──────────────────────────────────────────────────
 *
 * 所以再加一层：**把相机（眼位）从最近的阻挡盒再推开 EYE_PUSH**，使
 *
 *     EYE_STANDOFF = playerRadius + EYE_PUSH = 0.23 m
 *
 * 这层**只动相机、不动身体**：碰撞半径不变 ⇒ 移动手感、门洞通过性、nav 连通性
 * 全部不变（玩家该能去的地方照样能去，只是眼位多留 8cm）。于是 CLEAR_EFF 从
 * 0.133 抬到 0.213，MARGIN 的上限跟着抬到 0.213 − 0.025·1.66 ≈ 0.172，
 * 取 **MARGIN = 0.15**（覆盖实测最大外凸 0.132，留 13% 余量）。
 *
 * 外推为什么必须有上限（EYE_PUSH）：推出去的方向可能正对着另一个盒子，推太多
 * 反而把相机送进对面那堵墙。上限 0.08 ⇒ 相机离「对面那个盒」仍 ≥ 0.15。
 * 外推也**只做水平方向** —— 竖直方向动相机会破坏「视高 = 支撑面 + 眼高」这条
 * 不变量（蹲下/上楼/落地全靠它）。
 *
 * 不贴墙时 near 自动回到 NEAR_MAX，深度精度无损。注意 near 只影响裁剪与深度
 * 映射，**不影响画面构图**（同样 fov 下远近物体的屏幕位置与 near 无关），
 * 所以每帧调 near 是视觉无感的。
 *
 * ── 第②层解决不了的：外凸 ≥ CLEAR_EFF 的面 ────────────────────────────
 *
 * 相机能走到可见面**里面**的，near 与外推都无解，只能补碰撞盒。实测这类点
 * （h ≥ 0.15）集中在室外：裙房立面装饰、楼体外墙构件、街道地块/停车场景物、
 * 居酒屋门头、自行车，以及室内几处摆件。见 tmp/_nc-attr.mjs 的归因表。
 */
/** 角色碰撞半径（米）。门洞净宽 0.85m ⇒ 直径 0.30m 过门绰绰有余 */
const PLAYER_RADIUS = 0.15;
/** 安全网：万一 CLEAR_EFF 被将来的改动压到 MARGIN + NEAR_MIN·k 以下，near 也不会失控 */
export const FPS_NEAR_MIN = 0.025;
/** 不贴墙时用的 near（米）——与观察者模式一致，保持原有的深度精度 */
export const FPS_NEAR_MAX = 0.1;
/** head bob 的横向摆幅上限（米）。bob 在碰撞解算之后叠加，所以它会再吃掉这么多净距 */
const BOB_LATERAL_MAX = 0.017;
/** 眼位外推量（米）：相机比身体多留出的水平净距。上限见文件头说明 */
const EYE_PUSH = 0.08;
/** 眼位到「眼高阻挡盒」的保证距离（米）= 身体净距 + 外推 */
const EYE_STANDOFF = PLAYER_RADIUS + EYE_PUSH;
/** 贴墙时相机到阻挡盒的真实最小距离（外推之后再扣掉 bob） */
const CLEAR_EFF = EYE_STANDOFF - BOB_LATERAL_MAX;
/**
 * 相机与「可见面」之间恒定保证的距离（米）。near 按「相机到碰撞盒的距离减去它」
 * 来算，所以这个值直接就是「多厚的饰面/线脚还能保证不被切」。
 * 上限 = CLEAR_EFF − FPS_NEAR_MIN·k_max ≈ 0.172（k_max 取水平视野封顶 100° 的
 * 最坏宽高比），这里取 0.15 留 13% 余量。
 */
const NEAR_SURFACE_MARGIN = 0.15;
/** 外推回落速度（1/s）：推出去要立刻，收回来只要不抖就行 */
const EYE_PUSH_RELEASE = 10;
/**
 * 近裁面竖直方向最多能伸到眼睛上下多远（米）：near·k ≤ FPS_NEAR_MAX·k_max ≈ 0.166。
 * clearanceAt 用它筛掉"离眼位太远、根本切不到"的盒子（齐腰矮柜、地毯、吊顶）。
 */
const NEAR_EYE_REACH = 0.25;

/** 碰撞盒接口 */
export interface Collider {
  min: THREE.Vector3;
  max: THREE.Vector3;
  /** 可选：来源标识，调试用 */
  source?: string;
  /** 碰撞盒类型：
   *  - wall：阻挡水平移动（墙/栏/柱/家具，高盒）
   *  - floor：可站立薄板（楼板/平台/地面/路缘），不参与水平阻挡，只作为落地支撑
   *  - ramp：斜坡（外楼梯），用 heightAt 取 (x,z) 处表面高度，不阻挡水平移动 */
  kind?: 'wall' | 'floor' | 'ramp';
  /** 斜坡表面高度函数：返回 (x,z) 处可站立高度，或 null（不在斜坡范围内） */
  heightAt?: (x: number, z: number) => number | null;
}

export class FPSControls {
  private camera: THREE.PerspectiveCamera;
  private dom: HTMLElement;

  // ---- 方向状态 ----
  private moveForward = false;
  private moveBackward = false;
  private moveLeft = false;
  private moveRight = false;
  private sprint = false;
  private jump = false;
  private crouch = false;
  private mouseRightDown = false; // 鼠标右键按住 = 加速
  private boost = false;          // 双击右键并按住 = 高速冲刺挡
  /** 上一次右键按下的时刻：只用来判定"这两下算不算双击" */
  private lastRightDownAt = -Infinity;

  // ---- 欧拉角 ----
  private yaw = 0;    // 绕 Y（水平旋转）
  private pitch = 0;  // 绕 X（上下看）

  // ---- 速度配置（米制，适配室内探索尺度）----
  public walkSpeed = 2.0;      // m/s 正常行走
  public sprintSpeed = 3.1;   // m/s 跑步
  public boostSpeed = 6.0;    // m/s 高速冲刺（双击右键并按住）
  public crouchSpeed = 0.85;   // m/s 蹲走
  public mouseSensitivity = 0.0015;  // rad/px

  // ---- 移动手感（Source/Quake 模型）----
  private groundAccel = 9;   // 加速响应系数（越大起步越脆；1-exp(-k·dt)）
  private groundFriction = 10; // 地面摩擦（越大停得越急）
  private stopSpeed = 0.8;    // 低于此速度进入强停区，快速归零

  // ---- 垂直运动（跳跃/重力）----
  private vy = 0;             // 垂直速度
  private gravity = -20;      // 重力加速度 m/s²（Quake 的"重"感，下落果断）
  private jumpForce = 6.0;    // 跳跃初速度 m/s（跳高 ≈ v²/2g = 0.9m）
  public isGrounded = true;   // 是否在地面上（外部可读）
  /**
   * 地板的世界 Y。落地判定和视高锁定都以它为基准——房间作为公寓楼的
   * 一个单元被整体抬高时（203 室在二楼），由调用方设成单元地板高度。
   */
  public floorY = 0;

  /** 调试遥测：最近一帧的玩家眼高 / 脚下支撑面 Y / 是否着地（HUD 读取，定位掉落用） */
  public dbgPlayerY = 0;
  public dbgGroundY = 0;
  public dbgGrounded = true;

  /** 可跨上的矮台高度上限（台阶/门槛/路缘等很低碰撞盒）：
   *  - 顶面不高于「脚底 + STEP_UP」且自身高度 ≤ STEP_UP 的矮盒，水平不阻挡、可直接走上去；
   *  - supportY 同时把这类矮盒顶面当落脚面，平滑抬升视高。
   * 高于此值的墙/栏/家具仍正常阻挡，需跳跃才能上。 */
  private readonly STEP_UP = 0.45;

  // ---- 视高 ----
  private standHeight = 1.6;  // 站立视高 m
  private crouchHeight = 0.8; // 蹲下视高 m
  private targetEyeY = 1.6;   // 目标视高（平滑过渡用）
  private currentEyeY = 1.6;  // 当前实际视高

  // Visual gait is removed before physics and reapplied after collision resolution.
  private bobPhase = 0;
  private gaitOffset = new THREE.Vector3();
  private gaitVertical = 0;
  private gaitLateral = 0;
  private gaitRoll = 0;
  private gaitPitch = 0;
  /**
   * 眼位外推偏移（世界 XZ，米）。与 gaitOffset 同源同寿命：每帧开头先撤掉，
   * 帧末按当前贴墙情况重算再叠上去。**身体位置不包含它** —— 所有碰撞/落脚/
   * 存档都只看撤掉之后的 camera.position。
   */
  private eyePush = new THREE.Vector2();
  private eyePushTarget = new THREE.Vector2();
  private clearGait(): void {
    this.camera.position.sub(this.gaitOffset);this.gaitOffset.set(0,0,0);
    this.camera.position.x-=this.eyePush.x;this.camera.position.z-=this.eyePush.y;this.eyePush.set(0,0);
    this.bobPhase=this.gaitVertical=this.gaitLateral=this.gaitRoll=this.gaitPitch=0;
    this.updateOrientation();
  }

  // ---- 内部状态 ----
  private isLocked = false;
  private lockTime = 0;       // 最近一次进入指针锁定的时间戳（跳过初始 spike）
  private velocity = new THREE.Vector3();

  // ---- 回调 ----
  onLock?: () => void;
  onUnlock?: () => void;

  constructor(camera: THREE.PerspectiveCamera, domElement: HTMLElement) {
    this.camera = camera;
    this.dom = domElement;
  }

  /** 绑定所有事件 */
  init(): void {
    document.addEventListener('keydown', this.onKeyDown);
    document.addEventListener('keyup', this.onKeyUp);
    document.addEventListener('mousemove', this.onMouseMove);
    document.addEventListener('mousedown', this.onMouseDown);
    document.addEventListener('mouseup', this.onMouseUp);
    document.addEventListener('contextmenu', this.onContextMenu);
    document.addEventListener('pointerlockchange', this.onLockChange);
    document.addEventListener('pointerlockerror', this.onLockError);
  }

  /** 清理事件 */
  dispose(): void {
    // 若在指针锁定状态下卸载（同 webview 重挂场景），主动退出锁定
    if (document.pointerLockElement) {
      try {
        document.exitPointerLock();
      } catch {
        /* ignore */
      }
    }
    document.removeEventListener('keydown', this.onKeyDown);
    document.removeEventListener('keyup', this.onKeyUp);
    document.removeEventListener('mousemove', this.onMouseMove);
    document.removeEventListener('mousedown', this.onMouseDown);
    document.removeEventListener('mouseup', this.onMouseUp);
    document.removeEventListener('contextmenu', this.onContextMenu);
    document.removeEventListener('pointerlockchange', this.onLockChange);
    document.removeEventListener('pointerlockerror', this.onLockError);
  }

  /** 是否处于第一人称锁定状态 */
  get locked(): boolean {
    return this.isLocked;
  }

  /** 请求指针锁定（必须由用户手势触发，否则浏览器拒绝并派发 pointerlockerror） */
  requestLock(): void {
    try {
      // unadjustedMovement: true 请求原始鼠标输入，去掉 OS 级鼠标加速——
      // 否则快速甩鼠标时 movementX 被系统放大，视角过冲、手感飘。
      // 不支持该参数的平台会同步抛错或返回 rejected Promise，两种情况都退回普通锁定。
      const el = this.dom as HTMLElement & {
        requestPointerLock: (opts?: { unadjustedMovement?: boolean }) => Promise<void> | void;
      };
      const result = el.requestPointerLock({ unadjustedMovement: true });
      if (result && typeof (result as Promise<void>).catch === 'function') {
        (result as Promise<void>).catch(() => {
          // 退回普通锁定。**这个 promise 也要 catch** —— 不支持指针锁定的环境
          // （headless、部分 webview）里它会 reject，不接就是一条 Uncaught
          // (in promise) 噪声，掩盖真正的报错。
          try {
            const fallback = this.dom.requestPointerLock() as unknown as Promise<void> | undefined;
            if (fallback && typeof fallback.catch === 'function') fallback.catch(() => { /* 由 onLockError 语义接管 */ });
          } catch { /* ignore */ }
        });
      }
    } catch {
      try { this.dom.requestPointerLock(); } catch { /* 浏览器不支持或已销毁 */ }
    }
  }

  /** 设置初始位置和朝向。y 是世界坐标（眼睛高度）；视高用绝对 standHeight 存，
   * 落地时再贴到"脚下支撑面 + 视高"，所以不依赖单一 floorY（支持多层/斜坡）。 */
  setPosition(x: number, y: number, z: number): void {
    this.clearGait();
    this.camera.position.set(x, y, z);
    this.currentEyeY = this.standHeight;
    this.targetEyeY = this.standHeight;
    // 落点/瞬移/恢复存档时清掉残留垂直速度并标记为着地，避免把上一帧的 vy 带进新位置
    // （否则从高处恢复存档会带着向下的速度继续坠，或 spawn 直接被拉穿当前楼板）。
    this.vy = 0;
    this.isGrounded = true;
  }

  setRotation(yaw: number, pitch: number): void {
    this.yaw = yaw;
    this.pitch = pitch;
    this.updateOrientation();
  }

  /** Clear held input and momentum before/after a lift transfer. */
  stopMotion(): void {
    this.clearGait();
    this.velocity.set(0,0,0);this.vy=0;
    this.moveForward=this.moveBackward=this.moveLeft=this.moveRight=false;
    this.sprint=this.jump=this.crouch=this.mouseRightDown=this.boost=false;
    this.lastRightDownAt=-Infinity;
    this.bobPhase=0;this.currentEyeY=this.targetEyeY=this.standHeight;
  }

  /** 当前世界坐标（眼睛位置）—— 不含 bob 与眼位外推，是**身体**所在处 */
  getPosition(): { x: number; y: number; z: number } {
    return { x: this.camera.position.x-this.gaitOffset.x-this.eyePush.x, y: this.camera.position.y-this.gaitOffset.y, z: this.camera.position.z-this.gaitOffset.z-this.eyePush.y };
  }

  /** 当前朝向（弧度） */
  getRotation(): { yaw: number; pitch: number } {
    return { yaw: this.yaw, pitch: this.pitch };
  }

  /**
   * 当前速度挡位，供 HUD 显示。
   *
   * 挡位与 update 里的 maxSpeed 分支一一对应——放在一处判定，避免 HUD 显示的挡位
   * 和实际用的速度各算各的、慢慢漂开。
   */
  getSpeedTier(): 'crouch' | 'boost' | 'sprint' | 'walk' {
    if (this.crouch) return 'crouch';
    if (this.boost) return 'boost';
    if (this.sprint || this.mouseRightDown) return 'sprint';
    return 'walk';
  }

  /**
   * 每帧调用。返回是否发生了移动。
   * @param delta 帧间隔秒数
   * @param colliders 碰撞 AABB 列表（可选）
   */
  update(delta: number, colliders?: Collider[]): boolean {
    if (!this.isLocked) return false;

    this.camera.position.sub(this.gaitOffset);this.gaitOffset.set(0,0,0);
    // 撤掉上一帧的眼位外推：下面所有碰撞/落脚/视高逻辑都在**身体**位置上做，
    // 眼位外推只是渲染用的相机偏移（见文件头「第②层」）。
    this.camera.position.x-=this.eyePush.x;this.camera.position.z-=this.eyePush.y;
    const startX=this.camera.position.x,startZ=this.camera.position.z;
    const playerRadius = PLAYER_RADIUS;

    // 当前脚下支撑面（动态地面：地面/各层平台/楼板/斜坡），用于贴地与视高复位
    const supportNow = this.supportY(this.camera.position.x, this.camera.position.z, playerRadius, this.camera.position.y - this.currentEyeY, colliders);
    const groundNow = supportNow != null ? Math.max(supportNow, this.floorY) : this.floorY;

    // 地面时先复位 y，消除上一帧 head bob 的偏移（bob 不累积进逻辑高度）
    if (this.isGrounded) {
      this.camera.position.y = groundNow + this.currentEyeY;
    }

    // ---- 1. 根据状态选速度上限与视高 ----
    // 三挡：走 / 跑（Ctrl 或单击右键按住）/ 冲刺（双击右键并按住）。
    // 蹲下优先级最高——蹲着不该有高速挡。
    const sprinting = this.sprint || this.mouseRightDown;
    let maxSpeed: number;
    if (this.crouch) {
      maxSpeed = this.crouchSpeed;
      this.targetEyeY = this.crouchHeight;
    } else if (this.boost) {
      maxSpeed = this.boostSpeed;
      this.targetEyeY = this.standHeight;
    } else if (sprinting) {
      maxSpeed = this.sprintSpeed;
      this.targetEyeY = this.standHeight;
    } else {
      maxSpeed = this.walkSpeed;
      this.targetEyeY = this.standHeight;
    }

    // ---- 2. 期望方向（相机相对，XZ 平面）。用标量算，避免每帧 new Vector3 造成 GC 压力 ----
    const fx = -Math.sin(this.yaw);
    const fz = -Math.cos(this.yaw);
    const rx = Math.cos(this.yaw);
    const rz = -Math.sin(this.yaw);

    let wishX = 0;
    let wishZ = 0;
    if (this.moveForward) { wishX += fx; wishZ += fz; }
    if (this.moveBackward) { wishX -= fx; wishZ -= fz; }
    if (this.moveRight) { wishX += rx; wishZ += rz; }
    if (this.moveLeft) { wishX -= rx; wishZ -= rz; }

    const hasInput = wishX !== 0 || wishZ !== 0;
    const horizSpeed = Math.hypot(this.velocity.x, this.velocity.z);

    if (hasInput) {
      // ---- 加速：向期望速度平滑逼近（无硬 clamp，斜向天然不超速）----
      const inv = 1 / Math.hypot(wishX, wishZ);
      const targetX = wishX * inv * maxSpeed;
      const targetZ = wishZ * inv * maxSpeed;
      const t = 1 - Math.exp(-this.groundAccel * delta);
      this.velocity.x += (targetX - this.velocity.x) * t;
      this.velocity.z += (targetZ - this.velocity.z) * t;
    } else if (this.isGrounded) {
      // ---- 停止：Quake 式地面摩擦 + 强停区 ----
      if (horizSpeed < 0.01) {
        this.velocity.x = 0;
        this.velocity.z = 0;
      } else {
        const control = Math.max(horizSpeed, this.stopSpeed);
        const drop = control * (1-Math.exp(-this.groundFriction * delta));
        const newSpeed = Math.max(0, horizSpeed - drop);
        const scale = newSpeed / horizSpeed;
        this.velocity.x *= scale;
        this.velocity.z *= scale;
      }
    }
    // 空中且无输入：无摩擦，保持惯性（不在这里减速）

    // ---- 3. 计算位移并应用水平碰撞 ----
    const dx = this.velocity.x * delta;
    const dz = this.velocity.z * delta;

    let moved = false;
    const travel = Math.hypot(dx, dz);
    if (travel > 1e-6) {
      // 分子步推进。单点检测只保证"落点不在墙里"，高速时必须保证"路径上也没有墙"：
      // 墙最薄 12cm、玩家半径 15cm，一帧跨过 0.42m 就能从墙这头跳到那头——冲刺 6m/s
      // 在掉到 15fps 以下时单帧就到。按 ≤12cm 切段后，正常帧率下仍是 1 段（无额外开销），
      // 掉帧时也不会穿墙；分轴滑动的行为完全不变。
      const steps = Math.min(16, Math.max(1, Math.ceil(travel / 0.12)));
      const sx = dx / steps;
      const sz = dz / steps;
      for (let i = 0; i < steps; i++) {
        // 分轴检测（允许贴墙滑动）
        const newX = this.camera.position.x + sx;
        if (!this.collidesAt(newX, this.camera.position.z, playerRadius, colliders)) {
          this.camera.position.x = newX;
          moved = true;
        }
        const newZ = this.camera.position.z + sz;
        if (!this.collidesAt(this.camera.position.x, newZ, playerRadius, colliders)) {
          this.camera.position.z = newZ;
          moved = true;
        }
      }
    }

    const distanceMoved = Math.hypot(this.camera.position.x-startX,this.camera.position.z-startZ);

    // ---- 4. 垂直运动（跳跃 + 重力）----
    if (this.jump && this.isGrounded && !this.crouch) {
      this.vy = this.jumpForce;
      this.isGrounded = false;
    }

    this.vy += this.gravity * delta;
    let newY = this.camera.position.y + this.vy * delta;

    // 根因修复（"第一次进第一人称从二楼掉到一楼"）：贴地时单帧下落量封顶。
    // 刚锁定指针 / RAF 被节流后首帧的 elapsed 会被夹到 0.25s，单帧重力把脚在一帧内
    // 拉出当前楼板（二楼）的 STEP_UP 吸附窗口（3.42→2.97），supportY 于是翻到下一层
    // 楼板（一楼地面 0.02），玩家就此自由落体穿楼。封顶后脚始终留在当前楼板的可吸附
    // 范围内，落地判定会把人稳稳接回原楼层，且走下小台阶/走出楼板边缘的判定不受影响。
    if (this.isGrounded) {
      const footNow = this.camera.position.y - this.currentEyeY;
      const floorLimit = footNow - this.STEP_UP * 0.9;
      if (newY - this.currentEyeY < floorLimit) {
        newY = floorLimit + this.currentEyeY;
        this.vy = 0;
      }
    }

    // 落地判定用动态支撑面（地面/平台/楼板/斜坡），不再是单一 floorY——
    // 这样能走外楼梯上到 2F/3F/4F，也能在整张地图自由行动
    const supportNew = this.supportY(this.camera.position.x, this.camera.position.z, playerRadius, newY - this.currentEyeY, colliders);
    const groundNew = supportNew != null ? Math.max(supportNew, this.floorY) : this.floorY;
    const footY = newY - this.currentEyeY;
    if (footY <= groundNew && this.vy <= 0) {
      // 落地/贴地：把相机 y 锁在 支撑面 + 视高处
      this.camera.position.y = groundNew + this.currentEyeY;
      this.vy = 0;
      this.isGrounded = true;
    } else {
      // 腾空（上升中、或脚下已无支撑面，如走下边缘/从高处坠落）：标记离地。
      // 关键：否则 isGrounded 一直为 true，下一帧开头「grounded 时贴地」逻辑会用
      // 兜底地面（floorY）把相机瞬移到街道高度——这正是"高处坠落竖直瞬移"的根因。
      this.isGrounded = false;
      if (!this.collidesAt3D(this.camera.position.x, newY, this.camera.position.z, playerRadius, colliders)) {
        this.camera.position.y = newY;
        moved = true;
      } else if (this.vy > 0) {
        this.vy = 0; // 头顶撞到东西
      }
    }

    // ---- 5. 视高平滑过渡（蹲下/站起）----
    const eyeLerpSpeed = 12.0;
    if (Math.abs(this.currentEyeY - this.targetEyeY) > 0.001) {
      this.currentEyeY += (this.targetEyeY - this.currentEyeY) * Math.min(1.0, eyeLerpSpeed * delta);
      if (this.isGrounded) {
        this.camera.position.y = groundNew + this.currentEyeY;
      }
    }

    // Distance-driven footsteps: one vertical pulse per step, alternating lateral sway.
    const actualSpeed=distanceMoved/Math.max(delta,.0001);
    const runBlend=THREE.MathUtils.smoothstep(actualSpeed,this.walkSpeed,this.sprintSpeed);
    const active=this.isGrounded&&actualSpeed>.025;
    const strength=active?Math.min(1,actualSpeed/this.walkSpeed)*(this.crouch?.38:1):0;
    // 走路步长按「步频 ≈1.9 步/秒」从 walkSpeed 反推，不写死数值：
    // 写死的话 walkSpeed 一改、步频就跟着漂（2.0 m/s 配固定的 0.76m 会走出
    // 2.6 步/秒的碎步，比跑步还密），调速度的人还得回头手动配步长。
    const walkStep=this.walkSpeed/1.9;
    const stepLength=THREE.MathUtils.lerp(walkStep,1.20,runBlend)*(this.crouch?.8:1);
    if(active)this.bobPhase=(this.bobPhase+distanceMoved/stepLength*Math.PI*2)%(Math.PI*4);
    const blend=1-Math.exp(-12*delta);
    const vertical=Math.sin(this.bobPhase)*THREE.MathUtils.lerp(.018,.033,runBlend)*strength;
    const lateral=Math.sin(this.bobPhase*.5)*THREE.MathUtils.lerp(.009,BOB_LATERAL_MAX,runBlend)*strength;
    this.gaitVertical+=(vertical-this.gaitVertical)*blend;
    this.gaitLateral+=(lateral-this.gaitLateral)*blend;
    this.gaitRoll+=(Math.sin(this.bobPhase*.5)*.0025*strength-this.gaitRoll)*blend;
    this.gaitPitch+=(Math.cos(this.bobPhase)*.0018*strength-this.gaitPitch)*blend;
    this.gaitOffset.set(Math.cos(this.yaw)*this.gaitLateral,this.gaitVertical,-Math.sin(this.yaw)*this.gaitLateral);
    this.camera.position.add(this.gaitOffset);
    this.updateOrientation();

    /* 第②层：眼位外推。放在 bob 之后 —— 此时相机位置就是玩家真正看到的位置，
     * 推的是"眼睛"，不是"身体"。 */
    this.applyEyeStandoff(colliders, delta);

    /* 第①层：贴墙防透视。near 随「相机到最近阻挡盒的距离」收放（推导见文件头）。
     * 放在最后算 —— 此时相机位置已经含 bob 与外推，量到的就是真实间距。 */
    this.applyNearPlane(this.clearanceAt(colliders), delta);

    // 调试遥测：最近一帧的玩家眼高 / 脚下支撑面 / 是否着地（供 HUD 读出来定位掉落）
    this.dbgPlayerY = this.camera.position.y;
    this.dbgGroundY = groundNew;
    this.dbgGrounded = this.isGrounded;

    return moved;
  }

  // ---- 碰撞检测 ----

  /** 2D 圆-AABB 碰撞（水平移动用） */
  private collidesAt(
    x: number,
    z: number,
    radius: number,
    colliders?: Collider[]
  ): boolean {
    if (!colliders || colliders.length === 0) return false;

    /**
     * 只跟「玩家身体所在高度区间」有交集的碰撞盒才算挡路。
     * 之前这里完全不看 Y，于是任何挂高的东西（吊灯、壁挂搁板、上柜）都会在
     * 地面高度凭空挡人——看不见却过不去，正是所谓的空气墙。
     * 脚底 = 相机 y − 当前视高；头顶再留 10cm 余量。
     */
    const footY = this.camera.position.y - this.currentEyeY;
    const headY = this.camera.position.y + 0.1;

    for (const c of colliders) {
      if (c.kind === 'ramp') continue;       // 斜坡可踩，不阻挡水平移动
      if (c.min.y > headY) continue;        // 整个盒子在头顶之上：钻得过去
      if (c.max.y < footY + 0.12) continue; // 整个盒子在脚踝以下：门槛/地垫，迈过去
      // 矮台（自身高度 ≤ STEP_UP 且顶面不高于脚底 + STEP_UP）：当作可跨上的台阶，
      // 水平不阻挡——很低碰撞盒能直接走上去，无需跳跃（supportY 会同步抬升视高）
      const boxH = c.max.y - c.min.y;
      if (boxH <= this.STEP_UP && c.max.y <= footY + this.STEP_UP) continue;
      const closestX = Math.max(c.min.x, Math.min(x, c.max.x));
      const closestZ = Math.max(c.min.z, Math.min(z, c.max.z));
      const distX = x - closestX;
      const distZ = z - closestZ;
      if (distX * distX + distZ * distZ < radius * radius) {
        return true;
      }
    }
    return false;
  }

  /**
   * 相机到最近「**眼高**可见盒」的水平距离（米）。一个都没有就是 Infinity。
   *
   * 谓词与 collidesAt **故意不同**，因为两者关心的事不一样：
   *   - collidesAt 管走路：齐腰的矮柜也挡人，所以它按「脚底~头顶」整段算；
   *   - 这里管近裁面：近裁面在竖直方向最多只伸到眼睛上下 near·k ≤ 0.166 m，
   *     离眼位 0.7 m 的桌面根本够不到。若沿用走路那套谓词，站在任何家具旁边
   *     near 都会被压到下限、白白丢掉深度精度（实测贴着洗面台 near 就掉到 0.025，
   *     而那个台面比眼睛低 0.75 m）。
   *
   * 所以这里只认**与眼位上下 0.25 m 内有交集**的盒子（0.25 > 0.166 留了余量）。
   * 墙/柱/衣柜/包管都还在，矮柜/桌面/地毯被排除。
   */
  private clearanceAt(colliders?: Collider[]): number {
    if (!colliders || colliders.length === 0) return Infinity;
    const eyeY = this.camera.position.y;
    const x = this.camera.position.x;
    const z = this.camera.position.z;
    let best = Infinity;
    for (const c of colliders) {
      if (c.kind === 'ramp') continue;
      if (c.max.y < eyeY - NEAR_EYE_REACH) continue; // 整体在眼睛下方：近裁面够不到
      if (c.min.y > eyeY + NEAR_EYE_REACH) continue; // 整体在眼睛上方：同上
      const dx = x - Math.max(c.min.x, Math.min(x, c.max.x));
      const dz = z - Math.max(c.min.z, Math.min(z, c.max.z));
      const d = Math.sqrt(dx * dx + dz * dz);
      if (d < best) best = d;
    }
    return best;
  }

  /**
   * 眼位外推：把**相机**从最近的阻挡盒推开，直到离它 ≥ EYE_STANDOFF。
   *
   * 为什么需要（见文件头）：碰撞只保证**身体**离碰撞盒 playerRadius，而可见面
   * 常常比碰撞盒往外凸 5~13cm（饰面/线脚/包管/门套/立面装饰）。光收 near 救不了
   * 外凸 ≥ CLEAR_EFF 的面，因为相机那时已经站在面里了。把眼位多推开 EYE_PUSH
   * 就把可容忍的外凸从 0.065 抬到 0.15。
   *
   * 三条约束：
   *   - **只推相机、不推身体**：碰撞半径不变 ⇒ 手感 / 门洞 / nav 连通性零影响。
   *   - **只推水平方向**：竖直方向动相机会破坏「视高 = 支撑面 + 眼高」。
   *   - **推量有上限**：推的方向可能正对着另一个盒子，推太多等于把相机送进对面
   *     那堵墙。上限 EYE_PUSH ⇒ 相机离"对面那个盒"仍 ≥ playerRadius。
   *
   * 多个盒子按"各推各的再求和"处理（不是只取最近那个）：贴着墙角时两个方向各推
   * 一点、自然落到角平分线上，不会因为"最近的是哪一个"在两堵墙之间来回跳。
   *
   * 推出去立刻生效、收回来平滑：慢一帧就漏一帧的透视，收快收慢无所谓。
   */
  private applyEyeStandoff(colliders: Collider[] | undefined, delta: number): void {
    const want = this.eyePushTarget;
    want.set(0, 0);
    if (colliders && colliders.length) {
      const x = this.camera.position.x, y = this.camera.position.y, z = this.camera.position.z;
      const R2 = EYE_STANDOFF * EYE_STANDOFF;
      let px = 0, pz = 0;
      for (const c of colliders) {
        if (c.kind === 'ramp') continue; // 斜坡可踩，不挡人
        const cx = Math.max(c.min.x, Math.min(x, c.max.x));
        const cy = Math.max(c.min.y, Math.min(y, c.max.y));
        const cz = Math.max(c.min.z, Math.min(z, c.max.z));
        const dx = x - cx, dy = y - cy, dz = z - cz;
        const d2 = dx * dx + dy * dy + dz * dz;
        if (d2 >= R2) continue;
        const dh = Math.hypot(dx, dz);
        // 眼睛正在盒子的竖直投影内（站在矮柜/桌面上）：没有可用的水平方向，
        // 而且盒子本来就在脚下/头顶，不该推。
        if (dh < 1e-4) continue;
        const push = EYE_STANDOFF - Math.sqrt(d2);
        px += (dx / dh) * push;
        pz += (dz / dh) * push;
      }
      const mag = Math.hypot(px, pz);
      if (mag > EYE_PUSH) { const s = EYE_PUSH / mag; px *= s; pz *= s; }
      want.set(px, pz);
    }
    const t = want.length() >= this.eyePush.length() ? 1 : Math.min(1, EYE_PUSH_RELEASE * delta);
    this.eyePush.lerp(want, t);
    this.camera.position.x += this.eyePush.x;
    this.camera.position.z += this.eyePush.y;
  }

  /**
   * 按「相机到最近墙面的距离」收放近裁面。推导见文件头第①层那一节。
   *
   *   收：立刻生效 —— 慢一帧就漏一帧的透视。
   *   放：按 8/s 平滑长回去 —— 贴着墙来回蹭时 near 才不会一跳一跳。
   *
   * 调用点必须在「bob 偏移与眼位外推都已经加到相机位置之后」：这样量到的间距
   * 才是相机真实所在的间距，不必再手工扣 bob 幅度。
   */
  private applyNearPlane(clearance: number, delta: number): void {
    const cam = this.camera;
    const tanV = Math.tan(THREE.MathUtils.degToRad(cam.fov) * 0.5);
    const tanH = tanV * cam.aspect;
    /* 支撑函数上限 √(1+tan²H+tan²V)：竖直墙只吃前两项、地板/吊顶只吃 1+V 项，
     * 但斜坡/倒角三项都吃 —— 取完整模长同时覆盖两者（多花 ~7% 的 near）。 */
    const k = Math.sqrt(1 + tanH * tanH + tanV * tanV);
    const want = THREE.MathUtils.clamp(
      (clearance - NEAR_SURFACE_MARGIN) / k,
      FPS_NEAR_MIN,
      FPS_NEAR_MAX
    );
    const next = want < cam.near ? want : cam.near + (want - cam.near) * Math.min(1, 8 * delta);
    if (Math.abs(next - cam.near) < 1e-4) return;
    cam.near = next;
    cam.updateProjectionMatrix();
  }

  /** 3D AABB 碰撞（垂直移动 / 跳跃用）*/
  private collidesAt3D(
    x: number,
    y: number,
    z: number,
    radius: number,
    colliders?: Collider[]
  ): boolean {
    if (!colliders || colliders.length === 0) return false;

    for (const c of colliders) {
      if (c.kind === 'ramp') continue;       // 斜坡可踩，不阻挡垂直（否则高 AABB 会把上行/下行卡死）
      // 把玩家近似为圆柱体：xy 平面圆形 + z 轴范围
      const closestX = Math.max(c.min.x, Math.min(x, c.max.x));
      const closestY = Math.max(c.min.y, Math.min(y, c.max.y));
      const closestZ = Math.max(c.min.z, Math.min(z, c.max.z));
      const dx = x - closestX;
      const dy = y - closestY;
      const dz = z - closestZ;
      // 水平用圆距离，垂直用绝对距离
      if (dx * dx + dz * dz < radius * radius && Math.abs(dy) < 0.1) {
        return true;
      }
    }
    return false;
  }

  /**
   * 动态地面高度：返回 (x,z) 处玩家脚下「最高、且不超过 footNow + STEP 的落脚面」。
   *
   * 这样地面不再是单一 floorY：街道(0)、各层楼板/外廊/楼梯平台、以及外楼梯斜坡
   * 都能成为落点，第一人称就能在整张地图（含 2F/3F/4F）自由上下而不被单一地平线卡死。
   *
   *   - floor：薄板顶面作为落脚面（楼板/平台/地面/路缘）
   *   - ramp ：用 heightAt(x,z) 取斜坡表面高度（外楼梯），不阻挡水平/垂直
   *   - wall ：也取顶面作落脚面——矮墙（高度 ≤ STEP_UP）能直接踩上去，与 collidesAt
   *            的矮台跨上逻辑一致；高墙顶面远高于「脚底 + STEP_UP」，不会被误当地面
   *
   * STEP_UP 是「可登上/落下的单步高差上限」：小于它的台阶/矮墙会被平滑吸附，
   * 大于它的（如下一层楼板、远处地面、整面高墙）不会被瞬移吸附，只能靠走斜坡/跳下来到达。
   */
  private supportY(
    x: number,
    z: number,
    radius: number,
    footNow: number,
    colliders?: Collider[]
  ): number | null {
    if (!colliders || colliders.length === 0) return null;
    const STEP = this.STEP_UP;
    let best: number | null = null;
    for (const c of colliders) {
      if (c.kind === 'ramp') {
        const h = c.heightAt?.(x, z);
        if (h == null) continue;
        if (h <= footNow + STEP && (best == null || h > best)) best = h;
        continue;
      }
      // floor 与 wall 都取「顶面」作为可落脚面：矮墙也能踩上去，高墙顶面远高于脚底+STEP 故不会被误当。
      // 用水平距离判定 (x,z) 是否落在板/盒范围内（半径内才算踩在该物体上）
      const cx = Math.max(c.min.x, Math.min(x, c.max.x));
      const cz = Math.max(c.min.z, Math.min(z, c.max.z));
      const dx = x - cx;
      const dz = z - cz;
      if (dx * dx + dz * dz > radius * radius) continue;
      const top = c.max.y;
      if (top <= footNow + STEP && (best == null || top > best)) best = top;
    }
    return best;
  }

  // ---- 事件处理（箭头函数保持 this）----

  // 鼠标右键：按住 = 加速（等价于 Ctrl 跑步）；双击后按住 = 高速冲刺。
  //
  // 双击用"按下沿"计时，不用 dblclick/click 事件：这里要的是"第二下按住不放"，
  // 而 click 只在松开时才发，拿不到保持状态。间隔阈值取得比系统双击（500ms）紧一档
  // ——游戏里要的是干脆的两下，不是"按住前的抖动"。
  private onMouseDown = (e: MouseEvent): void => {
    if (e.button !== 2) return;
    if (!this.mouseRightDown) {
      const now = performance.now();
      if (now - this.lastRightDownAt < RIGHT_DOUBLE_TAP_MS) this.boost = true;
      this.lastRightDownAt = now;
    }
    this.mouseRightDown = true;
  };

  private onMouseUp = (e: MouseEvent): void => {
    if (e.button !== 2) return;
    this.mouseRightDown = false;
    this.boost = false; // 松开即掉挡：冲刺是"按住才保持"，不是开关
  };

  private onContextMenu = (e: MouseEvent): void => {
    e.preventDefault(); // 阻止右键菜单（尤其非锁定状态下）
  };

  private onKeyDown = (e: KeyboardEvent): void => {
    if (!this.isLocked) return;
    // 命令面板等输入框聚焦时，即使指针锁定尚未完全释放，这些键也该交给输入框打字，
    // 不能让空格把玩家弹起来 / Ctrl 变成跑步。
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
    // 忽略长按自动重复，保证跳跃/蹲下是边沿触发
    if (e.repeat) return;
    switch (e.code) {
      case 'KeyW': case 'ArrowUp':    this.moveForward = true; break;
      case 'KeyS': case 'ArrowDown':  this.moveBackward = true; break;
      case 'KeyA': case 'ArrowLeft':  this.moveLeft = true; break;
      case 'KeyD': case 'ArrowRight': this.moveRight = true; break;
      case 'ShiftLeft': case 'ShiftRight': this.crouch = true; break;
      case 'Space':
        e.preventDefault(); // 阻止页面滚动
        this.jump = true;
        break;
      case 'ControlLeft': case 'ControlRight':
        e.preventDefault();
        this.sprint = true;
        break;
    }
  };

  private onKeyUp = (e: KeyboardEvent): void => {
    switch (e.code) {
      case 'KeyW': case 'ArrowUp':    this.moveForward = false; break;
      case 'KeyS': case 'ArrowDown':  this.moveBackward = false; break;
      case 'KeyA': case 'ArrowLeft':  this.moveLeft = false; break;
      case 'KeyD': case 'ArrowRight': this.moveRight = false; break;
      case 'ShiftLeft': case 'ShiftRight': this.crouch = false; break;
      case 'Space':  this.jump = false; break;
      case 'ControlLeft': case 'ControlRight': this.sprint = false; break;
    }
  };

  private onMouseMove = (e: MouseEvent): void => {
    if (!this.isLocked) return;

    // 锁定后 120ms 内的 movement 丢弃：PointerLock 建立瞬间会有一帧
    // 巨大的 movementX（鼠标从实际位置"跳"到锁定点的虚拟偏移）。
    if (performance.now() - this.lockTime < 120) return;

    const movementX = e.movementX || 0;
    const movementY = e.movementY || 0;

    // spike 过滤：Windows Chromium 下高轮询率（500-1000Hz）鼠标会随机把
    // movementX 从个位数跳到几百（如 1 → 627），表现为视角猛地甩一大圈。
    // 单次位移超过上限视为 spike，整帧丢弃（正常甩鼠标一帧也就几十像素）。
    const MAX_MOVEMENT_DELTA = 200;
    if (Math.abs(movementX) > MAX_MOVEMENT_DELTA || Math.abs(movementY) > MAX_MOVEMENT_DELTA) {
      return;
    }

    this.yaw -= movementX * this.mouseSensitivity;
    this.pitch -= movementY * this.mouseSensitivity;

    // 限制俯仰角（不能翻转，±89°）
    const limit = Math.PI / 2 - 0.01;
    this.pitch = Math.max(-limit, Math.min(limit, this.pitch));

    this.updateOrientation();
  };

  private onLockChange = (): void => {
    this.isLocked = document.pointerLockElement === this.dom;
    if (this.isLocked) {
      this.lockTime = performance.now();
      this.onLock?.();
    } else {
      // 退出指针锁定时重置所有按键状态
      this.moveForward = false;
      this.moveBackward = false;
      this.moveLeft = false;
      this.moveRight = false;
      this.sprint = false;
      this.jump = false;
      this.crouch = false;
      this.mouseRightDown = false;
      this.stopMotion();
      this.onUnlock?.();
    }
  };

  private onLockError = (): void => {
    // 无用户手势时浏览器拒绝锁定：交给 onUnlock 语义（RoomScene 显示点击遮罩）
    this.isLocked = false;
    this.stopMotion();
    this.onUnlock?.();
  };

  /** 根据欧拉角更新相机四元数 */
  private updateOrientation(): void {
    this.camera.rotation.set(0, 0, 0, 'YXZ');
    this.camera.rotateY(this.yaw);
    this.camera.rotateX(this.pitch + this.gaitPitch);
    this.camera.rotateZ(this.gaitRoll);
  }
}
