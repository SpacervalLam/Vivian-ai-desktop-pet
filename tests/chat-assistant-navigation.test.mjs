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
 await open({assistant:{panel:'clipboard',characterId:'nana'}});
 assert.deepEqual(events.pop(),{name:'open_chat_window',args:{origin:'chat',panel:'clipboard',characterId:'nana'}});
 await open(); assert.deepEqual(events.pop(),{name:'open_chat_window',args:{origin:'chat',panel:null,characterId:'vivian'}});
 const chat = await readFile('src/components/ChatWindow.tsx','utf8');
 assert.match(chat,/set_chat_close_policy', \{ origin: 'header' \}/);
 assert.doesNotMatch(source,/ensureWechatWindow\(\{ show: false \}\)/);
 // 助手子页不带自己的标题栏和返回键，两者都由 chat 头部承担：标题走共享分区目录
 // （单一真相源），返回键在子页上退回总览、在总览上才离开助手。
 assert.match(chat,/from '\.\/assistantPanels'/,'assistant panel titles come from the shared catalogue');
 assert.match(chat,/isAssistantOverview\(assistantTarget\.panel\)/,'the header owns the assistant back level');
 assert.match(chat,/assistantPanelTitle\(assistantTarget\.panel/,'the header title follows the active panel');
 assert.doesNotMatch(chat,/key=\{`\$\{assistantTarget\.character\}:\$\{assistantTarget\.panel\}`\}/,'the panel key must not include the tab, or every switch remounts the page and drops drafts');
 assert.doesNotMatch(chat,/initialPanel=\{assistantTarget\.panel\}\s*\/>/,'the assistant panel is controlled, not initial-only');
 console.log('Chat navigation: no precreation, single native owner, role/tab retained, header policy and assistant back level passed');
} finally {delete globalThis.invoke; delete globalThis.getCharacterId;}
