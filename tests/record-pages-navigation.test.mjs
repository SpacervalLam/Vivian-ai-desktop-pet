// @test-environment live-desktop
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile, readdir } from 'node:fs/promises';
import { chromium } from 'playwright-core';

const result = await build({
  stdin: { contents: `import React from 'react';import{createRoot}from'react-dom/client';
    import Inspector from './src/components/mind-inspector/MindInspector';
    createRoot(document.getElementById('root')).render(<Inspector/>);`, loader: 'tsx', resolveDir: process.cwd() },
  bundle: true, write: false, outfile: 'tmp/record-navigation.js', format: 'iife', jsx: 'automatic',
  external: ['/record-art/*'], define: { 'import.meta.env.BASE_URL': '"/"' },
  plugins: [{ name: 'desktop-fixture', setup(b) {
    b.onResolve({ filter: /^@tauri-apps\/api\// }, args => ({ path: args.path, namespace: 'native' }));
    b.onLoad({ filter: /.*/, namespace: 'native' }, () => ({ contents: `
      export const invoke=async(name)=>name==='apartment_plugin_status'?{enabled:false}:name==='list_todos'?{items:[]}:name==='list_scheduled_tasks'?{tasks:[]}:name==='get_user_facts'?{basic_facts:[],custom_facts:[],recent_state:{recent_goals:[],current_projects:[],recent_preferences:[]}}:name==='get_discovery_profile'?{interests:[],probes:[],disliked_topics:[]}:name==='get_discovery_duty'?null:[];
      export const listen=async(name,fn)=>{window.handlers[name]=fn;return()=>{delete window.handlers[name]}};
      export const emit=async()=>{};export const convertFileSrc=x=>x;
      export class WebviewWindow{};export class LogicalPosition{};
      export const getCurrentWindow=()=>({minimize:async()=>{},close:async()=>{},onDragDropEvent:async()=>()=>{}});` }));
    b.onResolve({ filter: /^react-i18next$/ }, () => ({ path: 'translations', namespace: 'translations' }));
    b.onLoad({ filter: /.*/, namespace: 'translations' }, () => ({ resolveDir: process.cwd(), contents: `
      import{recordTranslations}from'./src/i18n/records.js';
      const words={'mind_inspector.nav_memory':'对话','mind_inspector.nav_diary':'日记本','mind_inspector.nav_notebook':'笔记本','mind_inspector.nav_planner':'待办与提醒','mind_inspector.nav_code':'办公'};
      export const initReactI18next={type:'3rdParty',init(){}};
      export const useTranslation=()=>({i18n:{language:'zh-CN',resolvedLanguage:'zh-CN'},t:(key)=>words[key]??recordTranslations['zh-CN'][key.replace(/^records\./,'')]??key});` }));
    // Office internals are unchanged; this fixture exercises the shared shell and four real record pages.
    b.onResolve({ filter: /\/CodeAgentPageNew$/ }, () => ({ path: 'office', namespace: 'office' }));
    b.onLoad({ filter: /.*/, namespace: 'office' }, () => ({ resolveDir: process.cwd(), contents: `export default function Office(){return <div data-office>Office</div>}`, loader: 'jsx' }));
  } }],
});
const js = result.outputFiles.find(f => f.path.endsWith('.js')).text;
const css = result.outputFiles.find(f => f.path.endsWith('.css')).text;
const browser = await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1366, height: 900 } });
  const errors = [], missing = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route('http://records.test/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/') return route.fulfill({ contentType: 'text/html', body: `<style>body{margin:0}${css}</style><div id="root"></div><script>window.handlers={}</script><script>${js}</script>` });
    try { return await route.fulfill({ contentType: 'image/webp', body: await readFile('public' + path) }); }
    catch { missing.push(path); return route.fulfill({ status: 404, body: '' }); }
  });
  await page.goto('http://records.test/');
  for (const [key, label, selector] of [
    ['graph', '对话', '.record-memory'], ['diary', '日记本', '.record-diary-art'],
    ['notebook', '笔记本', '.record-notebook-art'], ['planner', '待办与提醒', '.record-planner'],
    ['code', '办公', '[data-office]'],
  ]) {
    await page.locator('.mind-sb-menu-btn').click();
    assert.equal(await page.locator('.mind-nav-btn').count(), 5);
    await page.locator('.mind-nav-btn').filter({ hasText: label }).click();
    await page.locator(selector).waitFor();
    assert.equal(await page.locator('.mind-nav-btn.active').textContent(), label);
    assert.equal(await page.locator('.mind-sb-cover-sub').textContent(), label);
    assert.equal(await page.locator('.unified-comic-book,.mind-comic-book').count(), 0);
    assert.equal(await page.locator('.mind-page-error-title').count(), 0);
    if (key !== 'code') assert.equal(await page.locator('.record-workspace').getAttribute('data-record-page'), key);
    await page.setViewportSize({ width: 430, height: 800 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${key}: narrow viewport`);
    await page.setViewportSize({ width: 1366, height: 900 });
  }
  // Existing event destinations still land on independent pages and select their internal view.
  await page.evaluate(() => window.handlers['memory:navigate']({ payload: { page: 'scheduler' } }));
  await page.locator('.record-planner').waitFor();
  assert.equal(await page.locator('.rec-seg-item[aria-pressed="true"]').count(), 1);
  await page.evaluate(() => window.handlers['memory:navigate']({ payload: { page: 'profile' } }));
  await page.locator('.memory-profile-panel').waitFor();
  await page.setViewportSize({ width: 430, height: 800 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  assert.deepEqual(errors, []);
  assert.deepEqual(missing.filter(p => p.startsWith('/record-art/')), []);
  assert.equal((await readdir('public/record-art')).length, 6);
  if (process.env.RECORD_PAGES_SCREENSHOT) {
    await page.setViewportSize({ width: 1366, height: 900 });
    await page.evaluate(() => window.handlers['memory:navigate']({ payload: { page: 'diary' } }));
    await page.locator('.record-diary-art').waitFor();
    await page.evaluate(() => document.getAnimations().forEach(animation => {
      if (animation.effect?.getTiming().iterations !== Infinity) animation.finish();
    }));
    await page.screenshot({ path: process.env.RECORD_PAGES_SCREENSHOT });
  }
  console.log('Five peer pages, real record rendering, legacy deep links, artwork and narrow layout passed.');
} finally { await browser.close(); }
