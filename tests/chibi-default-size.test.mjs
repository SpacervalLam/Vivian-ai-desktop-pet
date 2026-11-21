/**
 * 四个桌宠「默认高度归一」的回归测试。
 *
 * 校验三件事：
 *   1. defaultSize.json 里每个角色的 scale / translateY 确实由当前 public/ 素材
 *      实测得出，且投影后的 top / foot 全部落在同一个目标上（素材被改过就会失败）。
 *   2. defaultSize.ts 导出的 defaultSizeStyle() 真的把配置变成 CSS 的
 *      scale / translate，未收录的角色回退到恒等变换。
 *   3. 渲染层确实存在 .chibi-pet-default-size 包裹层，并且夹在 tap-feedback
 *      与 .chibi-pet-sprite 之间（否则逐帧 frameScales / frameAnchors 会被覆盖）。
 */
import assert from 'node:assert/strict';
import sharp from 'sharp';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';
import { bounds } from '../scripts/chibi/frame-sheets/registration.mjs';
import { assertBottomDockFrame, assertSpriteSeam } from './helpers/character-assets.mjs';

const readJson = async path => JSON.parse(await readFile(path, 'utf8'));
const vocab = await readJson('src/chibi/animations.json');
const config = await readJson('src/chibi/defaultSize.json');

const CELL = 512;
const ORIGIN = config.originPercent / 100 * CELL;   // 90% -> 460.8
const PERCENT_PX = CELL / 100;                      // translate 的 1% = 5.12 格内像素
const idleUrl = id => id === 'nana'
  ? `public/chibi/${id}-atlas.webp`                 // nana 用 atlas slot 0
  : `public/chibi/motion/${id}/${id}-idle-sheet.webp`;

// ---- 1. 配置覆盖全部角色，且 origin 与渲染层一致 -----------------------------
assert.equal(config.originPercent, 90, 'defaultSize.json: originPercent 必须为 90');
assert.deepEqual(
  Object.keys(config.characters).sort(),
  [...vocab.characters].sort(),
  'defaultSize.json: characters 必须覆盖 animations.json 里的每个角色',
);
assert.ok(config.target.foot > config.target.top, 'target: foot 必须低于 top');
assert.ok(
  Math.abs(config.target.foot - config.target.top - config.target.height) <= 0.01,
  'target: height 必须等于 foot - top',
);

// ---- 2. 用真实素材独立复算，检查每个角色的归一投影 ----------------------------
const measuredHeights = [];
const renderedHeights = [];
for (const id of vocab.characters) {
  const pixels = await sharp(idleUrl(id))
    .extract({ left: 0, top: 0, width: CELL, height: CELL })
    .ensureAlpha().raw().toBuffer();
  const box = bounds(pixels, CELL, CELL);
  const height = box.y1 - box.y0;           // 坐标跨度，不是 bounds().height（+1 像素计数）

  // 素材没变：config.measured 记录的实测值必须仍然成立。
  assert.equal(config.measured[id].top, box.y0, `${id}: measured.top 与素材不符，需重跑 calibrate:chibi-default-size`);
  assert.equal(config.measured[id].foot, box.y1, `${id}: measured.foot 与素材不符，需重跑 calibrate:chibi-default-size`);
  assert.equal(config.measured[id].height, height, `${id}: measured.height 与素材不符`);

  // 由目标独立复算期望的变换，避免配置被手改。
  const expectedScale = config.target.height / height;
  const expectedTranslateY =
    (config.target.foot - ORIGIN - expectedScale * (box.y1 - ORIGIN)) / PERCENT_PX;
  const t = config.characters[id];
  assert.ok(
    Math.abs(t.scale - expectedScale) < 1e-5,
    `${id}: scale ${t.scale} 应为 ${expectedScale}`,
  );
  assert.ok(
    Math.abs(t.translateY - expectedTranslateY) < 1e-5,
    `${id}: translateY ${t.translateY} 应为 ${expectedTranslateY}`,
  );

  // CSS 组合矩阵 p' = O + T + S*(p - O)，验证头顶与脚底都落到目标。
  const project = y => ORIGIN + t.translateY * PERCENT_PX + t.scale * (y - ORIGIN);
  const top = project(box.y0);
  const foot = project(box.y1);
  assert.ok(Math.abs(top - config.target.top) <= 0.5, `${id}: 归一后头顶 ${top} 未对齐 ${config.target.top}`);
  assert.ok(Math.abs(foot - config.target.foot) <= 0.5, `${id}: 归一后脚底 ${foot} 未对齐 ${config.target.foot}`);
  assert.ok(Math.abs(config.measured[id].projected.top - top) <= 0.01, `${id}: projected.top 记录错误`);
  assert.ok(Math.abs(config.measured[id].projected.foot - foot) <= 0.01, `${id}: projected.foot 记录错误`);

  measuredHeights.push(height);
  renderedHeights.push(foot - top);
}

