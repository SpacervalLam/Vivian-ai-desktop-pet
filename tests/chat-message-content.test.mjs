import assert from 'node:assert/strict';
import { build } from 'esbuild';

const bundle = await build({
  entryPoints: ['src/utils/chatMessageContent.ts'],
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { hasVisibleChatText, isVisibleChatMessage } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`,
);

for (const content of ['', ' \n\t ', '（停止发言）', '(nods)', '（点头）\n（沉默）', '__']) {
  assert.equal(hasVisibleChatText(content), false, `hidden assistant text: ${content}`);
  assert.equal(isVisibleChatMessage({ role: 'assistant', content }), false);
}
assert.equal(hasVisibleChatText('（点头）好。'), true);
assert.equal(hasVisibleChatText('（用户的括号内容）', 'user'), true);
assert.equal(isVisibleChatMessage({ role: 'user', content: '(hello)' }), true);
for (const media of [
  { imagePath: 'image.png' }, { imageDataUrl: 'data:image/png;base64,AA==' },
  { voice: { audioPath: 'audio.wav', duration: 1 } },
  { fileMeta: { fileName: 'report.pdf' } },
  { linkCard: { url: 'https://example.com', title: 'Example' } },
]) {
  assert.equal(isVisibleChatMessage({ role: 'assistant', content: '', ...media }), true);
}
assert.equal(isVisibleChatMessage({ role: 'assistant', content: '', streaming: true }), true);

// The same reply can arrive as streamed paragraphs or as one history entry.
const reply = '好。\n（点头）\n先问一句——你要讲多久。\n（停止发言）';
const visibleParts = reply.split('\n').filter((part) => hasVisibleChatText(part));
assert.deepEqual(visibleParts, ['好。', '先问一句——你要讲多久。']);
const rows = reply.split('\n').map((content) => ({ role: 'assistant', content }));
assert.deepEqual(rows.filter(isVisibleChatMessage).map((row) => row.content), visibleParts);
console.log('ChatWindow empty/action-only text and media visibility: passed');
