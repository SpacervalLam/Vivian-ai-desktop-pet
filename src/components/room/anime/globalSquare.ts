import * as THREE from 'three';
import type { Collider } from './collider';
import { mergeByMaterial } from './merge';

/**
 * 南区东：世界广场 —— **欧式广场 + 中华街**。
 *
 * 存在的理由是一次配比失衡。整个街区（`sakuraTown` 商店街、`sakuraStation` 駅、
 * `civicQuarter` 的桜ヶ丘高校、`exterior` 的居酒屋/書店/西園寺前地铁口）几乎
 * 全是日本元素；唯一的非日式内容是 `worldStreetLife.ts` 那一小块（x -45…64,
 * z 83…112，意式三家店 + 中式茶市 + 美式餐车）。用户的原话是「地图里还可以增加
 * 一些中国，欧洲的城市元素，避免日本元素过多（但是不要过度删除原本就有的日本
 * 元素）」。
 *
 * 所以这一版**不删任何日本元素**，只在**风格空白的南区东**新开一片。选这块地
 * 有两条硬理由：
 *
 *  1. **它本来就是空的。** `cityDressing` 全模块没有 z>115 的落位（A7 段站东
 *     到 z=112 为止，A8 南带最南也只到 z=112.0），`civicQuarter` 的
 *     `district-ground`（152×80，中心 -4,123）只到 x=72。这块地在
 *     `WALK_BOUNDS`（x[-80,150] z[-78,165]）**之内**、边界墙以内，玩家走得到，
 *     此前只有世界地面。
 *  2. **它紧贴 LUMINA 的停车楼**（x 76…100）。商场 → 停车场 → 世界广场是一条
 *     连续的动线，不需要再造一条街把它们缝起来。
 *
 * 风格分工（西 → 东）：
 *
 *        x102   x109  x123.6 x126.8  x131.6  x144.8  x150
 *   z116  ┌───────┬─────┬──────┬──────────────┬──────┐
 *         │ 排屋  │柱廊 │ 巷   │   中华街      │ 骑楼 │
 *         │ 广场  │喷泉 │      │ 牌坊·灯笼串   │      │
 *   z164  └───────┴─────┴──────┴──────────────┴──────┘
 *
 *  **欧式**（x 102…123.6）：西侧一排坡屋顶排屋（老虎窗 + 烟囱 + 百叶 + 阳台）、
 *           西/北两面柱廊（额枋 + 檐口）、中心圆形喷泉、南端钟楼、露天咖啡座、
 *           街灯。语汇取自地中海/中欧旧城，与 `worldStreetLife` 的意式三家店
 *           同源，不打架。
 *  **中式**（x 126.8…150）：南北两座牌坊（四柱三间 + 琉璃顶 + 匾额）、两侧骑楼
 *           （**底层券柱廊真的让出来了**，见下方 qilou 一节的注释）、朱红檐柱、
 *           檐下一串串红灯笼、跨街灯笼串、中文店招（茶楼/药铺/点心/书店）。
 *
 * 昼夜：`setEnvironment(period)` 把 `litWarm`（欧式窗/咖啡座罩灯/钟面）、
 * `litRed`（中式灯笼）、`litCool`（街灯）三支材质按三档拉起，外加两盏点光。
 * 接线在 `districtArt.setEnvironment`。做法与 `civicQuarter.applyPeriod` 一致。
 *
 * 装配顺序：**必须在 `cityDressing` 之前建**。陈设模块的落位过滤器
 * （`free()` ← `keepOut`）吃的是"到目前为止的全部碰撞盒"，本模块的墙盒进了那份
 * 表，"扫街绿化"那一段才不会往广场中间种树。见 `districtArt.ts:66-70`。
 */

