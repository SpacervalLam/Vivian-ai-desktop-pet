// @test-environment live-desktop
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';

const built = await build({
  stdin: { contents: `
    import React,{useRef,useEffect} from 'react';import{createRoot}from'react-dom/client';
    import{useLiquidGlassInteractions}from'./src/hooks/useLiquidGlassInteractions';
    import './src/styles/global.css';import './src/components/PhoneHomeScreen.css';import './src/components/PhoneLiquidGlass.css';
    function App(){const ref=useRef(null);useLiquidGlassInteractions(ref);useEffect(()=>{window.glassReady=true},[]);return <div ref={ref} className="chat-device">
      <button className="phone-lock-action" onClick={e=>{e.stopPropagation();window.clicks++}}>Torch</button>
      <button className="phone-lock-notification phone-liquid-glass">Notification</button>
      <label className="phone-app-search phone-liquid-glass"><input placeholder="Search"/></label>
      <div className="phone-home-dock phone-liquid-glass"><button className="phone-app"><span className="phone-app-art">LINE</span></button><button className="phone-app"><span className="phone-app-art">Settings</span></button></div>
      <div className="chat-app-navigation"><div><button>Back</button><button>More</button></div></div>
      <div className="phone-liquid-glass floating" style={{position:'absolute',right:10,top:550,borderRadius:20}}><button>Floating</button></div>
      <button id="plain">Plain content</button>
    </div>};window.clicks=0;createRoot(document.getElementById('root')).render(<React.StrictMode><App/></React.StrictMode>);
  `, resolveDir: process.cwd(), loader: 'tsx' },
  bundle: true, write: false, outfile: 'glass.js', format: 'esm', jsx: 'automatic', external: ['/wallpapers/*'],
});
const browser = await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 500, height: 700 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('https://glass.test/', route => route.fulfill({ contentType: 'text/html', body: `<!doctype html><style>
    ${built.outputFiles.find(file => file.path.endsWith('.css')).text}
    body{background:#626c89!important} .chat-device{width:360px;margin:40px auto;display:flex;flex-direction:column;align-items:center;gap:24px}
    .phone-home-dock{width:100%;margin:0}.phone-app-art{background:#00c659}.chat-app-navigation{border-radius:28px;background:#ffffff40;padding:8px}.chat-app-navigation button{padding:10px}
    </style><div id="root"></div><script type="module">${built.outputFiles.find(file => file.path.endsWith('.js')).text}</script>` }));
  await page.goto('https://glass.test/');
  const torch = page.locator('.phone-lock-action');
  await torch.waitFor();
  await page.waitForFunction(() => window.glassReady);
  // StrictMode replays passive effects after the initial paint.
  await page.waitForTimeout(250);
  await page.evaluate(() => { window.inputTrace=[]; for (const name of ['pointerleave','focusout','blur','pointercancel']) window.addEventListener(name,event=>window.inputTrace.push([name,event.target?.className]),true); });
  const bounds = await torch.boundingBox();
  await page.mouse.move(bounds.x + 14, bounds.y + 18);
  await page.mouse.down();
  await page.waitForTimeout(130);
  const held = await torch.evaluate(el => ({
    scale: getComputedStyle(el).scale, transform: getComputedStyle(el).transform,
    filter: getComputedStyle(el).filter, x: parseFloat(el.style.getPropertyValue('--glass-contact-x')),
    light: +getComputedStyle(el, '::after').opacity, rim: getComputedStyle(el).borderTopWidth,
  }));
  assert.ok(parseFloat(held.scale) < 1, `holding flexes the glass: ${JSON.stringify(held)} ${JSON.stringify(await page.evaluate(()=>window.inputTrace))}`);
  assert.equal(held.transform, 'none', 'generic pressed transform does not stack');
  assert.equal(held.filter, 'none', 'glass lights up instead of generic button darkening');
  assert.ok(Math.abs(held.x - 14) < 1, 'contact highlight starts at the local pointer position');
  assert.ok(held.light > .95, 'contact light is fully visible while held');
  assert.ok(parseFloat(held.rim) <= 1, 'interaction does not thicken the rim');
  if (process.env.GLASS_SCREENSHOT) await page.screenshot({ path: process.env.GLASS_SCREENSHOT });
  await page.evaluate(() => {
    window.frames=[];const el=document.querySelector('.phone-lock-action');const start=performance.now();
    function sample(){window.frames.push(parseFloat(getComputedStyle(el).scale));if(performance.now()-start<470)requestAnimationFrame(sample)}sample();
  });
  await page.mouse.up();
  await page.waitForTimeout(500);
  assert.equal(await page.evaluate(() => window.clicks), 1, 'stopped click propagation still invokes exactly one action');
  const frames = await page.evaluate(() => window.frames);
  assert.ok(frames.some(value => value > 1.005), 'release has a restrained elastic overshoot');
  assert.ok(new Set(frames.filter(Number.isFinite).map(value => value.toFixed(3))).size > 5, 'release renders intermediate flex frames');
  assert.equal(await torch.evaluate(el => el.getAnimations().length), 0, 'settled animation releases its styles');
  assert.equal(await torch.evaluate(el => getComputedStyle(el, '::after').opacity), '0');

  // Moving away, wheel scrolling, cancelled touch and focus changes never leave a pressed lens.
  await torch.hover();await page.mouse.down();await page.mouse.move(10, 10);await page.mouse.up();
  assert.equal(await torch.getAttribute('data-glass-pressed'), null);
  await torch.hover();await page.mouse.down();await page.mouse.wheel(0, 20);await page.mouse.up();
  assert.equal(await torch.getAttribute('data-glass-pressed'), null);
  await torch.dispatchEvent('pointerdown', { pointerId: 72, pointerType: 'touch', isPrimary: true, button: 0, clientX: bounds.x + 25, clientY: bounds.y + 25 });
  await torch.dispatchEvent('pointercancel', { pointerId: 72 });
  assert.equal(await torch.getAttribute('data-glass-pressed'), null);
  for (let i = 0; i < 4; i++) { await torch.hover();await page.mouse.down();await page.mouse.up(); }
  await page.waitForTimeout(450);
  assert.equal(await torch.evaluate(el => el.getAnimations().length), 0, 'rapid repeated presses settle cleanly');

  await torch.focus();await page.keyboard.down('Space');
  assert.equal(await torch.getAttribute('data-glass-pressed'), '');
  await page.keyboard.up('Space');
  assert.equal(await torch.getAttribute('data-glass-pressed'), null);
  const search = page.locator('input');
  await search.click();await page.keyboard.type('two words');
  assert.equal(await search.inputValue(), 'two words', 'typing space does not trigger a glass press');
  assert.equal(await page.locator('label').getAttribute('data-glass-pressed'), null);

  const dockIcon = page.locator('.phone-home-dock button').first();
  await dockIcon.hover();await page.mouse.down();
  assert.equal(await page.locator('.phone-home-dock').getAttribute('data-glass-pressed'), '', 'dock receives light from its child control');
  assert.equal(await dockIcon.locator('.phone-app-art').getAttribute('data-glass-motion'), '');
  await page.mouse.up();
  await page.locator('.chat-app-navigation button').first().click();
  const navButton = page.locator('.chat-app-navigation button').last();
  await navButton.hover();await page.mouse.down();
  assert.equal(await navButton.getAttribute('data-glass-pressed'), '', 'focus leaving the previous control does not cancel the new press');
  await page.mouse.up();
  await page.locator('#plain').click();
  assert.equal(await page.locator('#plain').getAttribute('data-glass-control'), null, 'ordinary content is left alone');
  await page.getByRole('button', { name: 'Floating', exact: true }).click();
  assert.equal(await page.locator('.floating').evaluate(el => getComputedStyle(el).position), 'absolute', 'shared light preserves floating material positioning');

  await page.emulateMedia({ reducedMotion: 'reduce' });
  await torch.hover();await page.mouse.down();await page.waitForTimeout(120);
  assert.equal(await torch.evaluate(el => getComputedStyle(el).scale), 'none', 'reduced motion omits flex and rebound');
  assert.equal(await torch.evaluate(el => getComputedStyle(el, '::after').opacity), '1', 'reduced motion retains immediate visual feedback');
  await page.mouse.up();
  assert.equal(await torch.evaluate(el => getComputedStyle(el, '::after').opacity), '0');
  assert.deepEqual(errors, []);
  console.log('Liquid Glass: local light, press/rebound frames, shared dock light, cancellation, rapid clicks, keyboard, focus and reduced motion passed.');
} finally { await browser.close(); }
