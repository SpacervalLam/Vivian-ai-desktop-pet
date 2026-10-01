/**
 * 碰撞检测工具
 *
 * 从 Three.js 场景中收集所有 nav 家具和墙体的世界空间 AABB，
 * 供 FPSControls 做移动时的穿墙检测。
 *
 * 用法：
 *   const colliders = buildColliderList(scene, layout);
 *   // 每帧传给 fpsControls.update(delta, colliders)
 */

import * as THREE from 'three';

export interface Collider {
  min: THREE.Vector3;
  max: THREE.Vector3;
  /** 可选：来源标识，调试用 */
  source?: string;
  /** 碰撞盒类型：wall=阻挡水平移动；floor=可站立薄板（楼板/平台/地面）；ramp=斜坡（用 heightAt 取表面高度，不阻挡水平移动） */
  kind?: 'wall' | 'floor' | 'ramp';
  /** 斜坡表面高度函数：返回 (x,z) 处可站立高度，或 null（不在斜坡范围内） */
  heightAt?: (x: number, z: number) => number | null;
}

/**
 * 从碰撞表里挑出「头顶的水平板」——**只用来做遮挡判定**，不参与行走碰撞。
 *
 * 用途：街面积水那块水平镜面（`store-wet-reflection`）的可见性。视锥剔除只能挡住
 * "根本不在画面里"的情形；站在 203 室中间低头看街面时，积水确实落在视锥里，却被
 * 3.4m 的楼板挡得一个像素都露不出来 —— 实测这种机位反射面的真实像素贡献是 0，而
 * 那一趟"把整个场景按镜像相机再提交一遍"要 400+ 次提交、占整帧三成。
 *
 * 三条判据缺一不可：
 *   1. **薄**（厚度 < maxThickness）：楼板 / 平台 / 雨棚这类"整片实心、没有洞"的
 *      构件才满足。墙、柱、栏杆都又厚又窄，直接被排除。
 *   2. **大**（长宽都 > minSpan）：一片板必须真的盖住视野，一小块台面挡不住什么。
 *   3. **整体位于 `aboveY` 之上**（含 clearance 余量）。这条最容易被漏掉却最关键：
 *      街面 `street` 的 AABB 是 y[-0.02, 0.02]，而积水镜面就在 y = 0.009 —— 镜面
 *      正坐在它**里面**。不排除它的话，"从任何位置看积水"都会被判成被街面挡住。
 *      物理上也很清楚：只有**位于镜面之上**的板才可能挡住镜面。
 *
 * 为什么不能拿"全部碰撞盒"当遮挡体（上一版踩过的坑）：`rail-z-6.3`（阳台栏杆）的
 * AABB 是 x[-6.24,6.24] y[3.4,6.2] z[6.26,6.34] —— z 向只有 8cm 厚、却有 2.8m 高，
 * 一整片实心盒。拿它当遮挡体，"站在阳台上看街面"会被判成看不见。这条误判实测直接
 * 让整个方案不可用。
 *
 * 也不适合改成"遍历场景图自己认楼板"：`mergeByMaterial` 会把同材质的板并成大 mesh，
 * 并完之后 AABB 跨越多层或横跨整条街（实测 `planned-street-network` 里并出 98×132m
 * 的"薄板"），照认就会把阳台机位误剔。碰撞表是**合批之前**逐件收的，AABB 才是准的。
 */
export function pickOverheadSlabs(
  colliders: Collider[],
  aboveY: number,
  maxThickness = 0.35,
  minSpan = 4,
  clearance = 0.05
): THREE.Box3[] {
  const out: THREE.Box3[] = [];
  for (const c of colliders) {
    const dy = c.max.y - c.min.y;
    const dx = c.max.x - c.min.x;
    const dz = c.max.z - c.min.z;
    if (dy >= maxThickness || dx <= minSpan || dz <= minSpan) continue;
    if (c.min.y <= aboveY + clearance) continue;
    out.push(new THREE.Box3(c.min.clone(), c.max.clone()));
  }
  return out;
}

