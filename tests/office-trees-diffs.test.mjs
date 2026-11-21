import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { chromium } from 'playwright';
import path from 'node:path';

const root = process.cwd();
const out = path.join(root, 'tmp/office-trees-diffs-test');
await mkdir(out, { recursive: true });
const source = 'src/components/mind-inspector/pages';
await build({ stdin: { contents: `
import React from 'react';import{createRoot}from'react-dom/client';
import Tree from './${source}/WorkspaceFileTree';import Changes from './${source}/ChangesPanel';import WorkDiffBlock from './${source}/WorkDiffBlock';
import './${source}/CodeAgentPage.css';
window.__calls=[];window.__opened=[];window.__added=[];window.__channels=[];
window.__scenario=new URLSearchParams(location.search).get('scenario');window.__revision='r1';
window.__patch='diff --git a/example.ts b/example.ts\\n--- a/example.ts\\n+++ b/example.ts\\n@@ -10,2 +10,3 @@\\n context\\n-old\\n+new\\n++literal plus\\n';
if(window.__scenario==='bulk-diff'||window.__scenario==='raw-bulk')window.__patch='diff --git a/example.ts b/example.ts\\n--- a/example.ts\\n+++ b/example.ts\\n@@ -0,0 +1,10000 @@\\n'+Array.from({length:window.__scenario==='raw-bulk'?3995:10000},(_,i)=>'+const value'+i+' = '+i+';').join('\\n')+'\\n';
window.__notify=directory=>window.__channels.forEach(channel=>channel.onmessage?.(directory));
window.__extra=[];
createRoot(document.getElementById('app')).render(window.__scenario.includes('tree')?
<Tree root="G:/fixture" onOpenFile={p=>window.__opened.push(p)}/>:window.__scenario==='readonly'?<WorkDiffBlock text={window.__patch} maxHeight={360}/>:
<Changes workspace="G:/fixture" onOpen={p=>window.__opened.push(p)} onAddToChat={p=>window.__added.push(p)} fallback="tool fallback"/>);
`, resolveDir: root, loader: 'tsx' }, outdir: out, entryNames: 'app', bundle: true, splitting: true,
  format: 'esm', target: 'es2022', minify: true, define: { 'process.env.NODE_ENV': '"production"' }, logLevel: 'silent',
  plugins: [{ name: 'office-host-fixture', setup(b) {
    b.onResolve({ filter: /changeReviewStrings$/ }, () => ({ path: 'empty', namespace: 'empty' }));
    b.onLoad({ filter: /.*/, namespace: 'empty' }, () => ({ contents: '' }));
    b.onResolve({ filter: /^react-i18next$/ }, () => ({ path: 'translation', namespace: 'translation' }));
    b.onLoad({ filter: /.*/, namespace: 'translation' }, () => ({ contents: `export const useTranslation=()=>({t:(key,options)=>key+(options?.count??'')});` }));
    b.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'host', namespace: 'host' }));
    b.onLoad({ filter: /.*/, namespace: 'host' }, () => ({ contents: `
export class Channel{constructor(){window.__channels.push(this)}}
export async function invoke(command,args){window.__calls.push({command,args});
 if(command==='coding_watch_directories')return 'watch';if(command==='coding_unwatch_directories')return;
 if(command==='coding_list_dir_tree'){
  if(window.__scenario==='bulk-tree')return Array.from({length:10000},(_,i)=>({name:'file-'+String(i).padStart(5,'0')+'.ts',path:'G:/fixture/file-'+String(i).padStart(5,'0')+'.ts',is_dir:false}));
  return args.directory==='G:/fixture'?[{name:'src',path:'G:\\\\fixture\\\\src',is_dir:true},{name:'README.md',path:'G:/fixture/README.md',is_dir:false}]:[{name:'main.ts',path:'G:\\\\fixture\\\\src\\\\main.ts',is_dir:false},...window.__extra];
 }
 if(command==='git_changed_files')return{is_repo:true,root:'G:/fixture',truncated:false,files:[{path:'example.ts',original_path:window.__scenario==='rename'?'old.ts':null,index_status:window.__scenario==='rename'?'R':' ',worktree_status:'M',untracked:false,conflict:false}]};
 if(command==='git_file_diff')return{patch:window.__scenario==='truncated'?window.__patch.slice(0,-16):window.__scenario==='rename'?'diff --git a/old.ts b/example.ts\\nsimilarity index 100%\\nrename from old.ts\\nrename to example.ts\\n':window.__patch,revision:window.__revision,binary:window.__scenario==='binary',truncated:window.__scenario==='truncated'};
 throw Error(command);
}` }));
  } }],
});
await writeFile(path.join(out, 'index.html'), `<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/app.css">
<style>body{margin:0;background:#faf8f3;font:13px system-ui;--codex-ink:#36352e;--codex-ink-faint:#807b6c;--codex-paper:#fcfaf5;--codex-line:#ded9cd;--codex-accent:#537d96;--codex-accent-soft:#e6edf0;--codex-success:#357b51;--codex-success-soft:#e0f0e3;--codex-danger:#bd4d4d;--codex-danger-soft:#f7e0dd;--codex-mono:Consolas,monospace}#app{max-width:900px;padding:16px;box-sizing:border-box}</style>
<div id="app"></div><script type="module" src="/app.js"></script>`);
const server = createServer(async (req, res) => {
  try {
    const name = req.url.split('?')[0];
    const file = name === '/' ? 'index.html' : name.slice(1);
    res.setHeader('content-type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html; charset=utf-8');
    res.end(await readFile(path.join(out, file)));
  } catch { res.statusCode = 404; res.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await chromium.launch({ headless: true, executablePath: 'C:/Program Files/Google/Chrome/Application/chrome.exe' });
  const page = await browser.newPage({ viewport: { width: 1000, height: 1000 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const visit = async scenario => { await page.goto(`http://127.0.0.1:${server.address().port}/?scenario=${scenario}`); };

  await visit('tree');
  const src = page.getByRole('treeitem', { name: 'src', exact: true });
  await src.focus();
  await src.press('ArrowRight');
  await page.getByRole('treeitem', { name: 'main.ts', exact: true }).waitFor();
  const reads = await page.evaluate(() => window.__calls.filter(c => c.command === 'coding_list_dir_tree'));
  assert.ok(reads.some(c => c.args.directory === 'G:\\fixture\\src'), 'expanding loads the actual native path');
  await page.getByRole('treeitem', { name: 'main.ts', exact: true }).press('Enter');
  assert.deepEqual(await page.evaluate(() => window.__opened), ['G:\\fixture\\src\\main.ts']);
  await page.getByRole('textbox').fill('src/main');
  await page.getByRole('treeitem', { name: 'README.md', exact: true }).waitFor({ state: 'hidden' });
  assert.equal(await page.getByRole('treeitem', { name: 'main.ts', exact: true }).count(), 1);
  await page.getByRole('textbox').press('Escape');
  await page.getByRole('treeitem', { name: 'README.md', exact: true }).waitFor();
  await page.evaluate(() => { window.__extra = [{ name: 'new.ts', path: 'G:\\fixture\\src\\new.ts', is_dir: false }]; window.__notify('G:\\fixture\\src'); });
  await page.getByRole('treeitem', { name: 'new.ts', exact: true }).waitFor();
  assert.equal(await src.getAttribute('aria-expanded'), 'true', 'refresh keeps expansion');
  await src.press('ArrowLeft');
  await page.getByRole('treeitem', { name: 'main.ts', exact: true }).waitFor({ state: 'hidden' });

  await visit('bulk-tree');
  await page.getByRole('treeitem', { name: 'file-00000.ts', exact: true }).waitFor();
  assert.ok(await page.getByRole('treeitem').count() < 100, '10,000 files only mount a viewport');
  await page.getByRole('treeitem').first().press('End');
  await page.getByRole('treeitem', { name: 'file-09999.ts', exact: true }).waitFor();
  await page.getByRole('treeitem', { name: 'file-09999.ts', exact: true }).press('Enter');
  assert.equal(await page.evaluate(() => window.__opened.at(-1)), 'G:/fixture/file-09999.ts');

  await visit('diff');
  await page.getByRole('treeitem', { name: 'example.ts', exact: true }).click();
  await page.getByRole('button', { name: 'changeReview.split', exact: true }).click();
  await page.locator('diffs-container [data-diff-type="split"]').waitFor();
  // Diffs renders number cells in its shadow root. A deletion must retain the old side.
  const deleted = page.locator('diffs-container [data-line-type="change-deletion"][data-line="11"]');
  await deleted.first().waitFor();
  const oldNumber = page.locator('diffs-container [data-line-type="change-deletion"][data-column-number="11"]');
  await oldNumber.first().click();
  await page.getByRole('textbox', { name: 'changeReview.commentPlaceholder' }).fill('Keep the original behavior');
  await page.getByRole('button', { name: 'changeReview.save', exact: true }).click();
  await page.getByRole('button', { name: 'changeReview.addToChat', exact: true }).click();
  assert.ok((await page.evaluate(() => window.__added.at(-1))).includes('changeReview.old · L11'));
  assert.ok((await page.evaluate(() => window.__added.at(-1))).includes('```\nold\n```'));
  await page.locator('diffs-container [data-line-type="context"][data-column-number="10"]').first().click();
  await page.getByRole('textbox', { name: 'changeReview.commentPlaceholder' }).fill('Check the old context');
  await page.getByRole('button', { name: 'changeReview.save', exact: true }).click();
  await page.getByRole('button', { name: 'changeReview.addToChat', exact: true }).click();
  assert.ok((await page.evaluate(() => window.__added.at(-1))).includes('changeReview.old · L10'));
  await page.getByRole('button', { name: 'changeReview.wrap', exact: true }).click();
  await page.locator('diffs-container [data-overflow="wrap"]').waitFor();
  await page.evaluate(() => document.documentElement.dataset.theme = 'dark');
  await page.waitForFunction(() => getComputedStyle(document.querySelector('diffs-container').shadowRoot.querySelector('pre')).colorScheme === 'dark');
  await page.screenshot({ path: path.join(out, 'split-dark.png'), fullPage: true });
  await page.evaluate(() => document.documentElement.dataset.theme = 'light');
  await page.getByRole('button', { name: 'changeReview.unified', exact: true }).click();
  await page.locator('diffs-container [data-diff-type="single"]').waitFor();
  await page.setViewportSize({ width: 360, height: 900 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  await page.screenshot({ path: path.join(out, 'narrow.png'), fullPage: true });

  await visit('readonly');
  await page.locator('diffs-container').waitFor();
  assert.equal(await page.getByRole('button', { name: 'changeReview.save', exact: true }).count(), 0);
  assert.equal(await page.getByRole('button', { name: 'changeReview.wrap', exact: true }).getAttribute('aria-pressed'), 'true', 'display preferences survive remounts');
  await visit('bulk-diff');
  await page.getByRole('treeitem', { name: 'example.ts', exact: true }).click();
  await page.locator('diffs-container [data-line="1"]').waitFor();
  assert.ok(await page.locator('diffs-container [data-line]').count() < 100, '10k diff mounts only visible lines');
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await page.locator('.change-review-code-view').evaluate(element => element.scrollTop = element.scrollHeight);
  await page.locator('diffs-container [data-line="10000"]').waitFor();
  await visit('raw-bulk');
  await page.getByRole('treeitem', { name: 'example.ts', exact: true }).click();
  await page.locator('.change-review-raw .change-review-line').first().waitFor();
  assert.ok(await page.locator('.change-review-raw .change-review-line').count() < 100, 'truncated large patches also stay virtualized');
  await page.locator('.change-review-raw').evaluate(element => element.scrollTop = element.scrollHeight);
  await page.getByText('+const value3994 = 3994;', { exact: true }).waitFor();

  await visit('truncated');
  await page.getByRole('treeitem', { name: 'example.ts', exact: true }).click();
  await page.locator('.change-review-raw').waitFor();
  assert.ok((await page.locator('.change-review-raw').innerText()).includes('-old'), 'truncated patch retains raw captured content');
  await visit('rename');
  await page.getByRole('treeitem', { name: 'example.ts', exact: true }).click();
  await page.getByText('changeReview.noDiff', { exact: true }).waitFor();
  assert.ok((await page.locator('.change-review-path').innerText()).includes('old.ts → example.ts'));
  await visit('binary');
  await page.getByRole('treeitem', { name: 'example.ts', exact: true }).click();
  await page.getByText('changeReview.binary', { exact: true }).waitFor();
  assert.equal(await page.locator('diffs-container').count(), 0);
  assert.deepEqual(errors, []);
  console.log('Office trees/diffs: lazy paths, keyboard, search, watch refresh, 10k virtualization, split/unified, comments, wrap, themes, narrow layout, truncated/rename/binary patches passed.');
} finally { await browser?.close(); await new Promise(resolve => server.close(resolve)); }