// The original display size stays fixed when optional characters are installed or removed.
assert.equal(config.target.height, 439.25, 'installing character packages must not resize the originals');

// 归一后四个角色的屏幕高度必须一致（允许 0.5 格内像素的舍入误差）。
const spread = Math.max(...renderedHeights) - Math.min(...renderedHeights);
assert.ok(spread <= 0.5, `四个默认高度未统一，最大差 ${spread.toFixed(3)} 格内像素`);

// 角色包的独立待机图和图集回退都必须踩在重力碰撞使用的脚底线上。
for (const id of ['claude', 'deepseek', 'chatgpt']) {
  const root = `character-packs/${id}`;
  const { presentation: { defaultSize: { scale, translateY } } } = await readJson(`${root}/character.json`);
  for (const file of [`${id}-atlas.webp`, `motion/${id}/${id}-idle-sheet.webp`]) {
    const pixels = await sharp(`${root}/assets/chibi/${file}`)
      .extract({ left: 0, top: 0, width: CELL, height: CELL })
      .ensureAlpha().raw().toBuffer();
    const { y1 } = bounds(pixels, CELL, CELL);
    const foot = ORIGIN + translateY * PERCENT_PX + scale * (y1 - ORIGIN);
    assert.ok(Math.abs(foot - config.target.foot) <= 0.5, `${id}/${file}: 脚底未对齐 ${config.target.foot}`);
  }
}

// ---- 3. defaultSize.ts 真的把配置转成 CSS 属性 ------------------------------
const bundle = await build({
  entryPoints: ['src/chibi/defaultSize.ts'],
  bundle: true, write: false, platform: 'node', format: 'esm',
});
const module = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
assert.equal(module.DEFAULT_SIZE_ORIGIN_PERCENT, 90, 'defaultSize.ts: origin 导出错误');
for (const id of vocab.characters) {
  const t = config.characters[id];
  assert.deepEqual(
    module.defaultSizeStyle(id),
    { scale: String(t.scale), translate: `0% ${t.translateY}%` },
    `${id}: defaultSizeStyle() 输出错误`,
  );
  assert.deepEqual(module.defaultSizeTransform(id), t, `${id}: defaultSizeTransform() 输出错误`);
}
assert.deepEqual(
  module.defaultSizeStyle('unknown-character'),
  { scale: '1', translate: '0% 0%' },
  '未收录角色必须回退到恒等变换',
);