/**
 * 遍历补碰撞的「包裹体」阈值（米）：三轴中最短边超过它就不当作可碰撞实体，只可能是
 * 把整个场景罩住的幕布/天空/远景（它们半径几十米，AABB 是三轴都很厚的巨大实体块）。
 * 收进来的后果不是多一个盒，而是玩家在地图任何位置都判定撞墙、第一人称 WASD 彻底失效。
 *
 * 阈值取 20m 是刻意保守的：实体障碍物可以很厚（便利店后段实心体量就有 3.5m 厚，
 * 玩家能走到对街，本来就该整块挡住），但没有任何真实障碍物是三轴都超过 20m 的实心块。
 */
const SOLID_MASS_MIN = 20;

/**
 * 遍历场景，收集所有需要碰撞检测的物体的世界 AABB。
 *
 * 规则：
 *   - 带 `__outline` name 的跳过（描边壳）
 *   - nav:false 的家具跳过（地毯、窗等不挡路）
 *   - 每个实体 mesh 的几何包围盒 + 0.05m 余量
 */
export function buildColliderList(
  scene: THREE.Object3D,
  /** layout.furniture 中 nav !== false 的家具 ID 集合 */
  navFurnitureIds?: Set<string>
): Collider[] {
  const colliders: Collider[] = [];
  const box = new THREE.Box3();
  const padding = 0.05; // 5cm 余量，防止贴脸穿模

  scene.traverse((obj) => {
    const mesh = obj as THREE.Mesh;
    if (!mesh?.isMesh) return;
    if (mesh.name === '__outline') return;
    if (!mesh.geometry) return;
    // 跳过显式标记为非碰撞的网格（外楼梯踏步/斜梁等——它们的碰撞由斜坡盒接管，
    // 否则逐级踏步会被当成空气墙，第一人称上楼一卡一卡的）
    if (mesh.userData?.noCollide) return;

    // 跳过透明/半透明物体（窗帘、玻璃等）
    const mat = mesh.material as THREE.Material | THREE.Material[];
    const materials = Array.isArray(mat) ? mat : [mat];
    const isTransparent = materials.some((m) => m.transparent && m.opacity < 0.5);
    if (isTransparent) return;

    // 如果提供了 nav ID 白名单，检查父级组名是否在名单中
    if (navFurnitureIds && navFurnitureIds.size > 0) {
      let parent = mesh.parent;
      let found = false;
      while (parent && !found) {
        if (navFurnitureIds.has(parent.name || '')) {
          found = true;
        }
        parent = parent.parent;
      }
      // 不在白名单中的跳过（但墙体总是保留）
      const isWall = mesh.userData?.isWall === true;
      if (!found && !isWall) return;
    }

    mesh.updateWorldMatrix(true, true);
    box.setFromObject(mesh);

    // 跳过退化盒子
    if (box.isEmpty() || !isFinite(box.max.x)) return;

    // 加余量
    colliders.push({
      min: new THREE.Vector3(box.min.x - padding, box.min.y - padding, box.min.z - padding),
      max: new THREE.Vector3(box.max.x + padding, box.max.y + padding, box.max.z + padding),
      source: mesh.parent?.name || mesh.name || undefined,
    });
  });

  return colliders;
}

/**
 * 从 layout.furniture 直接算家具碰撞盒（不遍历场景 mesh）。
 *
 * 与 navGrid.ts 的障碍物构建**完全同源同算法**：nav !== false、有正的 size[0]/size[2]，
 * 标称尺寸绕 Y 旋转后取 AABB。两边同源才能保证「宠物走得过去的地方玩家也走得过去」，
 * 也让 `scripts/check-layout.mjs` 的连通性校验同时覆盖第一人称
 * （nav 用 0.18 半径 + 0.09 墙半厚，FPS 用 0.15 + 0.06，处处更宽松）。
 *
 * 不用 buildColliderList 遍历场景的原因有两个：
 *   1. 实际 mesh 外廓比标称 size 大（餐椅外扩、电竞椅后仰…），会造出 nav 认为能过、
 *      FPS 却过不去的夹点；
 *   2. 场景里的家具组并没有以 id 命名，白名单匹配不上会静默丢掉全部家具碰撞。
 */
