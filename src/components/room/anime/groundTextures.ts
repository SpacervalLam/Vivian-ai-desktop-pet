import * as THREE from 'three';

/**
 * 地面贴图。
 *
 * 为什么单独开一个文件：地面是场景里**面积最大**的一类材质 —— `civicQuarter` 那块
 * 152×80 的 `district-ground` 一张就顶掉半张俯视图，而它是**纯色**的。这个错误
 * 在 cityDressing（paving / road / green）、civicQuarter（grass / road / stone）、
 * globalSquare 里各犯了一次，各自 `new MeshStandardMaterial({color})` 了事。
 * 抽出来的理由和 `foliage.ts` 当初一样：不共享就必然各长各的，然后同一片街区
 * 出现三种草地。
 *
 * 三条约束：
 *
 *  1. **贴图只出明暗，颜色由调用方材质色相乘决定**。一张草地图同时服务市政草地、
 *     河堤草坡、公寓草坪三种绿；换成彩色贴图就得每种绿出一张。值域压在 0.55~1.0：
 *     `map` 与 `color` 是**相乘**，低于 0.55 会把彩色地面压成脏抹布。
 *  2. **必须无缝平铺**。地面 box 的 UV 是每面 0..1，repeat 由调用方按尺寸给；
 *     只要有一处接缝，一格一格的网格比纯色更扎眼。所以跨边界的笔触要在对侧
 *     补画一份（见 `stamp`）。
 *  3. **细节分三层**：细噪点（近景）+ 中等笔触（中景）+ 大块明暗（远景）。
 *     只有一层的话，走近了是塑料板、走远了又变回纯色 —— 纯色之所以"潦草"，
 *     缺的从来不是颜色，是尺度层次。
 */

export type GroundKind = 'grass' | 'paving' | 'asphalt' | 'soil' | 'gravel';

/** 确定性 PRNG：贴图每次生成都一致，否则改一次代码整片地面重排一遍。 */
function prng(seed: number) {
  let s = seed >>> 0;
  return () => { s = (Math.imul(s, 1664525) + 1013904223) >>> 0; return s / 4294967296; };
}

/**
 * 在平铺纹理上落一笔，跨界时在对侧补画。
 *
 * 只画一遍的话，平铺后每个 tile 边界都会出现"画到一半被切断"的元素 —— 那就是
 * 肉眼可见的接缝。`size` 是纹理边长，`r` 是笔触半径，超出 [−r, size+r] 的副本
 * 不可能与画布相交，跳过。
 */
function stamp(
  ctx: CanvasRenderingContext2D, size: number, x: number, y: number, r: number,
  draw: (px: number, py: number) => void,
) {
  for (const dx of [-size, 0, size]) {
    for (const dy of [-size, 0, size]) {
      const px = x + dx, py = y + dy;
      if (px < -r || px > size + r || py < -r || py > size + r) continue;
      draw(px, py);
    }
  }
}

const SIZE = 512;

/* ---------------------------------------------------------------- 草地 */

function grassCanvas(): HTMLCanvasElement {
  const cv = document.createElement('canvas');
  cv.width = cv.height = SIZE;
  const c = cv.getContext('2d')!;
  const rnd = prng(20260928);

  c.fillStyle = '#e8e6da'; c.fillRect(0, 0, SIZE, SIZE);

  // 第三层：大块明暗（远景）。几十个大椭圆，压得很淡 —— 它的作用只是让地面
  // 在 100m 外不是一整片均匀色，重了会读作"污渍"。
  for (let i = 0; i < 26; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, r = 60 + rnd() * 150;
    const up = rnd() > .45;
    stamp(c, SIZE, x, y, r, (px, py) => {
      c.beginPath(); c.ellipse(px, py, r, r * (.55 + rnd() * .5), rnd() * 3.14, 0, Math.PI * 2);
      c.fillStyle = up ? 'rgba(255,255,255,.055)' : 'rgba(40,54,36,.05)';
      c.fill();
    });
  }
  // 第二层：草丛短线（中景）。朝向随机、长短不一，是"这是草不是地毯"的关键。
  for (let i = 0; i < 2600; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE;
    const len = 3.4 + rnd() * 6.2, a = rnd() * Math.PI;
    const dark = 0.10 + rnd() * 0.20;
    stamp(c, SIZE, x, y, len, (px, py) => {
      c.strokeStyle = rnd() > .42
        ? `rgba(46,62,40,${dark})`
        : `rgba(255,255,248,${dark * .55})`;
      c.lineWidth = .8 + rnd() * .9;
      c.beginPath();
      c.moveTo(px - Math.cos(a) * len * .5, py - Math.sin(a) * len * .5);
      c.lineTo(px + Math.cos(a) * len * .5, py + Math.sin(a) * len * .5);
      c.stroke();
    });
  }
  // 第一层：细噪点（近景）。没有它，贴到镜头前就是一块抹了色的板子。
  for (let i = 0; i < 5200; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, v = rnd();
    stamp(c, SIZE, x, y, 2, (px, py) => {
      c.fillStyle = v > .5
        ? `rgba(255,255,246,${(v - .5) * .34})`
        : `rgba(38,50,32,${(.5 - v) * .40})`;
      c.fillRect(px, py, 1.5, 1.5);
    });
  }
  return cv;
}

