// Run: node scripts/preview-four-pets.mjs (Ctrl+C closes all preview windows).
import { createServer } from 'node:http';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../', import.meta.url));
const vocab = JSON.parse(await readFile(join(root, 'src/chibi/animations.json'), 'utf8'));
const anchors = JSON.parse(await readFile(join(root, 'src/chibi/frameAnchors.json'), 'utf8'));
const scales = JSON.parse(await readFile(join(root, 'src/chibi/frameScales.json'), 'utf8'));
const size = JSON.parse(await readFile(join(root, 'src/chibi/windowSize.json'), 'utf8'));
const ids = vocab.characters;
const html = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>桌宠动作控制台</title>
<style>*{box-sizing:border-box}body{margin:0;font:14px system-ui;background:#f5f2f8;color:#40364e;padding:20px}h1{font-size:21px}button,select,input{font:inherit;margin:4px;padding:8px 12px;border:1px solid #d7ccdf;border-radius:8px;background:white}button{cursor:pointer}button:hover{background:#eee5f6}fieldset{border:1px solid #d7ccdf;border-radius:12px;margin:16px 0}#status{color:#776886;white-space:pre-line}.pet{padding:0;overflow:hidden;background:transparent}.pet #console{display:none}.pet #viewport{width:100vw;height:100vh;display:flex;align-items:flex-end;justify-content:center;overflow:hidden}.stage{position:relative;width:min(96vw,94vh);aspect-ratio:1}.sprite{position:absolute;inset:0;background-repeat:no-repeat;filter:drop-shadow(0 4px 4px #30203d25)}.grid{background:repeating-conic-gradient(#e9e4ef 0 25%,#fff 0 50%) 50%/16px 16px!important}.dark{background:#282633!important}.light{background:#f5f2f8!important}</style>
<section id="console"><h1>四桌宠 · 动作控制台</h1><p>独立素材预览，动作与节奏读取现有动画配置。</p><fieldset><legend>目标角色</legend><label><input type="checkbox" value="all" id="all" checked>全部</label><span id="targets"></span></fieldset><label>方向 <select id="direction"><option value="left">左</option><option value="right">右</option></select></label><label>速度 <select id="speed"><option value="0.5">0.5×</option><option value="1" selected>1×</option><option value="2">2×</option></select></label><label>背景 <select id="background"><option value="grid">棋盘格</option><option value="light">浅色</option><option value="dark">深色</option></select></label><div><button id="reset">回到待机</button><button id="pause">暂停 / 继续</button></div><fieldset><legend>动作</legend><div id="actions"></div></fieldset><p id="status">正在等待桌宠窗口…</p></section><div id="viewport" hidden><div class="stage"><div class="sprite"></div></div></div>
<script>
const vocab=${JSON.stringify(vocab)},anchors=${JSON.stringify(anchors)},scales=${JSON.stringify(scales)},id=new URLSearchParams(location.search).get('pet'),channel=new BroadcastChannel('four-pets-preview');
const q=s=>document.querySelector(s);let rate=1,paused=false,timer=0,token=0,current='idle';
function send(command){channel.postMessage({...command,targets:q('#all').checked?vocab.characters:[...document.querySelectorAll('#targets input:checked')].map(e=>e.value)});}
if(id){
 document.title=id+' · 桌宠预览';document.body.className='pet grid';q('#viewport').hidden=false;
 const sprite=q('.sprite'),loaded=new Map();
 async function load(url){if(!loaded.has(url))loaded.set(url,new Promise((ok,no)=>{const image=new Image();image.onload=ok;image.onerror=()=>{loaded.delete(url);no(new Error('缺少资源 '+url));};image.src=url;}));await loaded.get(url);}
 function paint(url,cols,rows,frame){sprite.style.backgroundImage='url("'+url+'")';sprite.style.backgroundSize=cols*100+'% '+rows*100+'%';sprite.style.backgroundPosition=(cols>1?frame%cols*100/(cols-1):0)+'% '+(rows>1?Math.floor(frame/cols)*100/(rows-1):0)+'%';const a=anchors[url]?.[frame];sprite.style.translate=a?a[0]+'% '+a[1]+'%':'';const c=scales[url]?.[frame];sprite.style.scale=c?String(c[0]):'';sprite.style.transformOrigin=c?'50% '+c[1]+'%':'';}
 async function play(name,direction='left'){
  const run=++token;clearTimeout(timer);current=name;
  const spec=vocab.animations.find(s=>s.name===name&&(!s.characters||s.characters.includes(id)));
  const pose=vocab.atlas.poses.findIndex(s=>s.name===name);
  if(!spec){let url='/chibi/'+id+'-atlas.webp',cols=vocab.atlas.cols,rows=vocab.atlas.rows,slot=Math.max(0,pose);if(name==='idle'&&id!=='nana'){url='/chibi/motion/'+id+'/'+id+'-idle-sheet.webp';cols=rows=1;slot=0;}await load(url);if(run===token)paint(url,cols,rows,slot);return;}
  const url=spec.sheet.replaceAll('{character}',id).replaceAll('{direction}',direction);await load(url);if(run!==token)return;
  let frame=0;function step(){if(run!==token)return;if(paused){timer=setTimeout(step,40);return;}paint(url,spec.cols,spec.rows,frame);const delay=spec.durations[frame]/rate;frame++;timer=setTimeout(()=>{if(run!==token)return;if(frame===spec.frames){if(spec.loop)frame=0;else{if(!spec.hold)play('idle').catch(report);return;}}step();},delay);}step();
 }
 function report(e){channel.postMessage({report:id+': '+e.message});}
 channel.onmessage=({data:d})=>{if(!d.targets?.includes(id))return;if(d.type==='play')play(d.name,d.direction).catch(report);if(d.type==='pause')paused=!paused;if(d.type==='settings'){rate=d.rate;document.body.className='pet '+d.background;}};
 setInterval(()=>channel.postMessage({ready:id,current}),1000);play('idle').catch(report);
}else{
 q('#viewport').remove();const ready=new Map();
 for(const id of vocab.characters){const label=document.createElement('label');label.innerHTML='<input type="checkbox" value="'+id+'">'+id;q('#targets').append(label);}
 const names=new Set();for(const s of [...vocab.atlas.poses,...vocab.animations]){if(names.has(s.name))continue;names.add(s.name);const b=document.createElement('button');b.textContent=s.label;b.dataset.motion=s.name;b.onclick=()=>send({type:'play',name:s.name,direction:q('#direction').value});q('#actions').append(b);}
 q('#reset').onclick=()=>send({type:'play',name:'idle'});q('#pause').onclick=()=>send({type:'pause'});
 function settings(){send({type:'settings',rate:Number(q('#speed').value),background:q('#background').value});}q('#speed').onchange=settings;q('#background').onchange=settings;
 channel.onmessage=({data:d})=>{if(d.report){q('#status').textContent=d.report;return;}if(d.ready){ready.set(d.ready,d.current);q('#status').textContent=vocab.characters.map(id=>id+': '+(ready.get(id)??'等待窗口')).join(' · ');}};
}
</script></html>`;

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, 'http://localhost');
    if (url.pathname === '/') { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(html); return; }
    if (!/^\/chibi\/[a-z0-9/-]+\.webp$/.test(url.pathname)) { res.writeHead(404).end(); return; }
    const file = resolve(root, 'public', '.' + url.pathname);
    res.setHeader('Content-Type', 'image/webp'); res.end(await readFile(file));
  } catch { res.writeHead(404).end(); }
});
await new Promise(ok => server.listen(0, '127.0.0.1', ok));
const origin = `http://127.0.0.1:${server.address().port}`;
const profile = await mkdtemp(join(tmpdir(), 'four-pets-'));
let context;
async function close() {
  await context?.close().catch(() => {});
  server.closeAllConnections(); await new Promise(ok => server.close(ok));
  if (resolve(profile).startsWith(resolve(tmpdir(), 'four-pets-'))) {
    await rm(profile, { recursive: true, force: true, maxRetries: 3 }).catch(() => {});
  }
}
try {
  context = await chromium.launchPersistentContext(profile, {
    channel: 'chrome', headless: false, viewport: null,
    args: ['--app=' + origin, '--no-first-run', '--disable-background-timer-throttling', '--disable-renderer-backgrounding'],
  });
  const page = context.pages()[0];
  const cdp = await context.newCDPSession(page);
  for (const [index, id] of ids.entries()) {
    const { targetId } = await cdp.send('Target.createTarget', { url: origin + '/?pet=' + id, newWindow: true });
    const { windowId } = await cdp.send('Browser.getWindowForTarget', { targetId });
    await cdp.send('Browser.setWindowBounds', { windowId, bounds: {
      left: 30 + index * 280, top: 40, width: Math.round(size.w) + 40, height: Math.round(size.h) + 100,
    } });
  }
  console.log('已打开四个桌宠窗口和动作控制台：' + origin + '\nCtrl+C 关闭全部预览窗口。');
  if (process.argv.includes('--smoke')) {
    const consolePage = context.pages().find(p => p.url() === origin + '/') ?? page;
    await consolePage.waitForSelector('[data-motion="walk"]');
    await consolePage.locator('[data-motion="walk"]').click();
    for (const id of ids) {
      const pet = context.pages().find(p => p.url().includes('pet=' + id));
      await pet.waitForFunction(() => document.querySelector('.sprite').style.backgroundImage.includes('walk-left-sheet'));
      console.log(id + ': 已播放走路动作');
    }
    await consolePage.locator('#all').uncheck();
    for (const id of ['deepseek', 'claude']) await consolePage.locator(`#targets input[value="${id}"]`).check();
    for (const spec of vocab.animations.filter(s => s.frames > 1 && (!s.characters || s.characters.includes('deepseek')))) {
      for (const direction of spec.directions ?? ['left']) {
        await consolePage.locator('#direction').selectOption(direction);
        await consolePage.locator(`[data-motion="${spec.name}"]`).click();
        for (const id of ['deepseek', 'claude']) {
          const pet = context.pages().find(p => p.url().includes('pet=' + id));
          const expected = spec.sheet.replaceAll('{character}', id).replaceAll('{direction}', direction);
          await pet.waitForFunction(url => document.querySelector('.sprite').style.backgroundImage.includes(url), expected);
        }
      }
    }
    console.log('DeepSeek / Claude: 全部连续动作均已在独立窗口触发并加载。');
    await close();
  } else {
    const done = new Promise(ok => { process.once('SIGINT', ok); process.once('SIGTERM', ok); context.once('close', ok); });
    await done; await close();
  }
} catch (error) { await close(); throw error; }