export function buildFurnitureColliders(
  furniture: Array<{
    id: string;
    nav?: boolean;
    pos: number[];
    size?: number[];
    rot?: number;
  }>
): Collider[] {
  const colliders: Collider[] = [];
  for (const it of furniture) {
    if (it.nav === false) continue;
    const s = it.size;
    if (!Array.isArray(s) || !(s[0] > 0) || !(s[2] > 0)) continue;
    const [x, , z] = it.pos;
    const r = it.rot ?? 0;
    const c = Math.abs(Math.cos(r));
    const sn = Math.abs(Math.sin(r));
    const ex = (s[0] * c + s[2] * sn) / 2;
    const ez = (s[0] * sn + s[2] * c) / 2;
    const h = s[1] > 0 ? s[1] : 0.9;
    colliders.push({
      min: new THREE.Vector3(x - ex, 0, z - ez),
      max: new THREE.Vector3(x + ex, h, z + ez),
      source: it.id,
    });
  }
  return colliders;
}

/**
 * 声明式盒：世界坐标中心 + 尺寸。家具之外的世界实体（公寓外壳四壁等）用它描述实心体块。
 */
export interface BoxColliderSpec {
  id: string;
  pos: [number, number, number];
  size: [number, number, number];
}

/**
 * 声明式长方体列表 → 碰撞盒（世界坐标，kind:'wall'）。
 *
 * 与 buildFurnitureColliders 是同一套机制——「数据声明一个轴对齐长方体 → 生成世界 AABB」，
 * 只是坐标系约定不同：家具写在户型局部坐标、底面恒贴单元地板（所以 y 固定从 0 起、
 * 由调用方整户抬升 UNIT_LIFT）；外壳 / 楼体这类世界坐标实体跨 1F~4F 多层，
 * y 由 pos[1] 显式给中心高、size[1] 给高度，不能沿用家具的 0 基约定。
 *
 * 公寓楼全部走这一条：楼体四壁是 exterior.ts 的 APT_WALL_BOXES，阳台栏杆 / 外廊腰壁 /
 * 外置楼梯栏杆 / 自行车是 buildApartmentShell 随 geometry 一并交出的 boxes，
 * 两者拼成同一张表。它们都不能走 buildSceneColliders 遍历——外壳主体是实心整块、
 * 门洞只在 layout 层挖，遍历会把入户门重新堵死、地坪高度还会生成水平盒把人钉死。
 */
export function buildBoxColliders(boxes: BoxColliderSpec[]): Collider[] {
  const out: Collider[] = [];
  for (const b of boxes) {
    const [x, y, z] = b.pos;
    const [sx, sy, sz] = b.size;
    if (!(sx > 0) || !(sy > 0) || !(sz > 0)) continue;
    out.push({
      min: new THREE.Vector3(x - sx / 2, y - sy / 2, z - sz / 2),
      max: new THREE.Vector3(x + sx / 2, y + sy / 2, z + sz / 2),
      kind: 'wall',
      source: b.id,
    });
  }
  return out;
}

/**
 * 构建墙体碰撞盒列表（从 shell.walls —— 也就是真正建了墙面的那 9 面墙 —— 直接算）。
 *
 * ⚠️ 不能从 shell.rooms 的矩形边界推墙：房间只是地面分区，开放式 LDK
 * （客厅/餐厅/厨房之间）在 shell.walls 里根本没有墙，但房间矩形是相邻的。
 * 按房间边界造墙会在这些「开放分界线」上凭空生成整堵实心墙 —— 视觉上什么都没有、
 * 第一人称却完全走不过去，就是所谓的「空气墙」。墙面渲染和 nav 栅格都只认
 * shell.walls，这里必须同源，否则三套口径互相打架。
 *
 * @param walls shell.walls 定义（axis:'z' 表示墙在 z=at、沿 X 延伸；'x' 反之）
 * @param height 层高
 * @param wallThickness 墙厚
 * @param doorways 门洞列表：在墙段上挖掉开口，否则门洞被连续实心墙堵死、永远过不去
 * @param railings 栏杆（阳台边缘）：没有墙但要挡人，防止走出边缘掉进虚空
 * @returns 所有墙体的 AABB（门洞处断开）+ 栏杆
 */