/* ---------------------------------------------------------------- 铺装 */

function pavingCanvas(): HTMLCanvasElement {
  const cv = document.createElement('canvas');
  cv.width = cv.height = SIZE;
  const c = cv.getContext('2d')!;
  const rnd = prng(77123);

  c.fillStyle = '#dcd8cc'; c.fillRect(0, 0, SIZE, SIZE);

  /* 砖缝。8×8 的砖：512/8 = 64px 一块，按 4m 一个 tile 算就是 0.5m 见方的人行道砖。
   * 每块砖单独给一点色差 —— 全部同色的砖等于没贴纹理，只是多了一堆线。 */
  const N = 8, cell = SIZE / N;
  for (let iy = 0; iy < N; iy++) {
    for (let ix = 0; ix < N; ix++) {
      // 错缝：奇数行横移半块，读作工字铺而不是棋盘格
      const off = (iy % 2) * cell * .5;
      const v = rnd();
      c.fillStyle = v > .5
        ? `rgba(255,255,250,${(v - .5) * .40})`
        : `rgba(58,54,46,${(.5 - v) * .30})`;
      c.fillRect(ix * cell - off, iy * cell, cell, cell);
    }
  }
  // 缝：比砖面暗一档，宽度压到 1.5px —— 宽了就从"缝"变成"网格线"
  c.strokeStyle = 'rgba(52,48,42,.34)'; c.lineWidth = 1.5;
  for (let i = 0; i <= N; i++) {
    c.beginPath(); c.moveTo(0, i * cell); c.lineTo(SIZE, i * cell); c.stroke();
  }
  for (let iy = 0; iy < N; iy++) {
    const off = (iy % 2) * cell * .5;
    for (let ix = 0; ix <= N; ix++) {
      c.beginPath(); c.moveTo(ix * cell - off, iy * cell); c.lineTo(ix * cell - off, (iy + 1) * cell); c.stroke();
    }
  }
  // 磨损与脏污：边缘磨圆、零星深色斑。全新铺装比纯色还假。
  for (let i = 0; i < 900; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, r = 1 + rnd() * 7;
    stamp(c, SIZE, x, y, r, (px, py) => {
      c.beginPath(); c.arc(px, py, r, 0, Math.PI * 2);
      c.fillStyle = `rgba(44,40,34,${.03 + rnd() * .10})`;
      c.fill();
    });
  }
  for (let i = 0; i < 2600; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, v = rnd();
    stamp(c, SIZE, x, y, 2, (px, py) => {
      c.fillStyle = v > .5 ? `rgba(255,253,244,${(v - .5) * .30})` : `rgba(50,46,40,${(.5 - v) * .26})`;
      c.fillRect(px, py, 1.4, 1.4);
    });
  }
  return cv;
}

/* ---------------------------------------------------------------- 沥青 */

