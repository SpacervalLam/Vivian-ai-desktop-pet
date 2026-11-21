// @test-environment live-desktop
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile, mkdir } from 'node:fs/promises';
import path from 'node:path';
import { chromium } from 'playwright-core';

const built = await build({
  stdin: { contents: `import React from 'react';import{createRoot}from'react-dom/client';import ChatWindow from './src/components/ChatWindow';import './src/styles/global.css';createRoot(document.getElementById('root')).render(<ChatWindow/>);`, resolveDir: process.cwd(), loader: 'tsx' },
  bundle: true, write: false, outfile: 'phone-test.js', format: 'esm', jsx: 'automatic', external: ['/wallpapers/*'],
  plugins: [{ name: 'phone-native-fixture', setup(b) {
    b.onResolve({ filter: /^@tauri-apps\// }, a => ({ path: a.path, namespace: 'native' }));
    b.onLoad({ filter: /.*/, namespace: 'native' }, () => ({ contents: `
      export const invoke=(...args)=>window.nativeInvoke(...args);
      export const convertFileSrc=p=>p;
      export const emit=async(name,payload)=>window.nativeCalls.push({command:'emit',name,payload});
      export const listen=async(name,callback)=>{(window.nativeListeners[name]??=new Set()).add(callback);return()=>window.nativeListeners[name].delete(callback)};
      export const getCurrentWindow=()=>({label:'chat',startDragging:async()=>window.nativeCalls.push({command:'start_dragging'}),isVisible:async()=>true,onDragDropEvent:async()=>()=>{},onFocusChanged:async()=>()=>{},setFocus:async()=>{},hide:async()=>{}});
      export class WebviewWindow {static getByLabel=async()=>null;}
      export const open=async(options)=>window.nativeInvoke('open_file_dialog',options);
    ` }));
  } }],
});
const js = built.outputFiles.find(f => f.path.endsWith('.js')).text;
const css = built.outputFiles.find(f => f.path.endsWith('.css')).text;
const browser = await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 650, height: 920 } });
  await page.clock.install();
  const errors = [];
  if (process.env.PHONE_SCREENSHOTS) await mkdir(process.env.PHONE_SCREENSHOTS, { recursive: true });
  page.on('pageerror', e => errors.push(e.message));
  await page.route('https://phone-ui.test/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname === '/') return route.fulfill({ contentType: 'text/html', body: `<!doctype html><meta charset="utf-8"><style>${css}#root{width:390px;height:846px;margin:30px auto}body{background:#ddd}</style><div id="root"></div><script>
      localStorage.setItem('vivian_chat_last_seen',JSON.stringify({vivian:1}));
      localStorage.setItem('i18nextLng','zh-CN');
      window.nativeCalls=[];window.nativeListeners={};
      window.shortcuts=[{id:'web',name:'GitHub',kind:'website',target:'https://github.com',group:'工作'},{id:'app',name:'计算器',kind:'application',target:'calc.exe',group:''}];
      window.nativeInvoke=async(command,args)=>{window.nativeCalls.push({command,args});
        if(command==='companion_desktop_context' && !args?.refreshWeather)return {shortcuts:window.shortcuts,city:'西安市长安区',weather:null};
        if(command==='companion_desktop_context' && args?.refreshWeather)await new Promise(resolve=>setTimeout(resolve,150));
        if(command==='companion_desktop_context')return{shortcuts:window.shortcuts,city:'西安市长安区',weather:{temperature:12,feels_like:10,description:'多云',humidity:58,wind_speed:14,weather_code:2,is_day:true,sunrise_hour:6.7,sunset_hour:17.9,weather_source:'Open-Meteo',cached_at:Math.floor(Date.now()/1000)-900,sources:[{temp_max:14,temp_min:3,humidity_pct:58}],air_quality:{aqi:42,pm2_5_ug_m3:18,standard:'CN',source:'Open-Meteo'}}};
        if(command==='companion_save_shortcuts'){if(window.failSave)throw Error('保存失败');window.shortcuts=args.shortcuts;return;}
        if(command==='companion_shortcut_icon')return 'data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">'+(args.shortcutId==='web'?'<circle cx="32" cy="32" r="22" fill="black"/>':'<rect width="64" height="64" fill="rgb(240,220,40)"/>')+'</svg>');
        if(command==='list_characters')return{active_id:'vivian',characters:[{id:'vivian',name:'Vivian',online:true},{id:'nana',name:'Nana',online:true}]};
        if(command==='list_chat_partners')return{characters:[]};
        if(command==='get_latest_previews')return{previews:[{id:'latest',character_id:'vivian',character_name:'Vivian',role:'assistant',content:'今天也想和你聊聊天。',timestamp:Date.now(),metadata:{channel:'wechat'}},{id:'nana-latest',character_id:'nana',character_name:'Nana',role:'assistant',content:'一起去看看城市的晚霞吧。',timestamp:Date.now()-60000,metadata:{channel:'wechat'}}],unread:{vivian:2,nana:1}};
        if(command==='get_config')return args?.key==='base.theme'?(window.themeSetting||'light'):null;
        if(command==='save_temp_image')return 'C:/Temp/gallery-share.webp';
        if(command==='open_file_dialog')return window.dialogResult??null;
        if(command==='import_gallery_images'){window.imported=[{name:'upload.png',path:'data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="red"/></svg>'),modified:Date.now()/1000}];return window.imported.map(item=>item.path);}
        if(command==='companion_now_playing')return{music:null};
        if(command==='companion_status')return{quiet_reason:'focus'};
        if(command==='get_system_mute')return true;
        if(command==='list_gallery_images')return [...(window.imported||[]),...Array.from({length:5},(_,i)=>({name:'chat-'+(i+1)+'.png',path:'data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120"><rect width="120" height="120" fill="hsl('+(i*62+200)+' 55% 55%)"/><text x="60" y="72" text-anchor="middle" fill="white" font-size="30">'+(i+1)+'</text></svg>'),modified:Date.now()/1000-i*60}))];
        if(command==='get_recognition_status')return false;
        if(command==='get_chat_history')return[{id:'h1',role:'assistant',content:'天色慢慢暗下来了，城市的灯刚好亮起。',timestamp:Date.now()-60000,metadata:{channel:'wechat'}},{id:'h2',role:'user',content:'一起看看曼哈顿的晚霞吧。',timestamp:Date.now()-30000,metadata:{channel:'wechat'}}];
        if(command==='get_user_avatar_data_url')return null;
        if(command==='get_avatar_url')return '/icons/charaters/vivian.webp';
        return[];
      };
      window.dispatchNative=(name,payload)=>{for(const callback of window.nativeListeners[name]??[])callback({payload})};
      </script><script type="module">${js}</script>` });
    try { return await route.fulfill({ contentType: url.pathname.endsWith('.svg') ? 'image/svg+xml' : 'image/webp', body: await readFile((process.env.PHONE_ASSET_ROOT ?? 'public') + url.pathname) }); }
    catch { return route.fulfill({ status: 404, body: '' }); }
  });
  await page.mouse.move(0, 0); await page.clock.fastForward(30_000);
  await page.goto('https://phone-ui.test/');
  const home = page.locator('.phone-home');
  await home.waitFor();
  await page.waitForFunction(() => window.nativeListeners['chatwindow:navigate']?.size);
  await page.locator('.chat-device-screen.is-locked').waitFor();
  await page.mouse.move(320, 300);
  await page.locator('.phone-home.is-awake').waitFor();
  await page.mouse.move(0, 0);
  await page.clock.fastForward(29_000);
  assert.equal(await page.locator('.phone-home.is-awake').count(), 1, 'leaving does not lock before 30 seconds');
  await page.mouse.move(320, 300);
  await page.clock.fastForward(2_000);
  assert.equal(await page.locator('.phone-home.is-awake').count(), 1, 'reentering cancels the pending lock');
  await page.mouse.move(0, 0);
  await page.clock.fastForward(30_000);
  await page.locator('.phone-home.is-asleep').waitFor();
  await page.mouse.move(320, 300);
  for (const asset of ['/wallpapers/riverside-original-enhanced.webp', '/wallpapers/manhattan-dusk.webp']) {
    const width = await page.evaluate(url => new Promise((resolve, reject) => {
      const picture = new Image(); picture.onload = () => resolve(picture.naturalWidth);
      picture.onerror = () => reject(new Error('Image cannot load: ' + url)); picture.src = url;
    }), asset);
    assert.ok(width > 0, `${asset} decodes from the served asset directory`);
  }
  await page.getByRole('img', { name: '系统已静音' }).waitFor();
  await page.evaluate(() => window.dispatchNative('system:volume-changed', { muted: false }));
  await page.getByRole('img', { name: '系统已静音' }).waitFor({ state: 'detached' });
  await page.evaluate(() => window.dispatchNative('system:volume-changed', { muted: true }));
  await page.getByRole('img', { name: '系统已静音' }).waitFor();
  await page.locator('.phone-focus-pill').getByText('专注').waitFor();
  await page.locator('.chat-phone-status > span').first().click();
  assert.ok(await page.evaluate(() => window.nativeCalls.some(c => c.command === 'start_dragging')), 'lock status content starts native window dragging');
  assert.equal(await page.locator('.phone-home.is-lock-screen:visible').count(), 1);
  const controls = await page.locator('.phone-lock-action').evaluateAll(nodes => nodes.map(el => { const r=el.getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width}; }));
  await page.locator('.phone-focus-pill').evaluate(el => { el.style.display = 'none'; });
  const noPill = await page.locator('.phone-lock-action').evaluateAll(nodes => nodes.map(el => { const r=el.getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width}; }));
  assert.deepEqual(noPill, controls, 'camera and flashlight keep their columns without the focus pill');
  const brightDate = await page.locator('.phone-home-date').boundingBox();
  const brightTime = await page.locator('.phone-home-time').boundingBox();
  await page.mouse.move(0, 0); await page.clock.fastForward(30_000);
  await page.locator('.phone-home.is-asleep').waitFor();
  assert.deepEqual(await page.locator('.phone-home-date').boundingBox(), brightDate, 'dim date keeps its exact layout');
  assert.deepEqual(await page.locator('.phone-home-time').boundingBox(), brightTime, 'dim clock keeps its exact layout');
  await page.mouse.move(320, 300);

  await page.getByRole('button', { name: '打开闪光灯' }).click();
  await page.getByRole('button', { name: '关闭闪光灯' }).waitFor();
  assert.equal(await page.getByRole('button', { name: '关闭闪光灯' }).getAttribute('aria-pressed'), 'true');
  await page.getByRole('button', { name: '关闭闪光灯' }).click();
  await page.getByRole('button', { name: '打开闪光灯' }).waitFor();
  assert.ok(await page.evaluate(() => window.nativeCalls.some(c => c.command === 'set_system_flashlight' && c.args?.enabled === true)), 'flashlight drives the native torch');
  assert.ok(await page.getByRole('button', { name: '打开相机' }).count(), 'lock screen camera is a real button');
  assert.ok(await page.locator('.chat-phone-wallpaper').evaluate(el => getComputedStyle(el).backgroundImage.includes('riverside-original-enhanced.webp')));
  assert.equal(await page.locator('.phone-lock-notification:visible').count(), 2, 'bright lock screen shows the latest notification plus a real second card');
  assert.equal(await page.locator('.phone-home-unlocked:visible').count(), 0, 'hovering wakes without unlocking');
  if (process.env.PHONE_CLOCK_ONLY) {
    const clock = page.locator('.phone-home-time');
    const expanded = (await clock.boundingBox()).height;
    await page.mouse.move(250, 150);
    await page.evaluate(() => {
      window.clockFrames = [];
      const start = performance.now();
      const sample = () => {
        window.clockFrames.push(document.querySelector('.phone-home-time').getBoundingClientRect().height);
        if (performance.now() - start < 450) requestAnimationFrame(sample);
      };
      sample();
      document.querySelector('.phone-home').dispatchEvent(new WheelEvent('wheel', { deltaY: 80, bubbles: true }));
    });
    await page.waitForFunction(() => window.clockFrames.length > 18);
    const partial = (await clock.boundingBox()).height;
    const frames = await page.evaluate(() => window.clockFrames);
    assert.ok(new Set(frames.map(Math.round)).size > 4, 'clock renders intermediate compression frames');
    assert.ok(partial < expanded && partial > expanded * .6, 'small wheel movement only partly compresses the clock');
    await page.mouse.wheel(0, 120);
    await page.waitForTimeout(260);
    const further = (await clock.boundingBox()).height;
    assert.ok(further < partial - 20, 'more wheel travel further compresses the digits');
    await page.mouse.wheel(0, -60);
    await page.waitForTimeout(260);
    assert.ok((await clock.boundingBox()).height > further + 20, 'reverse wheel travel gradually restores height');
    await page.getByRole('button', { name: '收起通知中心' }).click();
    await page.waitForTimeout(260);
    assert.ok(Math.abs((await clock.boundingBox()).height - expanded) < 2);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    assert.equal(await clock.evaluate(el => getComputedStyle(el).transitionDuration), '0s');
    assert.deepEqual(errors, []);
    console.log('Clock scroll: proportional height, intermediate frames, reverse expansion, collapse and reduced motion passed.');
  } else {
  const tallClock = await page.locator('.phone-home-time').boundingBox();
  if (process.env.PHONE_SCREENSHOTS) await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-bright-lock.png') });
  await page.mouse.wheel(0, 200);
  await page.locator('.phone-home.is-notification-center').waitFor();
  await page.waitForTimeout(450);
  const compactClock = await page.locator('.phone-home-time').boundingBox();
  assert.ok(tallClock.height > compactClock.height * 1.7, 'browsing notifications compacts the time');
  assert.equal(await page.locator('.phone-lock-notification:visible').count(), 2);
  assert.ok(await page.locator('.phone-notification-backdrop').evaluate(el => {
    const style = getComputedStyle(el);
    return style.opacity === '1' && style.backdropFilter.includes('blur(30px)');
  }), 'notification center blurs the whole wallpaper');
  if (process.env.PHONE_SCREENSHOTS) await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-notification-center.png') });
  await page.evaluate(() => { for (let i = 0; i < 6; i++) window.dispatchNative('chat:assistant_message', { character_id: 'friend-' + i, channel: 'wechat', content: '通知滚轮测试 ' + i }); });
  await page.mouse.move(300, 440);
  await page.mouse.wheel(0, 380);
  await page.waitForFunction(() => document.querySelector('.phone-notification-scroll').scrollTop > 0);
  await page.getByRole('button', { name: '打开消息：Nana' }).click();
  await page.locator('.chat-phone-app:not([hidden])').waitFor();
  assert.equal(await home.isVisible(), false, 'a notification opens its destination directly');
  await page.getByRole('button', { name: '返回手机主屏' }).click();
  await home.waitFor({ state: 'visible' });
  await page.mouse.move(0, 0); await page.clock.fastForward(30_000);
  await page.locator('.phone-home.is-asleep').waitFor();
  assert.equal(await page.locator('.phone-lock-notification:visible').count(), 0, 'dark screen hides every notification');
  assert.ok(await page.locator('.chat-phone-sleep-veil').evaluate(el => getComputedStyle(el).backdropFilter.includes('28px')), 'dark screen adds a strong blur veil');
  if (process.env.PHONE_SCREENSHOTS) { await page.waitForTimeout(600); await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-dark-lock.png') }); }
  await page.mouse.move(320, 300);
  await page.locator('.phone-home.is-awake.is-lock-screen').waitFor();
  await page.locator('.phone-home-date').click();
  await page.locator('.phone-home.is-home-screen').waitFor();
  await page.getByRole('button', { name: 'GitHub', exact: true }).waitFor();
  assert.equal(await page.locator('.phone-home-widgets button').count(), 2, 'home has weather and photo widgets');
  assert.equal(await page.locator('.phone-home-time:visible').count(), 0, 'home is separate from the lock-screen clock');
  assert.equal(await page.getByRole('button', { name: 'LINE', exact: true }).count(), 1);
  assert.equal(await page.getByRole('button', { name: '快捷启动', exact: true }).count(), 0);
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.phone-app-shortcut[title="https://github.com"] .phone-app-art')).backgroundColor === 'rgb(255, 255, 255)');
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.phone-app-shortcut[title="calc.exe"] .phone-app-art')).backgroundColor === 'rgb(240, 220, 40)');

  // Weather and Clock are in-phone pages, not external dialogs.
  await page.getByRole('button', { name: '天气', exact: true }).click();
  await page.locator('.phone-weather-page').waitFor();
  assert.ok(await page.evaluate(() => window.nativeCalls.some(call => call.command === 'companion_desktop_context' && call.args?.refreshWeather === true)), 'opening weather requests fresh data');
  await page.locator('.phone-weather-card').first().waitFor();
  assert.ok(await page.locator('.phone-weather-card').count() >= 6, 'weather page shows all detail cards');
  assert.equal(await page.locator('.phone-app-page-title').textContent(), '西安市长安区');
  assert.ok(await page.getByRole('button', {name:'天气设置',exact:true}).count());
  const updateLabel = await page.locator('.phone-weather-source').textContent();
  assert.ok(Number(updateLabel.match(/(\d+) 分钟/)?.[1]) < 100, 'cached weather age is minutes, not seconds');
  if (process.env.PHONE_SCREENSHOTS) { await page.waitForTimeout(380); await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-weather.png') }); }
  await page.getByRole('button', { name: '返回主屏', exact: true }).click();
  await page.locator('.phone-weather-page').waitFor({ state: 'detached' });
  await page.getByRole('button', { name: '时钟', exact: true }).click();
  await page.locator('.phone-clock-page').waitFor();
  assert.equal(await page.locator('.phone-world-clock-row').count(), 4, 'clock page shows the world clock list');
  await page.getByRole('button',{name:'切换时间格式',exact:true}).click();
  assert.equal(await page.getByRole('button',{name:'切换时间格式',exact:true}).getAttribute('aria-pressed'),'true');
  assert.equal(await page.locator('.phone-clock-digital em').count(),1);
  await page.getByRole('button',{name:'切换时间格式',exact:true}).click();
  assert.equal(await page.locator('.phone-world-clock-city').first().evaluate(el=>getComputedStyle(el).color),'rgb(17, 17, 20)', 'world-clock city names remain legible in light mode');
  if (process.env.PHONE_SCREENSHOTS) { await page.waitForTimeout(380); await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-clock.png') }); }
  await page.getByRole('button', { name: '返回主屏', exact: true }).click();
  await page.locator('.phone-clock-page').waitFor({ state: 'detached' });

  // The Photos widget opens an in-phone gallery with chat photos and bundled wallpapers.
  await page.locator('.phone-photo-widget').click();
  await page.locator('.phone-gallery-page').waitFor();
  await page.waitForFunction(() => document.querySelectorAll('.phone-gallery-cell').length === 7);
  await page.getByRole('button',{name:'导入照片',exact:true}).click();
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('.phone-gallery-menu').count(),0,'Escape dismisses the menu while retaining the gallery');
  await page.getByRole('button',{name:'导入照片',exact:true}).click();
  await page.getByRole('button',{name:'导入整个文件夹',exact:true}).click();
  assert.ok(await page.evaluate(()=>window.nativeCalls.some(call=>call.command==='open_file_dialog' && call.args.directory===true)), 'gallery can select an entire source folder');
  assert.equal(await page.locator('.phone-gallery-cell').count(), 7, 'gallery combines chat photos and wallpapers');
  if (process.env.PHONE_SCREENSHOTS) { await page.waitForTimeout(380); await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-gallery.png') }); }
  await page.getByRole('button', { name: '精选集', exact: true }).click();
  await page.locator('.phone-gallery-collections').waitFor();
  if (process.env.PHONE_SCREENSHOTS) { await page.waitForTimeout(380); await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-collections.png') }); }
  await page.getByRole('button', { name: '壁纸', exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll('.phone-gallery-cell').length === 2);
  assert.equal(await page.locator('.phone-gallery-cell').count(), 2, 'wallpaper album only shows bundled wallpapers');
  await page.getByRole('button', { name: '选择', exact: true }).click();
  await page.getByRole('toolbar', { name: '照片选择操作' }).waitFor();
  assert.equal(await page.locator('.phone-gallery-tabbar').count(), 0, 'selection replaces bottom navigation');
  assert.equal(await page.getByRole('button', { name: '分享所选照片', exact: true }).isDisabled(), true);
  await page.getByRole('button', { name: '全选', exact: true }).click();
  assert.equal(await page.locator('.phone-gallery-check svg').count(), 2, 'album supports select all');
  await page.getByRole('button', { name: '取消全选', exact: true }).click();
  await page.locator('.phone-gallery-cell').first().click();
  assert.equal(await page.locator('.phone-gallery-check svg').count(), 1, 'select mode marks a photo');
  assert.equal(await page.getByRole('button', { name: '分享所选照片', exact: true }).isEnabled(), true);
  if (process.env.PHONE_SCREENSHOTS) await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-gallery-selection.png') });
  await page.getByRole('button', { name: '分享所选照片', exact: true }).click();
  await page.getByRole('dialog', { name: '分享照片', exact: true }).waitFor();
  await page.getByRole('button', { name: '发送给 Vivian', exact: true }).waitFor();
  assert.equal(await page.locator('.phone-gallery-action-sheet a[download]').count(), 0, 'sharing targets LINE roles instead of downloading');
  await page.getByRole('button', { name: '取消', exact: true }).click();
  await page.getByRole('button', { name: '取消选择', exact: true }).click();
  await page.locator('.phone-gallery-cell').first().click();
  await page.locator('.phone-gallery-viewer').waitFor();
  assert.equal(await page.locator('.phone-gallery-viewer').getByRole('button',{name:'LINE',exact:true}).count(),1,'viewer offers direct LINE sharing');
  const picture = await page.locator('.phone-gallery-viewer > img').boundingBox();
  await page.mouse.move(picture.x+picture.width*.75,picture.y+picture.height*.5);await page.mouse.down();await page.mouse.move(picture.x+picture.width*.25,picture.y+picture.height*.5,{steps:8});await page.mouse.up();
  assert.ok((await page.locator('.phone-gallery-viewer-bar').textContent()).includes('2 / 2'),'swiping changes the viewed image');
  await page.keyboard.press('Escape');
  await page.locator('.phone-gallery-viewer').waitFor({ state: 'detached' });
  await page.evaluate(() => { window.themeSetting='dark';window.dispatchNative('config:theme-changed', {theme:'dark'}); });
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  assert.equal(await page.locator('.phone-gallery-page').evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(9, 9, 11)');
  await page.evaluate(() => { window.themeSetting='light';window.dispatchNative('config:theme-changed', {theme:'light'}); });
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light');
  assert.equal(await page.locator('.phone-gallery-page').evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(255, 255, 255)');
  await page.getByRole('button', { name: '选择', exact: true }).click();
  await page.getByRole('button', { name: '全选', exact: true }).click();
  await page.getByRole('button', { name: '分享所选照片', exact: true }).click();
  await page.getByRole('button', { name: '发送给 Vivian', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('.phone-gallery-page') || document.querySelector('.phone-gallery-action-sheet [role="alert"]'));
  assert.equal(await page.locator('.phone-gallery-action-sheet [role="alert"]').count(),0, await page.locator('.phone-gallery-action-sheet').textContent().catch(()=> 'share complete'));
  await page.locator('.phone-gallery-page').waitFor({ state: 'detached' });
  assert.equal(await page.evaluate(() => window.nativeCalls.filter(c => c.command==='send_image_message' && c.args.characterId==='vivian' && c.args.channel==='wechat').length), 2, 'all selected wallpapers use the LINE image pipeline');
  await page.getByRole('button', { name: '返回手机主屏' }).click();


  const enterHomeEdit = async () => {
    if (await page.getByRole('toolbar', { name: '编辑主屏' }).isVisible()) return;
    const bounds = await page.getByRole('button', {name:'时钟',exact:true}).boundingBox();
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
    await page.mouse.down(); await page.waitForTimeout(680); await page.mouse.up();
    await page.getByRole('toolbar', { name: '编辑主屏' }).waitFor();
  };
  assert.equal(await page.getByRole('button', { name: '添加好友', exact: true }).count(), 0);
  assert.equal(await page.getByRole('button', { name: '添加 App', exact: true }).count(), 0);
  assert.deepEqual(await page.locator('.phone-home-dock .phone-app').evaluateAll(nodes => nodes.map(el => el.getAttribute('aria-label'))), ['LINE','设置']);
  const searchBox = page.getByRole('textbox', { name: '搜索 App' });
  const minimumWidth = (await page.locator('.phone-app-search').boundingBox()).width;
  await searchBox.fill('a longer app search');
  assert.ok((await page.locator('.phone-app-search').boundingBox()).width > minimumWidth + 30);
  await searchBox.fill('long '.repeat(24));
  const wideSearch = await page.locator('.phone-app-search').boundingBox();
  const homeBounds = await home.boundingBox();
  assert.ok(wideSearch.width <= homeBounds.width && wideSearch.x >= homeBounds.x, 'search stays inside the screen');
  await searchBox.fill('');
  const previousPhoto = await page.locator('.phone-photo-widget img').getAttribute('src');
  await page.waitForFunction(src=>document.querySelector('.phone-photo-widget img')?.getAttribute('src')!==src,previousPhoto,{timeout:10000});
  await enterHomeEdit();
  // Existing shortcut data becomes app icons. Editing, grouping, normalizing URLs and deletion still work.
  const clockItem = page.locator('.phone-home-layout [data-home-item="clock"]');
  const weatherItem = page.locator('[data-home-item="weather"]');
  const start = await clockItem.boundingBox(), destination = await weatherItem.boundingBox();
  await page.mouse.move(start.x+start.width/2,start.y+start.height/2);await page.mouse.down();
  await page.mouse.move(destination.x+destination.width/2,destination.y+destination.height/2,{steps:12});await page.mouse.up();
  assert.ok(await page.evaluate(()=>JSON.parse(localStorage.getItem('phone-home-layout')).indexOf('clock') < JSON.parse(localStorage.getItem('phone-home-layout')).indexOf('weather')), 'dragging can move an icon before a widget');
  await page.evaluate(()=>window.dialogResult=['C:/Photos/custom.png']);
  await page.getByRole('button',{name:'自定义壁纸',exact:true}).click();
  await page.waitForFunction(()=>localStorage.getItem('phone-wallpaper')?.startsWith('data:image'));
  await page.getByRole('button',{name:'还原壁纸',exact:true}).click();
  assert.equal(await page.evaluate(()=>localStorage.getItem('phone-wallpaper')),null);
  await enterHomeEdit();
  await page.getByRole('button', { name: '添加快捷方式', exact: true }).click();
  await page.getByLabel('名称', { exact: true }).fill('文档');
  await page.getByLabel('网址', { exact: true }).fill('example.com');
  await page.getByLabel('分组（可选）').fill('工作');
  await page.mouse.move(0, 0); await page.clock.fastForward(30_000);
  await page.locator('.phone-home.is-asleep').waitFor();
  await page.mouse.move(320, 300);
  await page.locator('.phone-home.is-awake.is-lock-screen').waitFor();
  assert.equal(await page.getByRole('dialog').count(), 0, 'an app editor cannot cover the lock screen');
  await page.locator('.phone-home-date').click();
  assert.equal(await page.getByLabel('名称', { exact: true }).inputValue(), '文档', 'unlocking retains the editor draft');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.getByRole('dialog').waitFor({ state: 'hidden' });
  await enterHomeEdit();
  await page.getByRole('button', { name: '编辑 文档', exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.shortcuts.find(s => s.name === '文档').target), 'https://example.com');
  await page.getByRole('button', { name: '编辑 GitHub', exact: true }).click({ button: 'right' });
  assert.equal(await page.getByLabel('名称', { exact: true }).inputValue(), 'GitHub');
  await page.getByLabel('名称', { exact: true }).fill('GitHub 工作');
  await page.evaluate(() => window.failSave = true);
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.getByRole('dialog').getByRole('alert').waitFor();
  assert.equal(await page.getByLabel('名称', { exact: true }).inputValue(), 'GitHub 工作', 'failed save keeps edits');
  await page.evaluate(() => window.failSave = false);
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.getByRole('button', { name: '编辑 GitHub 工作', exact: true }).waitFor();
  await page.getByRole('textbox', { name: '搜索 App' }).fill('工作 github');
  assert.equal(await page.locator('.phone-app-shortcut').count(), 1);
  await page.getByRole('textbox', { name: '搜索 App' }).fill('');
  await page.getByRole('button', { name: '编辑 文档', exact: true }).click({ button: 'right' });
  await page.getByRole('button', { name: '删除', exact: true }).click();
  await page.getByRole('button', { name: '确认删除', exact: true }).click();
  await page.getByRole('dialog').waitFor({ state: 'hidden' });
  assert.equal(await page.getByRole('button', { name: '编辑 文档', exact: true }).count(), 0);

  await page.getByRole('button', { name: '完成', exact: true }).click();
  // The LINE app and external deep links still enter conversations. Dimming preserves drafts.
  await page.getByRole('button', { name: 'LINE', exact: true }).click();
  await page.locator('.chat-phone-app:not([hidden])').waitFor();
  assert.equal(await page.locator('.chat-page-departing').count(), 0, 'opening LINE never animates a snapshot of the hidden app');
  if (process.env.PHONE_SCREENSHOTS) await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-line.png') });
  await page.evaluate(() => window.dispatchNative('chatwindow:navigate', { view: 'private', character_id: 'vivian' }));
  const draft = page.locator('.chat-phone-app textarea').last();
  await draft.fill('还没发出的草稿');
  await page.locator('.chat-thread-bubble.is-user').waitFor();
  await page.locator('.chat-page-departing').waitFor({ state: 'hidden' });
  if (process.env.PHONE_SCREENSHOTS) await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-conversation.png') });
  await page.evaluate(() => { window.themeSetting='dark';window.dispatchNative('config:theme-changed', { theme: 'dark' }); });
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  assert.equal(await page.locator('.chat-device-screen').evaluate(el => getComputedStyle(el).getPropertyValue('--ios-content').trim()), '#0b0b0d');
  await page.evaluate(() => { window.themeSetting='light';window.dispatchNative('config:theme-changed', { theme: 'light' }); });
  await page.mouse.move(0, 0); await page.clock.fastForward(30_000);
  await page.locator('.chat-device-screen.is-locked').waitFor();
  assert.equal(await home.isVisible(), true);
  await page.evaluate(() => window.dispatchNative('chat:assistant_message', { character_id: 'vivian', channel: 'wechat', content: '回来后接着聊。' }));
  await page.mouse.move(320, 300);
  await page.locator('.phone-home.is-awake.is-lock-screen').waitFor();
  await page.getByRole('button', { name: '打开消息：Vivian' }).click();
  await home.waitFor({ state: 'hidden' });
  assert.equal(await draft.inputValue(), '还没发出的草稿');
  await page.getByRole('button', { name: '返回手机主屏' }).click();
  await home.waitFor({ state: 'visible' });
  await page.evaluate(() => window.dispatchNative('chatwindow:navigate', { view: 'assistant', panel: 'shortcuts' }));
  assert.equal(await home.isVisible(), true, 'old launcher links land on the phone home');

  await page.getByRole('button', { name: '计算器', exact: true }).click();
  const launched = await page.evaluate(() => window.nativeCalls.find(c => c.command === 'companion_launch_shortcut'));
  assert.deepEqual(launched.args, { shortcutId: 'app', characterId: 'vivian' });
  assert.ok(await page.evaluate(() => window.nativeCalls.some(c => c.command === 'close_chat_window')));

  if (process.env.PHONE_SCREENSHOTS) {
    await page.locator('.phone-home-apps').evaluate(el => { el.scrollTop = 0; });
    await page.evaluate(() => { for (let i = 0; i < 3; i++) window.dispatchNative('chat:assistant_message', { character_id: 'vivian', channel: 'wechat', content: '今天也想和你聊聊天。' }); });
    await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-home.png') });
    await page.mouse.move(0, 0); await page.clock.fastForward(30_000);
    await page.waitForTimeout(800);
    await page.locator('.chat-device').screenshot({ path: path.join(process.env.PHONE_SCREENSHOTS, 'phone-dark-lock-final.png') });
  }
  await page.evaluate(() => Object.assign(document.getElementById('root').style, { width: '300px', height: '650px' }));
  await page.mouse.move(320, 300);
  if (await page.locator('.phone-home-date').isVisible()) await page.locator('.phone-home-date').click();
  await enterHomeEdit();
  await page.getByRole('button', { name: '添加快捷方式', exact: true }).click();
  const formBox = await page.getByRole('dialog').boundingBox();
  const screenBox = await page.locator('.chat-device-screen').boundingBox();
  assert.ok(formBox.x >= screenBox.x && formBox.x + formBox.width <= screenBox.x + screenBox.width + 1, 'editor fits a smaller phone');
  assert.ok(formBox.y >= screenBox.y && formBox.y + formBox.height <= screenBox.y + screenBox.height + 1);
  await page.keyboard.press('Escape');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.mouse.move(330, 310);
  assert.equal(await page.locator('.chat-phone-wallpaper').evaluate(el => getComputedStyle(el).transform), 'none', 'reduced motion disables wallpaper parallax');
  assert.deepEqual(errors, [], 'phone navigation must not throw browser errors');
  console.log('Phone home: wake/lock, LINE navigation, draft preservation, migrated app CRUD/search and native launch passed.');
  }
} finally { await browser.close(); }