export function buildWallColliders(
  walls: Array<{ id?: string; axis: 'x' | 'z'; at: number; from: number; to: number }>,
  height: number,
  wallThickness: number,
  doorways: Array<{ x: number; z: number; width: number; alongX: boolean }> = [],
  railings?: Array<{ axis: 'x' | 'z'; at: number; from: number; to: number }>
): Collider[] {
  const colliders: Collider[] = [];
  const t = wallThickness / 2;

  for (const seg of walls) {
    // 收集落在这段墙上的门洞，按沿墙坐标切分成若干开口区间
    const cuts: Array<{ from: number; to: number }> = [];
    for (const d of doorways) {
      if (seg.axis === 'z') {
        // 墙在 z=at、沿 x 方向；门洞沿 X 开口（alongX），门洞 z 要贴住 seg.at
        if (!d.alongX) continue;
        if (Math.abs(d.z - seg.at) > t + 0.2) continue;
        if (d.x < seg.from || d.x > seg.to) continue;
        cuts.push({ from: d.x - d.width / 2, to: d.x + d.width / 2 });
      } else {
        // 墙在 x=at、沿 z 方向；门洞沿 Z 开口，门洞 x 要贴住 seg.at
        if (d.alongX) continue;
        if (Math.abs(d.x - seg.at) > t + 0.2) continue;
        if (d.z < seg.from || d.z > seg.to) continue;
        cuts.push({ from: d.z - d.width / 2, to: d.z + d.width / 2 });
      }
    }

    // 门洞把墙切成若干段：跳过门洞区间，其余生成碰撞盒
    cuts.sort((a, b) => a.from - b.from);
    const ranges: Array<[number, number]> = [];
    let cur = seg.from;
    for (const c of cuts) {
      const cFrom = Math.max(seg.from, c.from);
      const cTo = Math.min(seg.to, c.to);
      if (cTo <= cFrom) continue;
      if (cur < cFrom) ranges.push([cur, cFrom]);
      cur = Math.max(cur, cTo);
    }
    if (cur < seg.to) ranges.push([cur, seg.to]);

    for (const [from, to] of ranges) {
      const label = seg.id ?? `${seg.axis}-${seg.at.toFixed(1)}`;
      if (seg.axis === 'z') {
        colliders.push({
          min: new THREE.Vector3(from - t, 0, seg.at - t),
          max: new THREE.Vector3(to + t, height, seg.at + t),
          source: `wall-${label}`,
        });
      } else {
        colliders.push({
          min: new THREE.Vector3(seg.at - t, 0, from - t),
          max: new THREE.Vector3(seg.at + t, height, to + t),
          source: `wall-${label}`,
        });
      }
    }
  }

  // 栏杆：实心薄碰撞盒，防止玩家走出阳台 / 楼层边缘掉进虚空（与 nav 栅格一致）。
  // 一旦把阳台落地玻璃门挖开、玩家能走上阳台，没这道栏杆就会直接穿出边缘。
  const rHalf = 0.04;
  for (const r of railings ?? []) {
    if (r.axis === 'z') {
      colliders.push({
        min: new THREE.Vector3(r.from - rHalf, 0, r.at - rHalf),
        max: new THREE.Vector3(r.to + rHalf, height, r.at + rHalf),
        source: `rail-z-${r.at.toFixed(1)}`,
      });
    } else {
      colliders.push({
        min: new THREE.Vector3(r.at - rHalf, 0, r.from - rHalf),
        max: new THREE.Vector3(r.at + rHalf, height, r.to + rHalf),
        source: `rail-x-${r.at.toFixed(1)}`,
      });
    }
  }

  return colliders;
}