function asphaltCanvas(): HTMLCanvasElement {
  const cv = document.createElement('canvas');
  cv.width = cv.height = SIZE;
  const c = cv.getContext('2d')!;
  const rnd = prng(31337);

  c.fillStyle = '#cfcdc8'; c.fillRect(0, 0, SIZE, SIZE);

  // 大块补丁：沥青翻修过的地方颜色深浅不一，一块一块的
  for (let i = 0; i < 14; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, r = 40 + rnd() * 110;
    stamp(c, SIZE, x, y, r, (px, py) => {
      c.beginPath(); c.ellipse(px, py, r, r * (.5 + rnd() * .6), rnd() * 3.14, 0, Math.PI * 2);
      c.fillStyle = rnd() > .5 ? 'rgba(255,255,255,.07)' : 'rgba(30,30,32,.07)';
      c.fill();
    });
  }
  // 骨料颗粒：沥青的"砂粒感"全靠这一层
  for (let i = 0; i < 9000; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, v = rnd(), r = .7 + rnd() * 1.5;
    stamp(c, SIZE, x, y, r, (px, py) => {
      c.beginPath(); c.arc(px, py, r, 0, Math.PI * 2);
      c.fillStyle = v > .5 ? `rgba(255,255,250,${(v - .5) * .40})` : `rgba(26,26,30,${(.5 - v) * .44})`;
      c.fill();
    });
  }
  // 裂纹：细、短、折线。长了会读成路面裂缝贴图，短了才是"用过一段时间"
  for (let i = 0; i < 26; i++) {
    let x = rnd() * SIZE, y = rnd() * SIZE;
    c.strokeStyle = `rgba(34,34,38,${.16 + rnd() * .18})`;
    c.lineWidth = .8 + rnd() * .8;
    c.beginPath(); c.moveTo(x, y);
    for (let k = 0; k < 5; k++) { x += (rnd() - .5) * 42; y += (rnd() - .5) * 42; c.lineTo(x, y); }
    c.stroke();
  }
  return cv;
}

/* ---------------------------------------------------------------- 泥土 / 砾石 */

function soilCanvas(): HTMLCanvasElement {
  const cv = document.createElement('canvas');
  cv.width = cv.height = SIZE;
  const c = cv.getContext('2d')!;
  const rnd = prng(6081);

  c.fillStyle = '#dedad0'; c.fillRect(0, 0, SIZE, SIZE);
  for (let i = 0; i < 34; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, r = 24 + rnd() * 70;
    stamp(c, SIZE, x, y, r, (px, py) => {
      c.beginPath(); c.ellipse(px, py, r, r * (.6 + rnd() * .5), rnd() * 3.14, 0, Math.PI * 2);
      c.fillStyle = rnd() > .5 ? 'rgba(255,252,244,.06)' : 'rgba(56,44,32,.07)';
      c.fill();
    });
  }
  // 土块：小方块而不是圆点，土是块状剥落的
  for (let i = 0; i < 1500; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, s = 1.6 + rnd() * 4.4, v = rnd();
    stamp(c, SIZE, x, y, s, (px, py) => {
      c.fillStyle = v > .5 ? `rgba(255,250,240,${(v - .5) * .34})` : `rgba(48,38,28,${(.5 - v) * .38})`;
      c.fillRect(px, py, s, s * (.6 + rnd() * .7));
    });
  }
  for (let i = 0; i < 3000; i++) {
    const x = rnd() * SIZE, y = rnd() * SIZE, v = rnd();
    stamp(c, SIZE, x, y, 2, (px, py) => {
      c.fillStyle = v > .5 ? `rgba(255,252,246,${(v - .5) * .28})` : `rgba(44,34,26,${(.5 - v) * .30})`;
      c.fillRect(px, py, 1.4, 1.4);
    });
  }
  return cv;
}

/* ---------------------------------------------------------------- 出口 */

/** canvas 只画一次；每次取用都是一个新的 Texture（共享同一张 image）。 */
const cache = new Map<GroundKind, HTMLCanvasElement>();
function canvasFor(kind: GroundKind): HTMLCanvasElement {
  let cv = cache.get(kind);
  if (cv) return cv;
  cv = kind === 'grass' ? grassCanvas()
    : kind === 'paving' ? pavingCanvas()
      : kind === 'asphalt' ? asphaltCanvas()
        : soilCanvas();
  cache.set(kind, cv);
  return cv;
}

/**
 * 取一张地面贴图。
 *
 * `repeat` 是**这块地面上平铺多少个 tile**，由调用方按地面尺寸给
 * （`repeat = 边长 / 期望 tile 米数`）。不能直接改共享纹理的 repeat ——
 * 一张纹理被几块不同尺寸的地面共用时，后设的会把先设的覆盖掉。
 */
export function groundTexture(kind: GroundKind, repeat = 8): THREE.CanvasTexture {
  const tex = new THREE.CanvasTexture(canvasFor(kind));
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.wrapS = tex.wrapT = THREE.RepeatWrapping;
  tex.repeat.set(repeat, repeat);
  tex.anisotropy = 4;
  return tex;
}
