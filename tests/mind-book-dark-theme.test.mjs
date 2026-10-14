/**
 * 记忆页（书）的深色模式：书是「画」，不是「界面」。
 *
 * 深色下整本书必须仍然是那张浅色纸，只整体调暗 —— 既不换纸也不反色。
 * 这是漫画阅读器的通行做法（MochiDim / 各漫画 App）：反色会毁掉线稿，
 * 而「界面跟着主题走、画不跟」会出现亮插图浮在深色底上的割裂感。
 *
 * 这条不变量有三个容易写坏的点，都在这里钉住：
 *   ① 书根必须出现在 ClaudeTheme.css 那条**浅色**令牌块的选择器里 ——
 *      派生别名（--codex-paper: var(--claude-bg) 等）是在窗口根上求值完再继承的，
 *      只钉书根的 --claude-* 追不回那些已经变成字面量的别名。漏掉时画格里的对白气泡
 *      .voice 会读到深色 --comic-paper，变成深字压深底 1.28:1（真机量过）。
 *   ② 调暗只能用 mix-blend-mode: multiply 的薄幕，**不能**用 filter / transform ——
 *      那两者会让祖先变成 position: fixed 的包含块，面板里的 .rec-overlay
 *      全屏弹窗会缩成面板大小。
 *   ③ 面板入场动画的 fill-mode 不能是 both —— both 会把 translateY(8px)→none 的终态
 *      永久填充成 identity matrix（不是 none），面板因此一直是个包含块。
 *
 * 真机证据：scripts/mind-book-shot.mjs（对比度审计 + fixed 包含块检查）。
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

const claude = read('src/components/mind-inspector/pages/ClaudeTheme.css');
const book = read('src/components/mind-inspector/pages/UnifiedComicBook.css');
const overview = read('src/components/mind-inspector/pages/ComicOverview.css');

// ① 书根在浅色令牌块的选择器里（连同 --codex-* / --panel-* / --graph-* 派生别名一起重算）。
assert.match(
  claude,
  /\.claude-theme,\s*\.mind-inspector-root\.is-claude,\s*\.unified-comic-book\.record-scope\s*\{/,
  'ClaudeTheme.css 的浅色令牌块必须把 .unified-comic-book.record-scope 也列进选择器 —— 否则书根的派生别名会继承窗口根上算好的深色值',
);

// ② 深色块绝不能命中书（否则书就跟着换纸了）。
const darkBlocks = [...claude.matchAll(/^[^{}]*\[data-theme="dark"\][^{}]*\{/gm)].map((m) => m[0]);
assert.ok(darkBlocks.length >= 1, 'ClaudeTheme.css 里应当还有深色块');
for (const block of darkBlocks) {
  assert.doesNotMatch(block, /unified-comic-book/, `深色块不能命中书：${block.trim().slice(0, 90)}`);
}

// ③ 面板自己挂着 .claude-theme，会被深色块（权重 0,3,0）盖掉继承来的浅色值，必须 !important 顶回去。
assert.match(
  book,
  /\.unified-comic-book \.claude-theme\s*\{[^}]*--claude-bg:\s*#faf9f5\s*!important/,
  '面板必须用 !important 把浅色调色盘顶回去（深色块在 :root[data-theme="dark"] .claude-theme 上）',
);

// ④ 调暗薄幕：multiply + pointer-events: none，且绝不用 filter / transform。
const veil = book.match(/:root\[data-theme="dark"\] \.unified-comic-book::after\s*\{([^}]*)\}/);
assert.ok(veil, '深色下必须有一层盖在书上的调暗薄幕');
assert.match(veil[1], /mix-blend-mode:\s*multiply/, '薄幕要用 multiply（按底色比例压暗，纸墨一起下来，对比度保住）');
assert.match(veil[1], /pointer-events:\s*none/, '薄幕不能吃指针事件，否则拖拽/点击全废');
for (const banned of ['filter:', 'backdrop-filter:', 'transform:']) {
  assert.ok(!veil[1].includes(banned), `薄幕不能用 ${banned} —— 会让书根变成 position: fixed 的包含块，.rec-overlay 全屏弹窗会缩成面板大小`);
}

// ⑤ 书根本身也不能有 filter / transform。
const bookRoot = book.match(/\.unified-comic-book\.record-scope\s*\{([^}]*)\}/);
assert.ok(bookRoot, '书根规则要还在');
for (const banned of ['filter:', 'backdrop-filter:', 'transform:']) {
  assert.ok(!bookRoot[1].includes(banned), `书根不能用 ${banned}（同上：包含块）`);
}

// ⑥ 面板入场动画的 fill-mode 必须是 backwards：both 会永久留下一个 identity matrix。
assert.match(
  book,
  /animation:\s*comic-functional-enter[^;]*backwards;/,
  '面板入场动画要用 backwards —— both 会把 translateY→none 的终态填充成 identity matrix，面板会一直是包含块',
);
assert.doesNotMatch(
  book,
  /animation:\s*comic-functional-enter[^;]*\bboth;/,
  '面板入场动画不能回到 fill-mode: both',
);

// ⑦ 画格的纸与气泡走 --comic-paper ← --codex-paper ← --claude-bg 这条链，别改回写死浅色。
assert.match(
  overview,
  /--comic-paper:\s*var\(--codex-paper/,
  '画格纸面要跟着 --codex-paper 走，才能在书根重算令牌时一起变',
);

// ⑧ 调暗压低对比度，次级文字要单独补深一档（深色下 --claude-muted 从 4.55:1 掉到 3.92:1）。
assert.match(
  book,
  /:root\[data-theme="dark"\] \.unified-comic-book \.claude-theme\s*\{[^}]*--claude-muted:/,
  '深色下书里的次级文字色要补深一档，否则调暗后掉到 AA 以下',
);

console.log('记忆页深色模式：书根浅色令牌、multiply 调暗、无包含块、动画 fill-mode、次级文字补偿全部就位。');
