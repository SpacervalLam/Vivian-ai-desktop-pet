/**
 * 把角色包的动作节奏对齐到它当初照着的节拍上。
 *
 * 背景：角色包自己的 `durations` 是逐帧毫秒数。素材重画/去重时帧数会变（16 帧去重成
 * 12 帧、24 帧压成 12 帧），但时长数组常常仍按老帧数填写——少掉的那些帧**带走的时间**
 * 没人补回来，整段就变快了：聊天版 chatgpt 的眨眼从 414ms 压到 281ms，入睡从 3550ms
 * 压到 1540ms。表现就是「动作快得像抽帧」，短到眨眼看不清。
 *
 * 判据：以 `animations.legacy-v1.json`（角色包素材当初照着画的冻结契约）为节拍基准。
 * 只修**过快**的段：总时长低于基准 85% 才按比例拉长，绝不放慢本来更慢的段——慢是
 * 有意的表演节奏，快才是漏算。
 *
 * 用法：
 *   node scripts/chibi/rescale-pack-beats.mjs            # 全部角色包，写回
 *   node scripts/chibi/rescale-pack-beats.mjs --dry      # 只出报告
 *   node scripts/chibi/rescale-pack-beats.mjs chatgpt    # 单个包
 */
import { readFile, writeFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');
const args = process.argv.slice(2);
const dryRun = args.includes('--dry');
const requested = args.filter(argument => !argument.startsWith('--'));

/** 低于基准的这个比例就算「过快」，需要拉长。 */
const TOO_FAST_RATIO = 0.85;
/** 判定阈值：拉长比例小于它就不动，避免为 3% 的差异重写整个包。 */
const MIN_RESIZE = 0.03;

const contract = JSON.parse(await readFile(resolve(root, 'src/chibi/animations.legacy-v1.json'), 'utf8'));
const beat = new Map(contract.animations.map(spec => [spec.name, spec.durations.reduce((sum, value) => sum + value, 0)]));

/** 只处理真正有清单的目录：正在做的角色包可能只有 prompts。 */
async function packIds() {
  const entries = await readdir(resolve(root, 'character-packs'), { withFileTypes: true });
  const ids = [];
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    try {
      await readFile(resolve(root, 'character-packs', entry.name, 'character.json'));
      ids.push(entry.name);
    } catch { /* 还没有打包的角色目录，跳过 */ }
  }
  return ids.sort();
}
const packs = requested.length > 0 ? requested : await packIds();

const round = value => Math.max(1, Math.round(value));
const report = [];

for (const id of packs) {
  const path = resolve(root, 'character-packs', id, 'character.json');
  const manifest = JSON.parse(await readFile(path, 'utf8'));
  if (!manifest.animations) continue;
  let changed = 0;
  for (const [name, spec] of Object.entries(manifest.animations)) {
    const target = beat.get(name);
    if (!target) continue;
    const durations = spec.durations;
    const total = durations.reduce((sum, value) => sum + value, 0);
    const ratio = total / target;
    if (ratio >= TOO_FAST_RATIO) {
      report.push({ id, name, frames: durations.length, from: total, to: target, verdict: 'ok' });
      continue;
    }
    if (target / total - 1 < MIN_RESIZE) continue;
    // 逐帧按同一比例拉长：保留原来的缓入缓出（匀速改写会毁掉眨眼/起身的节奏）。
    const scale = target / total;
    const scaled = durations.map(value => round(value * scale));
    // 取整会把总时长带偏几毫秒：差额补在最后一帧上，保证正好落在基准节拍。
    const drift = target - scaled.reduce((sum, value) => sum + value, 0);
    scaled[scaled.length - 1] = Math.max(1, scaled[scaled.length - 1] + drift);
    spec.durations = scaled;
    changed += 1;
    report.push({ id, name, frames: durations.length, from: total, to: scaled.reduce((sum, value) => sum + value, 0), verdict: 'rescaled' });
  }
  if (changed > 0 && !dryRun) await writeFile(path, JSON.stringify(manifest, null, 2) + '\n');
}

console.log('\npack       motion        frames   was      now      verdict');
for (const row of report) {
  console.log(`${row.id.padEnd(11)}${row.name.padEnd(14)}${String(row.frames).padEnd(9)}${String(row.from + 'ms').padEnd(9)}${String(row.to + 'ms').padEnd(9)}${row.verdict}`);
}
const rescaled = report.filter(row => row.verdict === 'rescaled').length;
console.log(`\n${rescaled} animation(s) ${dryRun ? 'would be' : ''} rescaled to the authored beat.`);
if (dryRun) console.log('Dry run — nothing written.');
