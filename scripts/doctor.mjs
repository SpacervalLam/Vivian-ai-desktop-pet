#!/usr/bin/env node
/**
 * Vivian 诊断工具 (doctor) —— 检查开发环境与运行时依赖的健康状态。
 *
 * 用法：node scripts/doctor.mjs
 *
 * 检查项：
 * 1. Rust 工具链（cargo / rustc）
 * 2. Node.js / npm
 * 3. Tauri CLI
 * 4. 前端依赖（node_modules）
 * 5. 配置文件（config.yaml）
 * 6. 应用数据目录（%APPDATA%\Vivian）
 * 7. Live2D 模型文件
 * 8. GPT-SoVITS 集成路径
 * 9. 日志目录
 * 10. 网络连通性
 */

import { execSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { homedir, platform } from 'node:os';

const ROOT = resolve(import.meta.dirname, '..');
const isWindows = platform() === 'win32';

const results = [];

function check(name, fn) {
  try {
    const detail = fn();
    results.push({ name, status: 'pass', detail });
  } catch (e) {
    results.push({ name, status: 'fail', detail: e.message });
  }
}

function run(cmd) {
  return execSync(cmd, { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] }).trim();
}

function getAppDataDir() {
  if (isWindows) {
    return join(process.env.APPDATA || '', 'Vivian');
  }
  if (platform() === 'darwin') {
    return join(homedir(), 'Library', 'Application Support', 'Vivian');
  }
  return join(homedir(), '.local', 'share', 'Vivian');
}

// ========== 检查项 ==========

check('Rust 工具链 (cargo)', () => {
  const v = run('cargo --version');
  if (!v.includes('cargo')) throw new Error('cargo 不可用');
  return v;
});

check('Rust 编译器 (rustc)', () => {
  const v = run('rustc --version');
  if (!v.includes('rustc')) throw new Error('rustc 不可用');
  return v;
});

check('Node.js', () => {
  const v = run('node --version');
  return v;
});

check('npm', () => {
  const v = run('npm --version');
  return v;
});

check('Tauri CLI', () => {
  try {
    return run('cargo tauri --version');
  } catch {
    try {
      return run('npx tauri --version');
    } catch {
      throw new Error('Tauri CLI 未安装（cargo tauri / npx tauri 均不可用）');
    }
  }
});

check('前端依赖 (node_modules)', () => {
  const dir = join(ROOT, 'node_modules');
  if (!existsSync(dir)) throw new Error('node_modules 不存在，请运行 npm install');
  const count = readdirSync(dir).length;
  return `${count} 个包`;
});

check('配置文件 (config.yaml)', () => {
  const appData = getAppDataDir();
  const candidates = [
    join(appData, 'config.yaml'),
    join(ROOT, 'src-tauri', 'config.yaml'),
  ];
  for (const p of candidates) {
    if (existsSync(p)) {
      const content = readFileSync(p, 'utf-8');
      if (content.length === 0) throw new Error(`${p} 为空`);
      return `${p} (${content.length} 字节)`;
    }
  }
  throw new Error('config.yaml 未找到（首次运行时由应用自动创建）');
});

check('应用数据目录', () => {
  const dir = getAppDataDir();
  if (!existsSync(dir)) {
    return `${dir}（尚未创建，首次运行应用后自动生成）`;
  }
  const items = readdirSync(dir);
  return `${dir}（${items.length} 项: ${items.slice(0, 5).join(', ')}${items.length > 5 ? '...' : ''}）`;
});

check('Live2D 模型文件', () => {
  const modelDir = join(ROOT, 'public', 'Vivian');
  const required = ['Vivian.model3.json', 'Vivian.moc3'];
  const missing = required.filter(f => !existsSync(join(modelDir, f)));
  if (missing.length > 0) {
    throw new Error(`缺失文件: ${missing.join(', ')}`);
  }
  return `模型完整 (${modelDir})`;
});

check('GPT-SoVITS 集成路径', () => {
  const appData = getAppDataDir();
  const candidates = [
    join(appData, 'config.yaml'),
    join(ROOT, 'src-tauri', 'config.yaml'),
  ];
  let configPath = null;
  for (const p of candidates) {
    if (existsSync(p)) { configPath = p; break; }
  }
  if (!configPath) {
    return 'config.yaml 不存在，跳过 GPT-SoVITS 路径检查';
  }
  const content = readFileSync(configPath, 'utf-8');
  const match = content.match(/gpt_sovits[^:]*:\s*\n\s*integration_path:\s*(.+)/);
  if (!match) {
    return '未配置 GPT-SoVITS 集成路径（TTS 将使用其他后端）';
  }
  const sovitsPath = match[1].trim().replace(/^["']|["']$/g, '');
  if (!existsSync(sovitsPath)) {
    throw new Error(`配置路径不存在: ${sovitsPath}`);
  }
  const apiV2 = join(sovitsPath, 'api_v2.py');
  if (!existsSync(apiV2)) {
    throw new Error(`api_v2.py 不存在: ${apiV2}`);
  }
  return `集成包完整: ${sovitsPath}`;
});

check('日志目录', () => {
  const logDir = join(getAppDataDir(), 'logs');
  if (!existsSync(logDir)) {
    return `${logDir}（尚未创建，应用运行后自动生成）`;
  }
  const logs = readdirSync(logDir).filter(f => f.endsWith('.log'));
  return `${logDir}（${logs.length} 个日志文件）`;
});

check('网络连通性', () => {
  try {
    run('curl -s -o /dev/null -w "%{http_code}" --connect-timeout 3 https://www.baidu.com');
    return '可访问 baidu.com（国内网络正常）';
  } catch {
    try {
      run('curl -s -o /dev/null -w "%{http_code}" --connect-timeout 5 https://1.1.1.1');
      return '可访问 1.1.1.1（国际网络正常）';
    } catch {
      throw new Error('网络不通，请检查代理设置');
    }
  }
});

// ========== 输出报告 ==========

const pass = results.filter(r => r.status === 'pass');
const fail = results.filter(r => r.status === 'fail');

console.log('\n' + '='.repeat(60));
console.log('  Vivian 诊断报告');
console.log('='.repeat(60));

for (const r of results) {
  const icon = r.status === 'pass' ? '[PASS]' : '[FAIL]';
  console.log(`\n${icon} ${r.name}`);
  console.log(`       ${r.detail}`);
}

console.log('\n' + '-'.repeat(60));
console.log(`  结果: ${pass.length} 通过 / ${fail.length} 失败 / ${results.length} 总计`);

if (fail.length > 0) {
  console.log('\n  失败项:');
  for (const r of fail) {
    console.log(`    - ${r.name}: ${r.detail}`);
  }
  console.log('\n  建议优先修复上述失败项后重新运行诊断。');
} else {
  console.log('\n  所有检查项通过，环境健康。');
}

console.log('='.repeat(60) + '\n');
process.exit(fail.length > 0 ? 1 : 0);
