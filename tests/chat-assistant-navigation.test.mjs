import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';
const source = await readFile('src/App.tsx', 'utf8');
const body = source.match(/const ensureWechatWindow = useCallback\(async ([\s\S]*?)\n  }, \[\]\);/)?.[1];
assert.ok(body);
const js = (await transform(`export default async ${body}\n}`, { loader: 'ts', format: 'esm' })).code;
const events = [];
globalThis.invoke = async (name, args) => events.push({ name, args });
globalThis.getCharacterId = () => 'vivian';
try {
 const {default: open} = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
 await open({show:false}); assert.equal(events.length,0,'hidden chat is not pre-created');
 // 桌面助手总览页已移除：聊天窗口没有「带着某个工具页打开」这条路径，一律落首页。
 await open(); assert.deepEqual(events.pop(),{name:'open_chat_window',args:{origin:'chat',panel:null,characterId:'vivian'}});
 await open({show:true}); assert.deepEqual(events.pop(),{name:'open_chat_window',args:{origin:'chat',panel:null,characterId:'vivian'}});
 const chat = await readFile('src/components/ChatWindow.tsx','utf8');
 assert.doesNotMatch(chat,/set_chat_close_policy/,'the header no longer toggles the outside-click close policy');
 assert.doesNotMatch(chat,/isAssistantOverview/,'the assistant overview layer is gone');
 // 工具子页不带自己的标题栏和返回键，两者都由 chat 头部承担：标题走共享分区目录（单一真相源），
 // 返回键一律回首页（没有上一层总览可退）。
 assert.match(chat,/from '\.\/assistantPanels'/,'tool page titles come from the shared catalogue');
 assert.match(chat,/assistantPanelTitle\(assistantTarget\.panel/,'the header title follows the active tool page');
 assert.match(chat,/else setView\('home'\);/,'the header back key leaves a tool page straight to home');
 // 首页三点改为呼出下拉；私聊三点仍是进聊天详情。
 assert.match(chat,/const \[headerMenuOpen, setHeaderMenuOpen\] = useState\(false\)/,'the header three-dot owns a dropdown');
 assert.match(chat,/if \(view === 'home'\) setHeaderMenuOpen\(\(open\) => !open\); else setView\('details'\)/,'home toggles the menu, private still opens details');
 assert.match(chat,/role="menu"/,'the dropdown is a menu');
 for (const id of ['notes','shortcuts','screen','settings']) assert.match(chat,new RegExp(`id: '${id}'`),`the dropdown keeps the ${id} entry`);
 assert.doesNotMatch(chat,/t\('chat\.desktop_assistant'/,'the three-dot no longer jumps to a page');
 assert.match(chat,/panel=\{assistantTarget\.panel\}/,'the tool page is controlled by the header');
 assert.doesNotMatch(chat,/onPanelChange=/,'with the overview gone nothing changes the page from inside');
 assert.doesNotMatch(chat,/initialPanel=\{assistantTarget\.panel\}\s*\/>/,'the tool panel is the controlled panel, not an initial-only value');
 assert.doesNotMatch(chat,/key=\{`\$\{assistantTarget\.character\}:\$\{assistantTarget\.panel\}`\}/,'the panel key must not include the tab, or every switch remounts the page and drops drafts');
 console.log('Chat navigation: no precreation, single native owner, header dropdown owns the tool pages passed');
} finally {delete globalThis.invoke; delete globalThis.getCharacterId;}
