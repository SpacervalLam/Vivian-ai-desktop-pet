import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const ui = 'src-tauri/plugins/3d-apartment/ui/room.js';
if (!existsSync(ui) || statSync(ui).size < 100_000) {
  throw new Error('3D 公寓插件脚本缺失或为空');
}
if (!readFileSync(ui, 'utf8').includes('VivianApartment')) {
  throw new Error('3D 公寓插件入口未导出');
}
if (existsSync('dist/room')) {
  throw new Error('公寓模型资源仍被复制进主前端包');
}
for (const file of readdirSync('dist/assets')) {
  if (!file.endsWith('.js')) continue;
  const content = readFileSync(join('dist/assets', file), 'utf8');
  if (content.includes('apartment-twin-shell') || content.includes('apartment-residential-podium')) {
    throw new Error(`公寓场景代码仍在主前端包中：${file}`);
  }
}
console.log('[apartment-plugin] 场景脚本与模型已从主前端包外置');
