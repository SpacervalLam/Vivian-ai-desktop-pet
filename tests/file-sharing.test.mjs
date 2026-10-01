import assert from 'node:assert/strict';
import { build as bundleSource } from 'esbuild';

const calls = [];
const saved = { path: 'C:/data/attachments/id/report.pdf', filename: 'report.pdf', size: 1234 };
const extracted = { filename: saved.filename, text: 'Report contents', file_type: 'pdf', truncated: false, original_char_count: 15 };
let extractionFails = false;
globalThis.__fileInvoke = async (command, args) => {
  calls.push({ command, args });
  if (command === 'save_shared_file') return saved;
  if (extractionFails) throw new Error('Encrypted PDF');
  return extracted;
};
const bundle = await bundleSource({
  entryPoints: ['src/utils/sharedFiles.ts'], bundle: true, write: false, format: 'esm', platform: 'node',
  plugins: [{ name: 'mock-tauri', setup(build) {
    build.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'core', namespace: 'mock' }));
    build.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({ contents: 'export const invoke = (...args) => globalThis.__fileInvoke(...args);' }));
  } }],
});
const { prepareSharedFile, fileMessage, fileMetadata } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const shared = await prepareSharedFile('D:/report.pdf');
assert.deepEqual(calls, [
  { command: 'save_shared_file', args: { sourcePath: 'D:/report.pdf' } },
  { command: 'extract_file_text', args: { sourcePath: saved.path } },
]);
assert.equal(fileMetadata(shared.result, shared.file).file_path, saved.path);
assert.equal(fileMetadata(shared.result, shared.file).file_size, 1234);
assert.match(fileMessage(shared.result), /Report contents/);
assert.match(fileMessage({ ...extracted, truncated: true, original_char_count: 20000 }), /20000/);
assert.match(fileMessage({ ...extracted, text: '' }), /未提取到可阅读的文本/);
extractionFails = true;
const unreadable = await prepareSharedFile('D:/encrypted.pdf');
assert.equal(unreadable.file.path, saved.path, 'extraction failure still leaves a shareable file');
assert.equal(unreadable.result.file_type, 'unsupported');
assert.match(fileMessage(unreadable.result), /请勿推测文件内容/);
delete globalThis.__fileInvoke;
console.log('File sharing: snapshot extraction, metadata, truncation, and unreadable file fallback passed.');
