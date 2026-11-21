import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFileSync, existsSync } from 'node:fs';
import { resolve, extname } from 'node:path';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_PATH || 'playwright');
const root = resolve('.');
const requests = [];
const server = createServer((request, response) => {
  const pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
  if (pathname === '/favicon.ico') { response.writeHead(204).end(); return; }
  if (pathname === '/') {
    response.setHeader('Content-Type', 'text/html');
    response.end('<!doctype html><html><body><div id="root"></div></body></html>');
    return;
  }
  const path = resolve(root, `.${pathname}`);
  if (!path.startsWith(root + (process.platform === 'win32' ? '\\' : '/')) || !existsSync(path)) {
    response.writeHead(404).end();
    return;
  }
  requests.push(pathname);
  response.setHeader('Content-Type', extname(path) === '.js' ? 'text/javascript' : 'application/octet-stream');
  response.end(readFileSync(path));
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ channel: 'msedge', headless: true, args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] });
try {
  const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
  const errors = [];
  page.on('pageerror', error => { errors.push(error.message); console.error(error.message); });
  page.on('console', message => { if (message.type() === 'error') console.error(message.text()); });
  await page.addInitScript(({ origin }) => {
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'room' }, currentWebview: { label: 'room', windowLabel: 'room' } },
      invoke: async () => null,
      transformCallback: () => 1,
      unregisterCallback: () => {},
      convertFileSrc: path => `${origin}/plugins/3d-apartment/room/${path.replace(/^PLUGIN_ROOM[\\/]/, '').replace(/\\/g, '/')}`,
    };
  }, { origin });
  await page.goto(origin);
  await page.addScriptTag({ url: `${origin}/plugins/3d-apartment/ui/room.js` });
  await page.evaluate(() => window.VivianApartment.mount(document.getElementById('root'), 'PLUGIN_ROOM'));
  try {
    await page.waitForFunction(() => window.__BOOT__?.some(mark => mark.name === 'scene:first-frame'), null, { timeout: 120000 });
  } catch (error) {
    console.error(await page.evaluate(() => ({ marks: window.__BOOT__, canvases: document.querySelectorAll('canvas').length, room: !!window.__ROOM__ })));
    throw error;
  }
  await page.waitForFunction(() => !document.getElementById('boot-loader'), null, { timeout: 10000 });
  const details = await page.evaluate(() => ({
    calls: window.__ROOM__.renderer.info.render.calls,
    sceneChildren: window.__ROOM__.scene.children.length,
    canvases: document.querySelectorAll('canvas').length,
    marks: window.__BOOT__.map(mark => mark.name),
  }));
  assert.equal(details.canvases, 1);
  assert.ok(details.marks.includes('scene:first-frame'));
  assert.deepEqual(errors, []);
  assert.ok(requests.some(path => path.endsWith('vivian_qver.glb')));
  assert.ok(requests.some(path => path.endsWith('nana_qver.glb')));
  await page.evaluate(() => window.VivianApartment.unmount());
  await page.waitForFunction(() => document.querySelectorAll('canvas').length === 0);
  console.log(JSON.stringify({ ...details, modelRequests: requests.filter(path => path.endsWith('.glb')), errors, unmounted: true }, null, 2));
} finally {
  await browser.close();
  await new Promise(resolve => server.close(resolve));
}