// ---- 3b. 底边贴边校准：两只手之间的脸部贴住下沿，保留 idle 尺寸 --------
const dockConfig = await readJson('src/chibi/bottomDock.json');
const dockBundle = await build({
  entryPoints: ['src/chibi/dockSize.ts'],
  bundle: true, write: false, platform: 'node', format: 'esm', loader: { '.json': 'json' },
});
const dockModule = await import(`data:text/javascript;base64,${Buffer.from(dockBundle.outputFiles[0].text).toString('base64')}`);
assert.deepEqual(
  Object.keys(dockConfig.characters).sort(), [...vocab.characters].sort(),
  'bottomDock.json: characters 必须覆盖 animations.json 里的每个角色',
);
const dockedBottoms = [];
const dockMotion = vocab.animations.find(({ name }) => name === 'edge-bottom');
const holdFrame = dockMotion.frames - 1;
const frameLeft = holdFrame % dockMotion.cols * CELL;
const frameTop = Math.floor(holdFrame / dockMotion.cols) * CELL;
for (const id of vocab.characters) {
  // 独立量测 edge-bottom 的保持帧并复算贴边投影。
  const sheet = sharp(`public/chibi/motion/${id}/${id}-edge-bottom-sheet.webp`);
  const idle = await sharp(idleUrl(id)).extract({ left: 0, top: 0, width: CELL, height: CELL }).ensureAlpha().raw().toBuffer();
  const first = await sheet.clone().extract({ left: 0, top: 0, width: CELL, height: CELL }).ensureAlpha().raw().toBuffer();
  assertSpriteSeam(first, idle, `${id}: idle → bottom dock`);
  const pixels = await sheet.clone()
    .extract({ left: frameLeft, top: frameTop, width: CELL, height: CELL })
    .ensureAlpha().raw().toBuffer();
  const box = bounds(pixels, CELL, CELL);
  let previousHeight = bounds(first, CELL, CELL).height;
  for (let frame = 1; frame <= holdFrame; frame++) {
    const drawing = await sheet.clone().extract({
      left: frame % dockMotion.cols * CELL,
      top: Math.floor(frame / dockMotion.cols) * CELL,
      width: CELL,
      height: CELL,
    }).ensureAlpha().raw().toBuffer();
    previousHeight = assertBottomDockFrame(drawing, idle, previousHeight, `${id}/${frame + 1}`);
  }
  const faceWidth = 32;
  const faceStrip = await sheet.clone()
    .extract({ left: frameLeft + (CELL - faceWidth) / 2, top: frameTop, width: faceWidth, height: CELL })
    .ensureAlpha().raw().toBuffer();
  const faceBottom = Array.from({ length: CELL }, (_, y) => {
    const rowStart = y * faceWidth * 4;
    const row = faceStrip.subarray(rowStart, rowStart + faceWidth * 4);
    return row.filter((alpha, index) => index % 4 === 3 && alpha > 200).length;
  }).findLastIndex(opaque => opaque >= faceWidth * 0.75);
  assert.ok(faceBottom >= 0 && box.y1 - faceBottom <= 20,
    `${id}: 脸部和双手应位于同一条裁切线附近`);
  const style = dockModule.bottomDockStyle(id);
  const scale = Number(style.scale);
  const offsetPx = Number(style.translate.split(' ')[1].replace('%', '')) * PERCENT_PX;
  const projectedBottom = ORIGIN + offsetPx + scale * (faceBottom - ORIGIN);
  assert.ok(projectedBottom >= CELL / 2 + 2, `${id}: 两手之间的脸部应覆盖屏幕下沿`);
  dockedBottoms.push(projectedBottom);
  // 配置记录与独立复算一致（素材或 defaultSize 改过就会失败）。
  const entry = dockConfig.characters[id];
  const baseTransform = config.characters[id];
  assert.equal(entry.scale, 1, `${id}: 进入贴边时应保留 idle 尺寸`);
  assert.ok(Math.abs(scale - baseTransform.scale) < 1e-5);
  assert.equal(dockModule.BOTTOM_DOCK_ORIGIN_PERCENT, config.originPercent, '贴边校准必须与默认尺寸共用原点');
}
const bottomSpread = Math.max(...dockedBottoms) - Math.min(...dockedBottoms);
assert.ok(bottomSpread <= 0.5, `贴边接触线未对齐，最大差 ${bottomSpread.toFixed(3)} 格内像素`);
assert.ok(Math.abs(dockedBottoms[0] - dockConfig.target.bottom) <= 0.5, '贴边接触线偏离实测目标');
assert.deepEqual(
  dockModule.bottomDockStyle('unknown-character'),
  { scale: '1', translate: '0% 0%' },
  '未收录角色的贴边校准必须回退到恒等变换',
);

// ---- 4. 渲染层接线：包裹层在 tap-feedback 与 sprite 之间 ---------------------
const css = await readFile('src/components/ChibiPetCanvas.css', 'utf8');
const cssRule = css.match(/\.chibi-pet-default-size\s*\{([^}]*)\}/);
assert.ok(cssRule, 'CSS 缺少 .chibi-pet-default-size 规则');
assert.match(cssRule[1], /position:\s*absolute/, '包裹层必须绝对定位，保持与精灵同一坐标系');
assert.match(cssRule[1], /inset:\s*0/, '包裹层必须铺满舞台');
assert.match(cssRule[1], /transform-origin:\s*50%\s*90%/, '包裹层 transform-origin 必须与配置一致');

const tsx = await readFile('src/components/ChibiPetCanvas.tsx', 'utf8');
const tap = tsx.indexOf('className="chibi-pet-tap-feedback"');
const wrapper = tsx.indexOf('className="chibi-pet-default-size"');
const sprite = tsx.indexOf('className="chibi-pet-sprite"');
assert.ok(tap > 0 && wrapper > tap && sprite > wrapper,
  'DOM 层级必须是 tap-feedback > default-size > sprite');
assert.match(tsx, /style=\{bottomDocked \? bottomDockStyle\(characterId\) : defaultSizeStyle\(characterId\)\}/,
  '包裹层必须按贴边状态注入 bottomDockStyle / defaultSizeStyle');
assert.ok(tsx.includes("import { defaultSizeStyle } from '../chibi/defaultSize'"),
  'ChibiPetCanvas.tsx 必须导入 defaultSizeStyle');
assert.ok(tsx.includes("import { bottomDockStyle } from '../chibi/dockSize'"),
  'ChibiPetCanvas.tsx 必须导入 bottomDockStyle');

console.log(`Default size: ${vocab.characters.length} 个角色统一到 top=${config.target.top} / foot=${config.target.foot} / 高=${config.target.height}（512 格内），最大差 ${spread.toFixed(3)}px；底边贴边保留 idle 头部图和尺寸，接触线最大差 ${bottomSpread.toFixed(3)}px；CSS 组合矩阵与渲染层接线已验证。`);
