#!/usr/bin/env node
/**
 * Live2D 美术资源打包加密工具
 *
 * 流程：扫描资源 → 每文件独立 zstd 压缩 + AES-256-GCM 加密 → 拼接 Bundle
 * 输出：src-tauri/vivian.bundle.enc + src-tauri/asset_key.bin
 *
 * Bundle 格式（VBL2，按需解压）:
 *   [4字节 magic "VBL2"]
 *   [4字节 文件数 N (LE)]
 *   [N 条索引项：每条 = 4字节路径长度 + 路径UTF8 + 8字节密文偏移 + 8字节密文长度 + 8字节明文长度]
 *   [密文数据段：每文件独立 = 12字节nonce + AES-GCM(zstd(明文)) + 16字节tag]
 *
 * 运行时 bundle_reader 只加载索引段，按请求读取单个文件密文段解密解压，
 * 不再将整个解压后内容常驻内存。
 *
 * 用法：
 *   node scripts/encrypt-assets.mjs          # 打包加密
 *   node scripts/encrypt-assets.mjs --check  # 仅检查 bundle 是否已存在
 */

import { createCipheriv, randomBytes } from 'node:crypto';
import { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync, statSync, unlinkSync } from 'node:fs';
import { join, relative, sep, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { gzipSync } from 'node:zlib';
import { compress as zstdCompressAsync } from '@mongodb-js/zstd';

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = resolve(__dirname, '..');

const PUBLIC_DIR = join(projectRoot, 'public');
const BUNDLE_FILE = join(projectRoot, 'src-tauri', 'vivian.bundle.enc');
const KEY_FILE = join(projectRoot, 'src-tauri', 'asset_key.bin');
const INDEX_FILE = join(projectRoot, 'src-tauri', 'vivian.bundle.index.json');

const ASSET_DIRS = ['Vivian', 'Nana', 'world-bg'];
const KEY_SIZE = 32;
const NONCE_SIZE = 12;
const ZSTD_LEVEL = 19;

function ensureDir(dir) {
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
}

function getOrCreateKey() {
  if (existsSync(KEY_FILE)) {
    const key = readFileSync(KEY_FILE);
    if (key.length !== KEY_SIZE) {
      throw new Error(`密钥文件长度异常: 期望 ${KEY_SIZE} 字节, 实际 ${key.length} 字节`);
    }
    return key;
  }
  const key = randomBytes(KEY_SIZE);
  writeFileSync(KEY_FILE, key);
  console.log(`[encrypt-assets] 生成新密钥: ${KEY_FILE}`);
  return key;
}

function walkDir(dir, files = []) {
  for (const entry of readdirSync(dir)) {
    const fullPath = join(dir, entry);
    const stat = statSync(fullPath);
    if (stat.isDirectory()) {
      walkDir(fullPath, files);
    } else {
      files.push(fullPath);
    }
  }
  return files;
}

async function zstdCompress(data) {
  // 优先用 @mongodb-js/zstd（npm 包，与 Rust zstd crate 完全兼容）
  try {
    const compressed = await zstdCompressAsync(data, ZSTD_LEVEL);
    return { data: Buffer.from(compressed), format: 'zstd' };
  } catch (e) {
    // 回退到系统 zstd cli
    try {
      const tmpIn = join(projectRoot, 'node_modules', '.cache', 'zstd_in.tmp');
      const tmpOut = tmpIn + '.zst';
      ensureDir(dirname(tmpIn));
      writeFileSync(tmpIn, data);
      execFileSync('zstd', [`-${ZSTD_LEVEL}`, '-f', '-o', tmpOut, tmpIn], { stdio: 'pipe' });
      const compressed = readFileSync(tmpOut);
      try { unlinkSync(tmpIn); } catch {}
      try { unlinkSync(tmpOut); } catch {}
      return { data: compressed, format: 'zstd' };
    } catch (e2) {
      // 最终回退到 gzip
      console.warn('[encrypt-assets] 警告: zstd 不可用，使用 gzip 回退（压缩率略低）');
      return { data: gzipSync(data, { level: 9 }), format: 'gzip' };
    }
  }
}

async function buildBundle(allFiles, key) {
  // 收集所有文件: [{path: "Vivian/nana.model3.json", data: Buffer}, ...]
  const entries = [];
  for (const { dir: srcDir, prefix } of allFiles) {
    if (!existsSync(srcDir)) {
      console.warn(`[encrypt-assets] 跳过不存在的目录: ${prefix}`);
      continue;
    }
    const files = walkDir(srcDir);
    for (const file of files) {
      const relPath = relative(srcDir, file).split(sep).join('/');
      const virtualPath = `${prefix}/${relPath}`;
      const data = readFileSync(file);
      entries.push({ path: virtualPath, data });
    }
  }

  // 按 path 排序（便于稳定性和二分查找）
  entries.sort((a, b) => a.path.localeCompare(b.path));

  // 每文件独立：zstd 压缩 → AES-GCM 加密
  console.log(`[encrypt-assets] 逐文件压缩加密 (zstd level ${ZSTD_LEVEL} + AES-256-GCM)...`);
  const encryptedParts = [];
  const indexParts = [];
  const indexMeta = [];
  let dataOffset = 0;
  let plainTotal = 0;
  for (let i = 0; i < entries.length; i++) {
    const e = entries[i];
    const { data: compressed } = await zstdCompress(e.data);
    const encrypted = encrypt(compressed, key);
    encryptedParts.push(encrypted);

    const pathBuf = Buffer.from(e.path, 'utf8');
    const pathLen = Buffer.alloc(4);
    pathLen.writeUInt32LE(pathBuf.length, 0);
    const offBuf = Buffer.alloc(8);
    offBuf.writeBigUInt64LE(BigInt(dataOffset), 0);
    const sizeBuf = Buffer.alloc(8);
    sizeBuf.writeBigUInt64LE(BigInt(encrypted.length), 0);
    const plainBuf = Buffer.alloc(8);
    plainBuf.writeBigUInt64LE(BigInt(e.data.length), 0);
    indexParts.push(Buffer.concat([pathLen, pathBuf, offBuf, sizeBuf, plainBuf]));
    indexMeta.push({
      path: e.path,
      offset: dataOffset,
      size: encrypted.length,
      plainSize: e.data.length,
    });
    dataOffset += encrypted.length;
    plainTotal += e.data.length;
    if ((i + 1) % 50 === 0) {
      console.log(`[encrypt-assets] 已处理 ${i + 1}/${entries.length} 个文件`);
    }
  }

  // 拼接: magic + count + 索引段 + 密文数据段
  const magic = Buffer.from('VBL2', 'ascii');
  const count = Buffer.alloc(4);
  count.writeUInt32LE(entries.length, 0);
  const indexSegment = Buffer.concat(indexParts);
  const dataSegment = Buffer.concat(encryptedParts);
  const bundle = Buffer.concat([magic, count, indexSegment, dataSegment]);

  return { bundle, indexMeta, entries, plainTotal };
}

function encrypt(plaintext, key) {
  const nonce = randomBytes(NONCE_SIZE);
  const cipher = createCipheriv('aes-256-gcm', key, nonce);
  const encrypted = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  const tag = cipher.getAuthTag();
  return Buffer.concat([nonce, encrypted, tag]);
}

async function main() {
  const checkOnly = process.argv.includes('--check');

  if (checkOnly) {
    process.exit(existsSync(BUNDLE_FILE) ? 0 : 1);
  }

  if (!existsSync(PUBLIC_DIR)) {
    console.error(`[encrypt-assets] public 目录不存在: ${PUBLIC_DIR}`);
    process.exit(1);
  }

  const key = getOrCreateKey();

  // 收集所有资源目录
  const allFiles = ASSET_DIRS.map(d => ({
    dir: join(PUBLIC_DIR, d),
    prefix: d,
  }));

  console.log(`[encrypt-assets] 扫描资源...`);
  const { bundle, indexMeta, entries, plainTotal } = await buildBundle(allFiles, key);
  console.log(`[encrypt-assets] Bundle 打包: ${entries.length} 个文件, 明文 ${(plainTotal / 1024 / 1024).toFixed(2)} MB → 密文 ${(bundle.length / 1024 / 1024).toFixed(2)} MB`);

  writeFileSync(BUNDLE_FILE, bundle);
  console.log(`[encrypt-assets] 加密完成: ${BUNDLE_FILE} (${(bundle.length / 1024 / 1024).toFixed(2)} MB)`);

  // 输出索引 JSON（供 build.rs 读取生成 Rust 常量）
  writeFileSync(INDEX_FILE, JSON.stringify({
    version: 2,
    format: 'zstd',
    entries: indexMeta,
  }));
  console.log(`[encrypt-assets] 索引输出: ${INDEX_FILE} (${indexMeta.length} 条)`);
}

main();