/** 广场铺装 / 建筑用的统一色板。 */
export function createGlobalSquare(scene: THREE.Scene) {
  const root = new THREE.Group();
  root.name = 'global-square';
  root.userData.sceneCollideSkip = true;
  scene.add(root);

  const colliders: Collider[] = [];
  const materials: THREE.Material[] = [];
  const textures: THREE.Texture[] = [];
  const geometries = new Set<THREE.BufferGeometry>();

  /* 第二参传字符串 = 自发光色 ≠ 基色；传 true = 自发光取基色（与 civicQuarter 同一套约定）。 */
  const material = (color: string, emissive: boolean | string = false) => {
    const em = emissive === true ? color : (emissive || null);
    const m = new THREE.MeshStandardMaterial({ color, roughness: .78, ...(em ? { emissive: em, emissiveIntensity: .6 } : {}) });
    m.userData.outlineWeight = 0;
    materials.push(m);
    return m;
  };

  const M = {
    /* 欧式 */
    stone: material('#d5cfc1'), cream: material('#efe7d6'), ochre: material('#c08a5e'),
    slate: material('#49535e'), terracotta: material('#a85c42'), iron: material('#39404a'),
    /* 中式 */
    vermilion: material('#b23a2c'), gold: material('#d2a244'), tile: material('#39454e'),
    timber: material('#8a5a3c'), jade: material('#7d9e8a'),
    /* 通用 */
    paving: material('#b4b1a6'), cnPaving: material('#a89e8c'),
    road: material('#586675'), water: material('#7fb6c4'),
    /* 灯（初始 intensity 压到 0，由 applyPeriod 拉起来） */
    litWarm: material('#f2e0be', '#ffcf92'),   // 欧式窗 + 咖啡座罩灯 + 钟面
    /* 灯笼。基色和自发光都取**饱和的朱红**，不是第一版的 #e08a78/#ff6a4d ——
     * 那两个是鲑鱼粉，白天读作淡粉、夜里被 bloom 一晕更粉，"红灯笼"这个读数就没了。
     * 自发光压到 0.85 也是同一个理由：再高会被 tonemapping 推到近白。 */
    litRed: material('#c0392b', '#e03414'),    // 中式灯笼
    litCool: material('#d2e2e8', '#e2f0ff'),   // 街灯
  };
  for (const m of [M.litWarm, M.litRed, M.litCool]) m.emissiveIntensity = 0;

  /* 共享基元。**循环里绝不 new 几何** —— 否则 `dispose()` 漏放、几何数虚高，
   * 而且 `dedupeGeometries` 要在 78MB 顶点数据上白跑一遍逐字节哈希。 */
  const cube = new THREE.BoxGeometry(1, 1, 1);
  const cylinder = new THREE.CylinderGeometry(1, 1, 1, 14);
  const basin = new THREE.CylinderGeometry(1, 1, 1, 28);
  const ball = new THREE.SphereGeometry(1, 12, 10);
  const cone = new THREE.ConeGeometry(1, 1, 12);
  for (const g of [cube, cylinder, basin, ball, cone]) geometries.add(g);

  const box = (x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material) => {
    const o = new THREE.Mesh(cube, m); o.position.set(x, y, z); o.scale.set(w, h, d);
    o.castShadow = h > .25; o.receiveShadow = true; root.add(o); return o;
  };
  const block = (name: string, x: number, y: number, z: number, w: number, h: number, d: number, m: THREE.Material, kind: Collider['kind'] = 'wall') => {
    box(x, y, z, w, h, d, m);
    colliders.push({ source: `gs-${name}`, kind, min: new THREE.Vector3(x - w / 2, y - h / 2, z - d / 2), max: new THREE.Vector3(x + w / 2, y + h / 2, z + d / 2) });
  };
  const rod = (a: number[], b: number[], r: number, m: THREE.Material) => {
    const from = new THREE.Vector3(...a), v = new THREE.Vector3(...b).sub(from);
    const o = new THREE.Mesh(cylinder, m); o.position.copy(from.addScaledVector(v, .5));
    o.scale.set(r, v.length(), r);
    o.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), v.normalize());
    o.castShadow = true; root.add(o); return o;
  };
  /**
   * 招牌。`face` = 平面法线朝哪边：'n' → -z、's' → +z、'e' → +x、'w' → -x。
   *
   * 上一版只有 'n'/'s'，于是排屋（面朝 +x）和骑楼（面朝 ±x）的招牌全都只能
   * 横贴在墙上 —— 从广场看过去是一块侧过来的窄条。
   */
  const board = (title: string, subtitle: string, x: number, y: number, z: number, w: number, h: number, style: 'eu' | 'cn', face: 'n' | 's' | 'e' | 'w' = 'n', glow = true) => {
    const cv = document.createElement('canvas'); cv.width = 1024; cv.height = Math.max(256, Math.round(1024 * h / w));
    const c = cv.getContext('2d')!, H = cv.height;
    const g = c.createLinearGradient(0, 0, 0, H);
    if (style === 'cn') { g.addColorStop(0, '#7d1f16'); g.addColorStop(1, '#c2513a'); }
    else { g.addColorStop(0, '#243a56'); g.addColorStop(1, '#5a7392'); }
    c.fillStyle = g; c.fillRect(0, 0, 1024, H);
    c.globalAlpha = .18; c.strokeStyle = '#ffffff'; c.lineWidth = 3;
    for (let n = 0; n < 6; n++) { c.beginPath(); c.arc(880, H * .3, 50 + n * 58, 0, Math.PI * 2); c.stroke(); }
    c.globalAlpha = 1;
    c.fillStyle = '#faf3df';
    c.font = `700 ${Math.min(112, H * .3)}px "Microsoft YaHei",sans-serif`;
    c.fillText(title, 52, H * .48, 920);
    c.font = `${Math.min(40, H * .12)}px sans-serif`;
    c.fillText(subtitle, 55, H * .74, 900);
    c.fillStyle = style === 'cn' ? '#e8c05a' : '#d8b070';
    c.fillRect(55, H * .82, 150, 6);
    const tex = new THREE.CanvasTexture(cv); tex.colorSpace = THREE.SRGBColorSpace; textures.push(tex);
    const m = new THREE.MeshStandardMaterial({ map: tex, roughness: .62, ...(glow ? { emissive: '#ffffff', emissiveMap: tex, emissiveIntensity: .75 } : {}) });
    m.userData.outlineWeight = 0; materials.push(m);
    const geo = new THREE.PlaneGeometry(w, h); geometries.add(geo);
    const mesh = new THREE.Mesh(geo, m);
    mesh.position.set(x, y, z);
    mesh.rotation.y = face === 'n' ? Math.PI : face === 'e' ? Math.PI / 2 : face === 'w' ? -Math.PI / 2 : 0;
    root.add(mesh); return m;
  };

  /* ================================================================
   * 0. 地坪
   * ================================================================
   * 顶面统一压到 **y=0.10**，与 `civicQuarter` 的 `mall-plaza-east` /
   * `mall-east-ground` 同高、并**在 x=102 处对齐拼接**（那两块的地坪东界已收到
   * x=102）。同高 + 只相切不重叠 ⇒ 没有共面 z-fighting，也没有台阶。
   * 上一版这里顶面是 0.05，比商场那边的 0.09/0.10 低 4~5cm，接缝处会露出一条
   * 台阶；而且停车楼南侧那 5 条地面划线（y .02…04）会被新地坪整个埋掉。
   */
  block('ground', 126, .05, 140, 48, .10, 48, M.paving, 'floor');        // x 102..150, z 116..164
  /* 中央巷道（把欧式广场和中华街分开）。西界贴着柱廊额枋 123.55，东界贴着骑楼 126.8。 */
  /* 中华街街面 + 中央巷道：**比广场铺装抬 0.08m**，不是 0.02。
   *
   * 这两条是"铺在广场地坪之上的补丁"，footprint 必然重叠 ⇒ 只能靠高差分开。
   * 第一版给 0.02，核账③直接报 `gs-ground × gs-cn-street 643.2m² Δ=0.02` ——
   * 这个高差**不够**：near=0.1 / far≈400 的 24bit 深度缓冲在 200m 处只能分辨
   * 约 2.4cm，从公寓阳台（离广场 ~160m）看过去就会闪。
   *
   * 判据是「重叠 **且** 高差 < 2cm」，但真正决定闪不闪的是**颜色差**：
   * 场景里本来就有大片共面地板（`mall-boulevard × mall-boulevard` 几十处 Δ=0），
   * 那些是同一材质同一色，闪了也看不出来；这里两边是 #b4b1a6 / #a89e8c 两种
   * 颜色，闪一下就露馅。8cm 在 300m 处仍有裕量，视觉上读作一道铺装压边。 */
  block('lane', 124.9, .14, 140, 2.6, .08, 48, M.road, 'floor');
  block('cn-street', 138, .14, 140, 13.4, .08, 48, M.cnPaving, 'floor');

  /* ================================================================
   * 1. 欧式广场（x 102…123.6，中心 116.4, 140）
   * ================================================================ */
  const EX = 116.4, EZ = 140;

  /* 西侧一排排屋：坡屋顶 + 老虎窗 + 烟囱 + 百叶 + 阳台。四栋，面宽 6.4、进深 8.6，
   * 沿 z 排开，**正面朝东**（朝广场）。 */
  for (let i = 0; i < 4; i++) {
    const z = 126.5 + i * 9, h = 11 + (i % 2) * 1.6, fx = 109.2;
    block('eu-terrace', 106, h / 2, z, 6.4, h, 8.6, i % 2 ? M.cream : M.ochre);
    /* 坡屋顶：两块斜板 + 屋脊（屋脊沿 z，所以斜板绕 z 轴翻）。
     *
     * 倾斜方向是 **`-s`**，不是 `s`。绕 +z 转 θ 把 (X,0) 送到 (X cosθ, X sinθ)
     * —— θ>0 时局部 +x 端**抬起来**。西侧那块（s=-1，中心在屋脊以西）要的是
     * "越往西越低"，也就是局部 +x 端（靠屋脊那头）更高 ⇒ θ>0。写成 `s*.52`
     * 两块的外端一起翘、屋脊处反而凹下去，是**蝶形屋顶**，不是人字坡。 */
    for (const s of [-1, 1]) { const r = box(106 + s * 1.9, h + .85, z, 3.6, .34, 9.2, M.slate); r.rotation.z = -s * .52; }
    box(106, h + 1.95, z, .5, .35, 9.2, M.slate);
    /* 老虎窗（东向坡面）+ 烟囱。 */
    box(107.7, h + 1.5, z - 2.4, 1.5, 1.3, 1.5, M.cream);
    box(104.6, h + 2.6, z + 2.8, .9, 2.4, .9, M.terracotta);
    /* 三层窗 + 百叶 + 阳台，全部开在东立面。 */
    for (let f = 0; f < 3; f++) for (let c = 0; c < 3; c++) {
      const pz = z - 2.4 + c * 2.4, py = 2.2 + f * 3.2;
      box(fx + .06, py, pz, .1, 2.1, 1.5, M.litWarm);
      for (const s of [-1, 1]) box(fx + .12, py, pz + s * .86, .07, 2.2, .22, M.terracotta);
      if (f === 1) {
        box(fx + .3, py - 1.2, pz, .5, .12, 1.9, M.iron);
        for (let n = 0; n < 6; n++) box(fx + .45, py - .75, pz - .75 + n * .3, .05, .5, .05, M.iron);
      }
    }
  }

  /* 西侧柱廊：在排屋正面之外再立一列柱，柱 + 额枋 + 檐口。**x 取 110.4**
   * （排屋东立面 109.2 之外 1.2m），不会像上一版那样横梁从楼体里穿过去。 */
  for (let i = 0; i < 6; i++) block('colonnade', 110.4, 2.6, 124.5 + i * 6, .52, 5.2, .52, M.stone);
  box(110.4, 5.55, 139.5, 1.1, .7, 34, M.stone);
  box(110.4, 6.1, 139.5, 1.6, .45, 35, M.cream);
  /* 北侧柱廊（收住北端轴线）。 */
  for (let i = 0; i < 4; i++) block('colonnade', 110.4 + i * 4, 2.6, 120.5, .52, 5.2, .52, M.stone);
  box(116.4, 5.55, 120.5, 13.6, .7, 1.1, M.stone);
  box(116.4, 6.1, 120.5, 14.4, .45, 1.6, M.cream);

  /* 中心喷泉：圆形水池 + 中心柱 + 双层水盘 + 金顶饰。 */
  const pool = new THREE.Mesh(basin, M.stone); pool.scale.set(4.6, .55, 4.6); pool.position.set(EX, .38, EZ); pool.receiveShadow = true; root.add(pool);
  const water = new THREE.Mesh(basin, M.water); water.scale.set(4.1, .1, 4.1); water.position.set(EX, .66, EZ); root.add(water);
  rod([EX, .62, EZ], [EX, 4.3, EZ], .42, M.stone);
  const bowl = new THREE.Mesh(basin, M.stone); bowl.scale.set(2.3, .3, 2.3); bowl.position.set(EX, 4.5, EZ); bowl.castShadow = true; root.add(bowl);
  const bowl2 = new THREE.Mesh(basin, M.stone); bowl2.scale.set(1.25, .24, 1.25); bowl2.position.set(EX, 6, EZ); bowl2.castShadow = true; root.add(bowl2);
  rod([EX, 6.1, EZ], [EX, 7.4, EZ], .17, M.stone);
  const finial = new THREE.Mesh(ball, M.gold); finial.scale.setScalar(.42); finial.position.set(EX, 7.7, EZ); root.add(finial);
  /* 水池必须登记碰撞盒 —— 它是 4.6m 半径的实体，玩家不该能走进去。
   * 顺带把"扫街绿化"那一段挡在外面（`free()` 读到这一条就不会往喷泉上种树）。 */
  colliders.push({ source: 'gs-fountain', kind: 'wall', min: new THREE.Vector3(EX - 4.6, 0, EZ - 4.6), max: new THREE.Vector3(EX + 4.6, 1.0, EZ + 4.6) });

  /* 广场露天咖啡座（东南角，避开喷泉、柱廊与钟楼）。 */
  for (const [cx, cz] of [[120.5, 148], [118, 151], [122, 145]]) {
    rod([cx, .05, cz], [cx, .76, cz], .05, M.iron);
    box(cx, .78, cz, 1, .07, 1, M.cream);
    for (const d of [[-.82, 0], [.82, 0], [0, -.82], [0, .82]]) block('cafe-chair', cx + d[0], .42, cz + d[1], .42, .84, .42, M.iron);
    rod([cx, .8, cz], [cx, 2.7, cz], .025, M.iron);
    const umb = new THREE.Mesh(cone, M.terracotta); umb.scale.set(1.5, .7, 1.5);
    umb.position.set(cx, 2.9, cz); umb.castShadow = true; root.add(umb);
  }

  /* 广场角落的黄杨花池（给空地上一点体量，也顺手把"扫街绿化"的落点挡掉）。 */
  for (const [px, pz] of [[112.5, 125.5], [121, 125.5], [112.5, 152.5], [121, 152.5]]) {
    block('topiary-planter', px, .45, pz, 1.8, .9, 1.8, M.stone);
    const b = new THREE.Mesh(ball, M.jade); b.scale.set(.85, .95, .85); b.position.set(px, 1.75, pz); b.castShadow = true; root.add(b);
  }

  /* 喷泉外两圈铺装环。**不是装饰**：14×48m 的广场全铺一种灰，从人视高度看过去
   * 就是一块没有尺度的地面，喷泉像丢在停车场里。两圈浅色环给广场一个中心与
   * 尺度（内环 13.2m 直径 ≈ 一辆车绕行的圆环，外环 19.2m 收住整个轴线）。
   * 抬高 0.06m 与中华街街面同法，避免和广场地坪共面。
   *
   * 外环半径**卡在 7.0**：再大一点（>7.2）就会伸进 x≥123.6 的中央巷道，和那条
   * 抬 0.08m 的路面形成"重叠 + 高差 2cm"的临界对，又回到共面那一类问题上。
   * 7.0 时西缘到 x=109.4、东缘 123.4，正好整圈留在广场铺装里。 */
  for (const [r0, r1] of [[5.0, 5.6], [6.4, 7.0]]) {
    const ring = new THREE.RingGeometry(r0, r1, 56); geometries.add(ring);
    const m = new THREE.Mesh(ring, M.cream);
    m.rotation.x = -Math.PI / 2; m.position.set(EX, .16, EZ); m.receiveShadow = true; root.add(m);
  }

  /* 街灯（欧式：柱 + 罩灯）。 */
  for (const [lx, lz] of [[111.5, 118], [122.5, 118], [111.5, 156], [122.5, 156]]) {
    block('streetlamp', lx, 2.6, lz, .16, 5.2, .16, M.iron);
    box(lx, 5.4, lz, .7, .5, .7, M.litCool);
  }

  /* 广场铭牌：立式，两根柱子 + 面板，朝南（从停车楼那侧走过来正对）。 */
  for (const dx of [-2.6, 2.6]) block('signpost', 116.4 + dx, 1.6, 117.9, .22, 3.2, .22, M.iron);
  board('PIAZZA  VERDE', 'CAFFÈ  ·  GELATO  ·  FOUNTAIN', 116.4, 3.6, 117.75, 6, 1.7, 'eu', 's');
  /* 排屋山墙招牌（贴在排屋东立面高处，面朝广场）。 */
  board('TRATTORIA', 'CUCINA  ·  VINO', 109.34, 8.6, 126.5, 5.4, 1.6, 'eu', 'e');

  /* 钟楼。**不在广场中轴线上** —— 第一版放在 (EX, 159.5)，正好在喷泉正后方，
   * 人视轴线上喷泉的中柱 + 两层水盘和钟楼叠成一个"图腾"，看不出是两件东西。
   * 东移 4.2m 让两者分开；x 取 120.6（footprint 118…123.2）也避开了 x 123.6 起的巷道。 */
  const TWR = 120.6;
  block('clocktower', TWR, 9.5, 159.5, 5.2, 19, 5.2, M.stone);
  box(TWR, 19.6, 159.5, 6.2, .9, 6.2, M.cream);
  for (const s of [-1, 1]) { const r = box(TWR + s * 1.5, 20.8, 159.5, 3.0, .3, 5.2, M.slate); r.rotation.z = -s * .6; }
  const clockFace = new THREE.Mesh(new THREE.CircleGeometry(1.5, 24), M.litWarm); geometries.add(clockFace.geometry);
  clockFace.position.set(TWR, 15.4, 159.5 - 2.62); clockFace.rotation.y = Math.PI; root.add(clockFace);
  const clockRing = new THREE.Mesh(new THREE.TorusGeometry(1.62, .12, 6, 28), M.gold); geometries.add(clockRing.geometry);
  clockRing.position.set(TWR, 15.4, 159.5 - 2.6); root.add(clockRing);
  /* 指针 + 轴心。**不加就是一只空圈** —— 1.5m 半径的钟面在 35m 外只占十几个像素，
   * 没有指针就只是一个浅色圆片，"钟楼"这个读数立不住。
   * z 取 156.84 / 156.80，比钟面（156.88）再往 -z 偏 4~8cm，落在面之前。
   * 用 M.iron（#39404a）而不是一个"更黑的黑"：钟面是暖白发光片，深板岩蓝在它上面
   * 对比足够，且在阴天/夜里不会糊成一块死黑。 */
  box(TWR, 16.0, 159.5 - 2.66, .11, 1.25, .06, M.iron);
  box(TWR + .45, 15.4, 159.5 - 2.66, .95, .11, .06, M.iron);
  box(TWR, 15.4, 159.5 - 2.70, .24, .24, .06, M.gold);

  /* ================================================================
   * 2. 中华街（x 126.8…150，街心 138）
   * ================================================================ */
  const CX = 138;

  /* 南北两座牌坊：四柱三间，朱红柱 + 琉璃顶 + 匾额。
   * 柱顶抬到 7.6m，正好埋进上额枋（中心 7.4、厚 .72 ⇒ 7.04…7.76），
   * 上一版柱高只到 6.8，上额枋是**悬空**的。
   * `as const` 不是装饰：不加的话 TS 把每个元素推成 `(string|number)[]`，
   * 下面 pz / label / sub 全是 `string | number`，box/board 一路报参数类型错。 */
  for (const [pz, label, sub] of [[121.5, '中华街', 'CHINATOWN'], [158.5, '长安坊', "CHANG'AN  WARD"]] as const) {
    for (const px of [CX - 5.8, CX - 2.2, CX + 2.2, CX + 5.8]) block('paifang-column', px, 3.8, pz, .62, 7.6, .62, M.vermilion);
    for (const [w, y] of [[8.6, 5.6], [13.6, 7.4]]) { box(CX, y, pz, w, .72, .95, M.vermilion); box(CX, y + .5, pz, w + .9, .5, 1.5, M.tile); }
    for (const s of [-1, 1]) { const r = box(CX + s * 7.0, 8.15, pz, 2.6, .26, 1.5, M.tile); r.rotation.z = s * .42; }
    box(CX, 6.5, pz, 4.2, 1.5, .5, M.gold);
    /* 匾额面板贴住金框（金框 z 向 ±0.25 ⇒ 面板放 ±0.28）。第一版写 ±0.78，
     * 面板离框 0.53m，悬空飘在前面。牌坊两面都要有字：从北口进来看到的是北面，
     * 站在街里往南看到的是南面，只做一面必然有一半是空白背板。 */
    board(label, sub, CX, 6.5, pz - .28, 3.8, 1.35, 'cn', 'n');
    board(label, sub, CX, 6.5, pz + .28, 3.8, 1.35, 'cn', 's');
    /* 檐下红灯笼（吊杆 + 灯笼）。 */
    for (const px of [CX - 4.2, CX, CX + 4.2]) {
      rod([px, 5.2, pz], [px, 4.9, pz], .03, M.gold);
      const lan = new THREE.Mesh(ball, M.litRed); lan.scale.set(.42, .483, .42);
      lan.position.set(px, 4.5, pz); lan.castShadow = true; root.add(lan);
    }
  }

  /* 两侧骑楼。**这里是上一版最实的一处错**：楼身是一个从地面起的实心盒，
   * 而"券柱廊"的柱子摆在 `bx - s*2.1` —— 那个位置在楼体内部，柱子整个被埋掉，
   * 从街上看到的就是一栋没有骑楼的方盒子。
   *
   * 骑楼的定义恰恰相反：**底层临街那一跨让出来做有顶的券廊，人从廊下走**。
   * 所以这里拆成三段：
   *   · 底层后墙   x 靠街心一侧的 2.8m，y 0…4.35
   *   · 上层楼身   整个 4.8m 进深，y 4.35…h（骑楼的"骑"就是它骑在券廊上）
   *   · 券柱廊     柱立在临街外沿（bx ± 2.62），顶一条额枋
   * 西排 `s=-1`（券廊朝 +x / 朝街心），东排 `s=+1`（券廊朝 -x）。 */
  for (const [bx, s] of [[129.2, -1], [147.2, 1]]) {
    const face = bx - s * 2.4;              // 临街立面
    for (let i = 0; i < 4; i++) {
      const z = 129 + i * 8, h = 9.4 + (i % 2) * 1.2;
      const body = i % 2 ? M.timber : M.jade;
      /* 底层后墙（让出 2.0m 深的券廊）。用 `block()` 而不是 `box()`：后墙是实体，
       * 玩家不该能穿过去 —— 上一版这里整段楼身都没登记碰撞盒。 */
      block('qilou-back', bx + s * 1.0, 2.18, z, 2.8, 4.35, 7.4, body);
      /* 上层楼身：骑在券廊上。 */
      box(bx, (4.35 + h) / 2, z, 4.8, h - 4.35, 7.4, body);
      /* 券柱廊：三根柱 + 额枋。柱立在**临街立面内侧 0.25m**（`bx - s*2.15`，
       * 立面在 `bx - s*2.4`）—— 落在券廊里、顶着上层的挑出。 */
      for (const dz of [-2.6, 0, 2.6]) block('qilou-column', bx - s * 2.15, 2.05, z + dz, .46, 4.1, .46, M.vermilion);
      box(bx - s * 2.15, 4.42, z, .78, .6, 7.4, M.vermilion);
      /* 上层窗 + 木栏杆（开在临街立面）。 */
      for (let c = 0; c < 2; c++) {
        box(face - s * .06, 6.9, z - 1.9 + c * 3.8, .1, 2.2, 1.7, M.litWarm);
        box(face - s * .18, 5.5, z - 1.9 + c * 3.8, .12, .5, 2.0, M.timber);
      }
      /* **背面那一立面也要开窗。** 西排的背面正对中央巷道与欧式广场（只隔 3.2m），
       * 东排的背面对着 x=150 的边界墙外侧。只做临街一面的话，从广场看过去
       * 中华街就是一大片纯色墙 —— 取证帧 G2 里左边那块就是它。
       * 背面是"后墙"，不做券廊也不做店招，只补窗 + 一条贯通腰线。 */
      const back = bx + s * 2.4;
      for (let c = 0; c < 2; c++) box(back + s * .06, 6.9, z - 1.9 + c * 3.8, .1, 2.2, 1.7, M.litWarm);
      box(bx, 4.45, z, 4.9, .35, 7.5, M.tile);
      /* 瓦檐 + 女儿墙（临街一侧出檐）。檐口外端要**压低**：绕 z 转 θ>0 抬起局部
       * +x 端，所以西排（s=-1，出檐朝 +x）取 θ<0、东排（s=+1，出檐朝 -x）取 θ>0
       * ⇒ 正好是 `s*.3`。（牌坊的翘角相反，那里就是要外端翘起来。） */
      box(bx, h + .3, z, 5.6, .5, 8.2, M.tile);
      const eave = box(bx - s * 3.1, h + .75, z, 1.5, .3, 8.2, M.tile); eave.rotation.z = s * .3;
      /* 檐下一串红灯笼。 */
      for (const dz of [-2.4, 0, 2.4]) {
        rod([face - s * .4, h + .3, z + dz], [face - s * .4, h - .1, z + dz], .025, M.gold);
        const lan = new THREE.Mesh(ball, M.litRed); lan.scale.set(.36, .414, .36);
        lan.position.set(face - s * .4, h - .55, z + dz); root.add(lan);
      }
      /* 店招（朝街心）。东排的序号 +2 —— 否则同一段 z 上两侧挂的是同一块招牌，
       * 从街心正对着看就是"春和茶楼"照镜子。 */
      const names: [string, string][] = [['春和茶楼', 'TEA  HOUSE'], ['广济堂', 'HERBAL  HALL'], ['福记点心', 'DIM  SUM'], ['同仁书店', 'BOOKS']];
      const [t, sub] = names[(i + (s > 0 ? 2 : 0)) % names.length];
      board(t, sub, face - s * .12, 3.4, z, 3.2, 1.1, 'cn', s > 0 ? 'w' : 'e');
    }
  }

  /* 跨街灯笼串：从西骑楼拉到东骑楼，把 13m 宽的街面连起来 —— 中华街最认得出
   * 的一个读数。三串，落在两座牌坊之间。 */
  for (const z of [129, 141, 153]) {
    rod([131.72, 7.6, z], [144.72, 7.6, z], .022, M.gold);
    for (const lx of [133.5, 136, 138.5, 141, 143.5]) {
      const lan = new THREE.Mesh(ball, M.litRed); lan.scale.set(.34, .4, .34);
      lan.position.set(lx, 7.05, z); root.add(lan);
    }
  }

  /* 街心：石灯柱 + 长凳（中式收边）。 */
  for (const z of [133, 141, 149]) {
    block('cn-lamp', CX, 2.2, z, .34, 4.4, .34, M.timber);
    box(CX, 4.5, z, .95, .8, .95, M.litRed);
    block('cn-bench', CX - 4.2, .42, z, 1.1, .84, 2.2, M.timber);
    block('cn-bench', CX + 4.2, .42, z, 1.1, .84, 2.2, M.timber);
  }
  /* 入口铭牌：立在中华街北口轴线上的两根柱子之间。 */
  for (const dx of [-3.1, 3.1]) box(CX + dx, 1.7, 117.9, .26, 3.4, .26, M.vermilion);
  board('世界广场', 'WORLD  SQUARE  ·  LUMINA  EAST', CX, 3.8, 117.75, 6.4, 1.5, 'cn', 's');

  mergeByMaterial(root);

  /* 广场与街心的两盏点光。**数量克制：场景点光池是共享的**（sakuraTown 已占 6 盏，
   * civicQuarter 占 2 盏）。内透的主体是 emissive，点光只负责让地面有一小片暖色。 */
  const lamps = [new THREE.PointLight('#ffd39c', 2, 26, 2), new THREE.PointLight('#ff9a72', 2, 24, 2)];
  lamps[0].position.set(EX, 6, EZ); lamps[1].position.set(CX, 5.5, 141);
  for (const l of lamps) root.add(l);

  /**
   * 昼夜。三支自发光材质，三档（夜 / 黄昏 / 昼）。
   * 白天不压到 0 —— 否则窗和灯笼在晴天会变成死板的深色块。
   */
  function applyPeriod(period: string) {
    const night = period === 'night', dusk = period === 'dusk';
    M.litWarm.emissiveIntensity = night ? 1.05 : dusk ? .5 : .04;
    M.litRed.emissiveIntensity = night ? .95 : dusk ? .45 : .05;
    M.litCool.emissiveIntensity = night ? 1.1 : dusk ? .5 : .03;
    for (const l of lamps) l.intensity = night ? 11 : dusk ? 7 : 2;
  }
  applyPeriod('day');

  return {
    colliders,
    setEnvironment(period: string) { applyPeriod(period); },
    dispose() {
      root.removeFromParent();
      root.traverse(o => { if (o instanceof THREE.Mesh) geometries.add(o.geometry); if (o instanceof THREE.InstancedMesh) o.dispose(); });
      for (const g of geometries) g.dispose();
      for (const m of materials) m.dispose();
      for (const t of textures) t.dispose();
    },
  };
}