/**
 * 场景遍历全量补碰撞：收集所有未被现有 layout 驱动碰撞（家具/墙/栏杆/地板/斜坡）
 * 覆盖的实体 mesh 的世界 AABB，作为「其余几何体模型」的碰撞盒。
 *
 * 用途：让第一人称能挡住所有实心几何体——室内绿植/落地灯/lifestyle 摆件（拖鞋/伞/快递箱）/
 * 书架上的书/吊灯/挂钟/海报，室外路灯/便利店货架/吧台/冷饮柜等，全都不在 layout.furniture
 * 里（或没有尺寸），原本能直接穿过去，现在都会被挡住。
 *
 * 公寓楼（外壳楼体 / 阳台栏杆 / 外廊腰壁 / 外置楼梯栏杆 / 自行车）不在本函数的职责内：
 * 它们由 APT_WALL_BOXES 与 buildApartmentShell 的 boxes 声明式给出，见 buildBoxColliders。
 *
 * 跳过规则（避免出现重复盒 / 伪空气墙 / 堵死门洞）：
 *   - __outline 描边壳
 *   - userData.noCollide（楼梯踏步/斜梁/平台板，碰撞由 floor/ramp 盒接管）
 *   - userData.isWallPiece / isWall（内/外墙；mesh 是实心整块、门洞只在 layout 层挖，
 *     遍历它反而会把门重新堵死——所以墙体碰撞只认 buildWallColliders）
 *   - userData.sceneCollideSkip（室外大建筑体块：公寓外壳/对面楼群/便利店主体/地面/房间
 *     外壳的楼板天花板——它们要么不该挡人、要么门洞只在 layout 层挖、要么铺在地面高度
 *     会生成水平阻挡盒把玩家钉死在地板，都交由各自已有的碰撞盒或 floor 盒处理）
 *   - userData.furnitureRoot（已被 buildFurnitureColliders 覆盖的家具组；标在家具组上，
 *     避免重复成更大盒子、造成 nav/第一人称口径夹点。只跳过有尺寸的 nav 家具，门/窗/地毯/
 *     浴室以及没有尺寸的摆件 kind 不标，好让它们各自落到本函数里补碰撞）
 *   - 透明/半透明（opacity<0.5）材质
 *   - 实体块兜底（见下方 SOLID_MASS_MIN）：三轴都很厚的巨型盒
 * 以上「结构性跳过标记」只要出现在该 mesh 自身或任一祖先上即生效（一次向上回溯）。
 *
 * 与 check-fps-reach.mjs 的关系：该校验只复刻 layout 驱动的碰撞盒（墙/栏杆/家具），
 * 看不到这里遍历出来的摆件碰撞——这是选择「场景遍历全量」方案时已知接受的盲区：
 * 摆件若恰好卡在过道造成的局部卡点，校验脚本抓不到，需进第一人称实测微调。
 */
export function buildSceneColliders(root: THREE.Object3D): Collider[] {
  const colliders: Collider[] = [];
  const box = new THREE.Box3();
  const size = new THREE.Vector3();
  const padding = 0.04; // 4cm 余量，防贴脸穿模（摆件通常较小，余量取小）

  const shouldSkip = (o: THREE.Object3D): boolean => {
    const ud = o.userData;
    return !!(
      ud?.sceneCollideSkip ||
      ud?.furnitureRoot ||
      ud?.isWall ||
      ud?.isWallPiece ||
      ud?.noCollide
    );
  };

  root.traverse((obj) => {
    const mesh = obj as THREE.Mesh;
    if (!mesh?.isMesh) return;
    if (mesh.name === '__outline') return;

    // 向上回溯：任一祖先带跳过标记即跳过（furnitureRoot 标在家具组、sceneCollideSkip
    // 标在大建筑体块根上，都能连带跳过其所有子孙 mesh）
    let p: THREE.Object3D | null = mesh;
    while (p) {
      if (shouldSkip(p)) return;
      p = p.parent;
    }

    const mat = mesh.material as THREE.Material | THREE.Material[] | undefined;
    const materials = !mat ? [] : Array.isArray(mat) ? mat : [mat];
    if (materials.some((m) => m && (m as THREE.Material).transparent && (m as THREE.Material).opacity < 0.5)) return;

    mesh.updateWorldMatrix(true, true);
    box.setFromObject(mesh);
    if (box.isEmpty() || !isFinite(box.max.x)) return;

    // 包裹体兜底：三轴都很厚的巨型实体块只可能是罩住整个场景的幕布/天空/远景
    // （外景夜景地平线环就是半径 78m 的圆柱，AABB 156×42×156）。漏掉这一步的后果不是
    // 多一个盒，而是玩家在地图任何位置都判定撞墙、第一人称 WASD 彻底失效，
    // 所以这里显式告警而不是静默收下。
    box.getSize(size);
    const minSide = Math.min(size.x, size.y, size.z);
    if (minSide > SOLID_MASS_MIN) {
      console.warn(
        `[collider] 跳过疑似幕布/天空的巨型实体块（${size.x.toFixed(1)}×${size.y.toFixed(1)}×${size.z.toFixed(1)}m）`,
        mesh.name || mesh.parent?.name || '(未命名)',
        '—— 它不该承担碰撞，请给它标 userData.noCollide'
      );
      return;
    }

    colliders.push({
      min: new THREE.Vector3(box.min.x - padding, box.min.y - padding, box.min.z - padding),
      max: new THREE.Vector3(box.max.x + padding, box.max.y + padding, box.max.z + padding),
      source: mesh.parent?.name || mesh.name || undefined,
    });
  });

  return colliders;
}

