import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

function run(command, args) {
  const result = spawnSync(command, args, { stdio: 'inherit', shell: process.platform === 'win32' });
  if (result.status !== 0) process.exit(result.status || 1);
}
run('npm', ['run', 'build:apartment']);
run('npm', ['run', 'tauri:build', '--', '--bundles', 'nsis']);
mkdirSync('release', { recursive: true });
const folder = 'src-tauri/target/release/bundle/nsis';
const setup = readdirSync(folder).filter(name => name.endsWith('-setup.exe')).sort((a,b) => statSync(`${folder}/${b}`).mtimeMs - statSync(`${folder}/${a}`).mtimeMs)[0];
if (!setup) throw new Error('NSIS installer missing');
const slimName = setup.replace('-setup.exe', '-small-setup.exe');
copyFileSync(`${folder}/${setup}`, `release/${slimName}`);
const manifest = JSON.parse(readFileSync('plugins/3d-apartment/plugin.json', 'utf8'));
const names = [slimName, `Vivian-3D-Apartment-${manifest.version}.zip`];
const artifacts = names.map(name => ({ name, bytes: statSync(`release/${name}`).size, sha256: createHash('sha256').update(readFileSync(`release/${name}`)).digest('hex') }));
writeFileSync('release/checksums.json', JSON.stringify(artifacts, null, 2)+'\n');
writeFileSync('release/安装说明.txt', '基础安装：运行 small-setup.exe。\r\n安装公寓：把公寓 ZIP 与安装程序放在同一目录，运行 setup 并勾选“安装 3D 公寓插件”。请勿改 ZIP 文件名。\r\n安装后可在设置 → 通用启用或禁用。\r\n静默安装默认不安装公寓；/APARTMENT 安装旁边的公寓包，/NOAPARTMENT 不安装。\r\n');
console.log(JSON.stringify(artifacts, null, 2));
