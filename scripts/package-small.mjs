import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

function run(command, args) {
  const result = spawnSync(command, args, { stdio: 'inherit', shell: process.platform === 'win32' });
  if (result.status !== 0) process.exit(result.status || 1);
}
const collectOnly = process.argv.includes('--collect-only');
if (!collectOnly) {
  run('npm', ['run', 'build:apartment']);
  run('npm', ['run', 'tauri:build', '--', '--bundles', 'nsis']);
}
mkdirSync('release', { recursive: true });
const folder = 'src-tauri/target/release/bundle/nsis';
const setup = readdirSync(folder).filter(name => name.endsWith('-setup.exe')).sort((a,b) => statSync(`${folder}/${b}`).mtimeMs - statSync(`${folder}/${a}`).mtimeMs)[0];
if (!setup) throw new Error('NSIS installer missing');
const slimName = setup.replace('-setup.exe', '-small-setup.exe');
copyFileSync(`${folder}/${setup}`, `release/${slimName}`);
const manifest = JSON.parse(readFileSync('plugins/3d-apartment/plugin.json', 'utf8'));
const resourcePacks = JSON.parse(readFileSync('release/resource-packs.json', 'utf8'));
if (collectOnly) {
  const inputs = ['dist/index.html', 'src-tauri/windows/resource-packages.nsh', ...resourcePacks.map(pack => `release/${pack.filename}`)];
  if (inputs.some(path => statSync(path).mtimeMs > statSync(`${folder}/${setup}`).mtimeMs)) throw new Error('Installer is older than current resources; run tauri build/bundle first');
}
const names = [slimName, `Vivian-3D-Apartment-${manifest.version}.zip`, ...resourcePacks.map(pack => pack.filename)];
const artifacts = names.map(name => ({ name, bytes: statSync(`release/${name}`).size, sha256: createHash('sha256').update(readFileSync(`release/${name}`)).digest('hex') }));
writeFileSync('release/checksums.json', JSON.stringify(artifacts, null, 2)+'\n');
writeFileSync('release/安装说明.txt', '基础安装：运行 small-setup.exe。\r\n可选组件：将需要的配套 ZIP 与安装程序放在同一目录，勾选 3D 公寓、手写字体或内置贴纸。请勿改 ZIP 文件名。\r\n资源包：' + resourcePacks.map(pack => pack.label + '：' + pack.filename).join('；') + '。\r\n安装后重启应用，可在设置 → 通用查看资源包状态。未安装字体时使用系统字体；自定义贴纸不依赖内置贴纸包。\r\n未勾选的字体和贴纸组件不会移除已安装版本；卸载程序时会移除这些可选资源。\r\n静默安装默认只安装基础程序；/APARTMENT、/FONTS、/STICKERS 分别添加旁边对应的配套 ZIP；/NOAPARTMENT、/NOFONTS、/NOSTICKERS 显式跳过。\r\n');
console.log(JSON.stringify(artifacts, null, 2));
