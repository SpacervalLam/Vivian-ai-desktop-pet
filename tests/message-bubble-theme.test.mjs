/**
 * 桌宠消息气泡的角色配色回归测试。
 *
 * 之前 BUBBLE_THEMES 只有 nana / vivian，deepseek / claude 落到 DEFAULT_THEME
 * 的深色变量上（看上去就是「黑色气泡」）。这里用 esbuild 把真实组件打包后
 * 直接调用，检查渲染出来的 style，而不是只比对源码字符串。
 */
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile } from 'node:fs/promises';

// Optional characters supply their themes through installed character packages.
const packageVisuals = Object.fromEntries(await Promise.all(['deepseek', 'claude'].map(async id => {
  const manifest = JSON.parse(await readFile(new URL(`../character-packs/${id}/character.json`, import.meta.url), 'utf8'));
  return [id, manifest.presentation];
})));

const fixtures = {
  react: `export const useState=v=>[typeof v==='function'?v():v,()=>{}],useEffect=()=>{},useRef=v=>({current:v});
          export default {memo:f=>f,useState,useEffect,useRef};`,
  'react/jsx-runtime': `export const jsx=(type,props)=>({type,props}),jsxs=jsx;`,
  'react-i18next': `export const useTranslation=()=>({t:key=>key});`,
  '../utils/ActionText': `export const stripActions=s=>s;`,
  '../utils/bubbleText': `export const computeDuration=()=>0;`,
  '../utils/bubbleContent': `export const STICKER_BUBBLE_SIZE=96;`,
  './stickers/StickerImage': `export default function StickerImage(){return null;}`,
  '../characterPackages': `const visuals=${JSON.stringify(packageVisuals)}; export const characterVisuals=id=>visuals[id]??{};`,
};

const bundle = await build({
  entryPoints: ['src/components/MessageBubble.tsx'],
  bundle: true, write: false, platform: 'node', format: 'esm',
  plugins: [{
    name: 'bubble-theme-fixtures',
    setup(b) {
      b.onResolve({
        filter: /^react$|^react\/jsx-runtime$|^react-i18next$|^\.\.\/utils\/(?:ActionText|bubbleText|bubbleContent)$|^\.\/stickers\/StickerImage$|^\.\.\/characterPackages$/,
      }, ({ path }) => ({ path, namespace: 'fixture' }));
      b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: fixtures[path] }));
    },
  }],
});
const { default: MessageBubble } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`
);

const render = props => MessageBubble({ text: '今天也一起加油呀～', position: 'top', ...props }, {});
const tailOf = tree => {
  const children = Array.isArray(tree.props.children) ? tree.props.children.flat(Infinity) : [tree.props.children];
  return children.find(child => child && typeof child === 'object' && child.props?.style?.borderTop)?.props.style.borderTop;
};

// ---- 1. 四个角色都拿到专属主题，且没有谁掉回默认深色 --------------------------
const IDS = ['vivian', 'nana', 'deepseek', 'claude'];
const styles = Object.fromEntries(IDS.map(id => [id, render({ characterId: id }).props.style]));
for (const id of IDS) {
  assert.notEqual(styles[id].background, 'var(--bg-overlay)', `${id}: 不能回退到默认深色底`);
  assert.notEqual(styles[id].color, 'var(--text-primary)', `${id}: 不能回退到默认文字色`);
}
assert.equal(new Set(IDS.map(id => styles[id].background)).size, 4, '四个角色的底色应各不相同');
assert.equal(new Set(IDS.map(id => styles[id].boxShadow)).size, 4, '四个角色的描边/光晕应各不相同');

// ---- 2. DeepSeek：蓝色边框 + 白色底 + 蓝字 ------------------------------------
const deepseek = styles.deepseek;
assert.match(deepseek.background, /255,\s*255,\s*255/, 'deepseek: 白底');
assert.equal(deepseek.color, '#1D4ED8', 'deepseek: 蓝字');
assert.match(deepseek.boxShadow, /inset 0 0 0 2px #4D6BFE/, 'deepseek: 2px 蓝色边框（inset，不占布局空间）');
assert.match(tailOf(render({ characterId: 'deepseek' })), /#FFFFFF/, 'deepseek: 小尾巴与白底同色');

// ---- 3. Claude：与 Nana 同样的渐变底、浅色字和光晕，使用陶土橙色 ----------------
const claude = styles.claude;
assert.match(claude.background, /linear-gradient\(135deg, rgba\(156, 72, 50, 0.95\) 0%, rgba\(161, 78, 51, 0.92\) 100%\)/, 'claude: 陶土橙渐变');
assert.equal(claude.color, '#FFFAF5', 'claude: 暖白字');
assert.equal(claude.boxShadow, '0 6px 20px rgba(200, 130, 94, 0.38)', 'claude: 与 Nana 一致的柔和外光晕');
assert.match(tailOf(render({ characterId: 'claude' })), /#9C4832/, 'claude: 小尾巴与渐变起始色一致');

// ---- 4. 可读性：新配色的文字/底色对比度至少 4.5:1 -----------------------------
const luminance = hex => {
  const rgb = hex.replace('#', '').match(/../g).map(v => parseInt(v, 16) / 255)
    .map(v => v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  return 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
};
const contrast = (a, b) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};
assert.ok(contrast('#1D4ED8', '#FFFFFF') >= 4.5, 'deepseek: 蓝字在白底上可读性不足');
for (const background of ['#9C4832', '#A14E33', '#A95C43']) {
  assert.ok(contrast('#FFFAF5', background) >= 4.5, `claude: 暖白字在 ${background} 上可读性不足`);
}

// ---- 5. 跨角色对话仍用角色主色描边；未知角色保持原有兜底 ----------------------
for (const [id, colour] of [['deepseek', '77, 107, 254'], ['claude', '200, 130, 94']]) {
  const crossed = render({ characterId: id, crossCharacter: true, listenerName: 'Vivian' }).props.style;
  assert.match(crossed.border, new RegExp(colour), `${id}: 跨角色气泡应使用角色主色边框`);
}
const fallback = render({ characterId: 'some-new-character' }).props.style;
assert.equal(fallback.background, 'var(--bg-overlay)', '未收录角色仍回退默认深色');
assert.equal(fallback.color, 'var(--text-primary)', '未收录角色仍回退默认文字色');

console.log('Message bubble themes: vivian / nana / deepseek / claude(陶土橙渐变暖白字) 配色、小尾巴、对比度与兜底全部验证通过。');
