/**
 * 第一人称控制器 —— PointerLockControls + WASD 移动
 *
 * 操作：
 *   WASD          移动
 *   鼠标          视角
 *   Shift         跑步
 *   Space         跳跃（带重力）
 *   Ctrl          蹲下（视高降低 + 减速）
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

  // ---- 欧拉角 ----
  private yaw = 0;    // 绕 Y（水平旋转）
  private pitch = 0;  // 绕 X（上下看）

  // ---- 速度配置（米制，适配室内探索尺度）----
  public walkSpeed = 2.2;     // m/s 正常行走
  public sprintSpeed = 4.5;   // m/s 跑步
  public crouchSpeed = 1.2;   // m/s 蹲走
  public mouseSensitivity = 0.002;  // rad/px

  // ---- 移动手感（Source/Quake 模型）----
  private groundAccel = 15;   // 加速响应系数（越大起步越脆；1-exp(-k·dt)）
  private groundFriction = 6; // 地面摩擦（越大停得越急）
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

  // ---- 头部晃动（Head Bob）----
  private bobPhase = 0;       // 晃动相位（随步频累积，不累积进相机 y）
  private bobAmount = 0.018;  // 走路晃动振幅 m
  private sprintBobAmount = 0.03; // 跑步振幅
  private crouchBobAmount = 0.008; // 蹲下振幅

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
          try { this.dom.requestPointerLock(); } catch { /* ignore */ }
        });
      }
    } catch {
      try { this.dom.requestPointerLock(); } catch { /* 浏览器不支持或已销毁 */ }
    }
  }

  /** 设置初始位置和朝向。y 是世界坐标（眼睛高度）；视高用绝对 standHeight 存，
   * 落地时再贴到"脚下支撑面 + 视高"，所以不依赖单一 floorY（支持多层/斜坡）。 */
  setPosition(x: number, y: number, z: number): void {
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

  /** 当前世界坐标（眼睛位置） */
  getPosition(): { x: number; y: number; z: number } {
    return { x: this.camera.position.x, y: this.camera.position.y, z: this.camera.position.z };
  }

  /** 当前朝向（弧度） */
  getRotation(): { yaw: number; pitch: number } {
    return { yaw: this.yaw, pitch: this.pitch };
  }

  /**
   * 每帧调用。返回是否发生了移动。
   * @param delta 帧间隔秒数
   * @param colliders 碰撞 AABB 列表（可选）
   */
  update(delta: number, colliders?: Collider[]): boolean {
    if (!this.isLocked) return false;

    const playerRadius = 0.15; // 角色碰撞半径 15cm（Q 版 1.45m 角色直径 ~30cm，过 0.85m 门洞绰绰有余

    // 当前脚下支撑面（动态地面：地面/各层平台/楼板/斜坡），用于贴地与视高复位
    const supportNow = this.supportY(this.camera.position.x, this.camera.position.z, playerRadius, this.camera.position.y - this.currentEyeY, colliders);
    const groundNow = supportNow != null ? Math.max(supportNow, this.floorY) : this.floorY;

    // 地面时先复位 y，消除上一帧 head bob 的偏移（bob 不累积进逻辑高度）
    if (this.isGrounded) {
      this.camera.position.y = groundNow + this.currentEyeY;
    }

    // ---- 1. 根据状态选速度上限与视高 ----
    // 加速有两种触发：按住 Shift，或按住鼠标右键（右手操作更方便）
    const sprinting = this.sprint || this.mouseRightDown;
    let maxSpeed: number;
    if (this.crouch) {
      maxSpeed = this.crouchSpeed;
      this.targetEyeY = this.crouchHeight;
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
        const drop = control * this.groundFriction * delta;
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
    if (Math.abs(dx) > 1e-6 || Math.abs(dz) > 1e-6) {
      const newX = this.camera.position.x + dx;
      const newZ = this.camera.position.z + dz;

      // 分轴检测（允许贴墙滑动）
      if (!this.collidesAt(newX, this.camera.position.z, playerRadius, colliders)) {
        this.camera.position.x = newX;
        moved = true;
      }
      if (!this.collidesAt(this.camera.position.x, newZ, playerRadius, colliders)) {
        this.camera.position.z = newZ;
        moved = true;
      }
    }

    const wasMoving = moved || horizSpeed > 0.1;

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

    // ---- 6. Head Bob（行走晃动，仅地面 + 移动时）----
    if (wasMoving && this.isGrounded) {
      // 步频 = 水平速度 / 步长，跑步步长略大于走路
      const stride = sprinting ? 1.6 : 1.3;
      this.bobPhase += delta * (Math.max(horizSpeed, 0.01) / stride) * Math.PI * 2;
      const amp = this.crouch
        ? this.crouchBobAmount
        : sprinting ? this.sprintBobAmount : this.bobAmount;
      this.camera.position.y += Math.sin(this.bobPhase) * amp;
    } else {
      // 停止/腾空时相位快速衰减，避免恢复移动时从突兀角度起跳
      this.bobPhase *= 0.85;
    }

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

  // 鼠标右键按住 = 加速（等价于 Shift 跑步），松开恢复
  private onMouseDown = (e: MouseEvent): void => {
    if (e.button === 2) this.mouseRightDown = true;
  };

  private onMouseUp = (e: MouseEvent): void => {
    if (e.button === 2) this.mouseRightDown = false;
  };

  private onContextMenu = (e: MouseEvent): void => {
    e.preventDefault(); // 阻止右键菜单（尤其非锁定状态下）
  };

  private onKeyDown = (e: KeyboardEvent): void => {
    if (!this.isLocked) return;
    // 忽略长按自动重复，保证跳跃/蹲下是边沿触发
    if (e.repeat) return;
    switch (e.code) {
      case 'KeyW': case 'ArrowUp':    this.moveForward = true; break;
      case 'KeyS': case 'ArrowDown':  this.moveBackward = true; break;
      case 'KeyA': case 'ArrowLeft':  this.moveLeft = true; break;
      case 'KeyD': case 'ArrowRight': this.moveRight = true; break;
      case 'ShiftLeft': case 'ShiftRight': this.sprint = true; break;
      case 'Space':
        e.preventDefault(); // 阻止页面滚动
        this.jump = true;
        break;
      case 'ControlLeft': case 'ControlRight':
        e.preventDefault();
        this.crouch = true;
        break;
    }
  };

  private onKeyUp = (e: KeyboardEvent): void => {
    switch (e.code) {
      case 'KeyW': case 'ArrowUp':    this.moveForward = false; break;
      case 'KeyS': case 'ArrowDown':  this.moveBackward = false; break;
      case 'KeyA': case 'ArrowLeft':  this.moveLeft = false; break;
      case 'KeyD': case 'ArrowRight': this.moveRight = false; break;
      case 'ShiftLeft': case 'ShiftRight': this.sprint = false; break;
      case 'Space':  this.jump = false; break;
      case 'ControlLeft': case 'ControlRight': this.crouch = false; break;
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
      this.onUnlock?.();
    }
  };

  private onLockError = (): void => {
    // 无用户手势时浏览器拒绝锁定：交给 onUnlock 语义（RoomScene 显示点击遮罩）
    this.isLocked = false;
    this.onUnlock?.();
  };

  /** 根据欧拉角更新相机四元数 */
  private updateOrientation(): void {
    this.camera.rotation.set(0, 0, 0, 'YXZ');
    this.camera.rotateY(this.yaw);
    this.camera.rotateX(this.pitch);
  }
}
