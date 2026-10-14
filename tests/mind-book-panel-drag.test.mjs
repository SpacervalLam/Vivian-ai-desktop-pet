/**
 * 整本书（画格 + 四个功能面板）必须是一块可拖拽的纸，而且从面板上按下去也要能拖。
 *
 * 这是条纯「接线」不变量，只能在源码层面钉住：
 *   · 画格 .mind-comic-book .comic 与四个面板 .unified-functional-panel 是同一张纸的两层，
 *     面板浮在上面且 pointer-events: auto，按在面板上的事件到不了画格；
 *   · 所以拖拽通路必须挂在两者的公共祖先 —— 书根 .unified-comic-book 上，
 *     挂在画格上就只能从画格起手，正是「按在面板上拖不动整页」那个 bug。
 * 真机量过的复现见 scripts/mind-book-drag-probe.mjs（面板起手的用例）。
 *
 * 为什么面板不能靠 transform 跟随：面板里挂着 .rec-overlay（position: fixed 的全屏弹窗），
 * 祖先一旦有 transform / translate，fixed 就会改挂到那个祖先上，弹窗会缩成面板大小。
 * 所以通路必须是布局属性 left（calc 变量和），下面第 4 条就是在钉这个前提。
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

const css = read('src/components/mind-inspector/pages/UnifiedComicBook.css');
const book = read('src/components/mind-inspector/pages/UnifiedComicBook.tsx');
const comic = read('src/components/mind-inspector/pages/ComicOverview.tsx');
const record = read('src/components/mind-inspector/pages/RecordTheme.css');

// 1) 面板横向位置 = 槽位变量 + 拖拽偏移变量。
assert.match(
  css,
  /\.unified-functional-panel:not\(\.is-expanded\)\s*\{[^}]*left:\s*calc\(var\(--panel-left[^;]*var\(--comic-drag-x/,
  '面板的 left 必须写成 calc(var(--panel-left) + var(--comic-drag-x))，否则拖拽时面板不跟着走',
);

// 2) 展开态仍走 inset，不能被上面那条规则碰到（否则展开的面板会塌回槽位大小）。
assert.match(
  css,
  /\.unified-functional-panel\.is-expanded\s*\{[^}]*inset:\s*36px 20px 16px/,
  '展开态面板必须保持 inset: 36px 20px 16px',
);

// 3) 槽位由 --panel-left 下发，而不是直接写 left（写 left 就吃不到拖拽偏移了）。
assert.match(book, /'--panel-left':\s*`/, 'UnifiedComicBook 必须把槽位下发成 --panel-left');
const placement = book.match(/const placement:[^\n]+/);
assert.ok(placement, '功能面板保留独立的槽位样式');
assert.doesNotMatch(placement[0], /\bleft:\s*bounds\.left/, '面板不能直接写 left；翻页按钮的位置不受此约束');

// 4) 前提：面板里确实有 position: fixed 的弹层，所以不能用 transform 跟随。
assert.match(record, /\.rec-overlay\s*\{[^}]*position:\s*fixed/, '.rec-overlay 仍是全屏 fixed 弹层');

// 5) 拖拽归书根：偏移写在书根上，拖拽期间临时给面板挂同曲线的 left 过渡，松手清掉。
assert.match(book, /setProperty\('--comic-drag-x'/, '书根要在拖拽中把偏移写进 --comic-drag-x');
assert.match(book, /removeProperty\('--comic-drag-x'\)/, '松手后要摘掉 --comic-drag-x');
assert.match(book, /panel\.style\.transition = 'left \.22s ease-out'/, '拖拽期间面板要挂与画格 transform 同曲线的 left 过渡');
assert.match(book, /endFollow\(turning\)/, '翻页那条路径要立即摘掉过渡，否则面板会从旧槽位滑过去');

// 6) 拖拽通路必须挂在书根这个公共祖先上，并且自己找画格来平移。
assert.match(book, /const root = useRef<HTMLDivElement>\(null\)/, '书根 ref 要还在');
assert.match(book, /querySelector<HTMLElement>\('\.mind-comic-book \.comic'\)/, '书根要能找到画格来施加 transform');
assert.match(book, /onPointerMove=/, '书根要有 onPointerMove —— 拖拽通路在书根上，不在画格上');
assert.match(book, /setPointerCapture\(event\.pointerId\)/, '拖拽要抓住指针，否则移出书根就断线');

// 7) 起手过滤：面板与画格都不能被排除（否则又拖不动），真控件和 fixed 弹层必须被排除。
const downBlock = book.match(/onPointerDown=\{event => \{([\s\S]*?)\}\} onPointerMove=/);
assert.ok(downBlock, '书根要有 onPointerDown 起手块');
const startFilter = [...downBlock[1].matchAll(/closest\('([^']+)'\)/g)].find(match => match[1].split(',').includes('button'));
assert.ok(startFilter, '书根的 pointerdown 要有起手过滤');
for (const mustIgnore of ['button', 'a', 'input', 'textarea', 'select', '[contenteditable]', '[role="button"]', '.is-editing', '.rec-overlay', '.unified-book-contents', '.record-diary-index-scroll', '.record-note-index-scroll']) {
  assert.ok(startFilter[1].includes(mustIgnore), `起手过滤要排除 ${mustIgnore}`);
}
for (const mustDrag of ['.unified-functional-panel', '.mind-comic-book']) {
  assert.ok(!startFilter[1].includes(mustDrag), `起手过滤不能排除 ${mustDrag} —— 排除它就从面板/画格上拖不动了`);
}

// 8) 画格自己让位：functional（书页里那层纸）时不再自带拖拽，免得两处都动。
assert.match(comic, /if \(functional \|\|/, 'ComicOverview 在 functional 下要让出拖拽');
assert.doesNotMatch(comic, /setProperty\('--comic-drag-x'/, '跟随偏移的写入已经搬到书根，画格里不该再有');
assert.doesNotMatch(comic, /querySelectorAll<HTMLElement>\('\.unified-functional-panel/, '面板过渡的挂/摘也已经搬到书根');

console.log('书页拖拽：单一通路挂在书根上，面板起手可拖，真控件与 fixed 弹层被排除，画格已让位。');
