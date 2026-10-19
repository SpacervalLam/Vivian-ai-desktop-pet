import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';

const read = path => readFileSync(new URL('../src-tauri/src/embedded_browser/scripts/' + path, import.meta.url), 'utf8');
function anchor(href, title) { return { textContent:title,getAttribute:name=>name==='href'?href:'',closest:()=>null,querySelector:()=>null }; }
function page(hostname, anchors=[], body='') {
  const context=vm.createContext({URL,location:{hostname,href:'https://'+hostname+'/explore',pathname:'/explore'},HTMLElement:class {},
    document:{querySelectorAll:()=>anchors,querySelector:()=>null,body:{textContent:body}}}, {codeGeneration:{strings:false,wasm:false}});
  vm.runInContext(read('extractors.js'),context);vm.runInContext(read('page.js'),context);
  return {context,invoke:vm.runInContext('vivianCreatePage()',context)};
}

test('image extraction returns real sources, resolves relative URLs and bounds untrusted references', async () => {
  const {context, invoke} = page('example.com');
  const image = (src, alt = '参考图', extra = {}) => Object.assign(new context.HTMLElement(), {
    currentSrc: src, naturalWidth: 400, naturalHeight: 600,
    getAttribute: name => name === 'alt' ? alt : name === 'src' ? src : null,
    getBoundingClientRect: () => ({width:400, height:600}), ...extra,
  });
  context.getComputedStyle = el => ({display: el.hidden ? 'none' : 'block', visibility:'visible', opacity:'1'});
  const images = [image('/fanart.png?token=original'), image('/fanart.png?token=original'),
    image('data:image/png;base64,secret'), image('javascript:alert(1)'),
    image('/tracking.png', '', {naturalWidth:1, naturalHeight:1}),
    image('/hidden.png', '', {hidden:true}), image('', 'empty')];
  context.document.querySelectorAll = selector => selector === 'img' ? images : [];
  const extracted = JSON.parse((await invoke('browser_extract', {extractor:'images'})).text);
  assert.deepEqual(extracted, [{url:'https://example.com/fanart.png?token=original', alt:'参考图',
    source_url:'https://example.com/explore', width:400, height:600}]);
  images.length = 0;
  assert.deepEqual(JSON.parse((await invoke('browser_extract', {extractor:'images'})).text), []);
  for (let i=0; i<45; i++) images.push(image(`/image-${i}.png`, 'x'.repeat(500)));
  const bounded = JSON.parse((await invoke('browser_extract', {extractor:'images'})).text);
  assert.equal(bounded.length, 30);
  assert.equal(bounded[0].alt.length, 200);
});
test('native packaged extraction preserves access parameters and ignores duplicates and offsite links',async()=>{
  const {invoke}=page('www.xiaohongshu.com',[
    anchor('/explore/abcdef123456?xsec_token=access&xsec_source=pc_feed','Note title'),
    anchor('/explore/abcdef123456','Duplicate'),anchor('https://evil.example/explore/abcdef999999','Offsite')]);
  const items=JSON.parse((await invoke('browser_extract',{extractor:'xiaohongshu'})).text);
  assert.equal(items.length,1);assert.equal(items[0].title,'Note title');assert.match(items[0].url,/xsec_token=access&xsec_source=pc_feed/);
});
test('extractors reject platform mismatch and arbitrary code remains unsupported',async()=>{
  const {invoke}=page('www.bilibili.com',[anchor('/video/BV1abc123','Video title')]);
  assert.equal(JSON.parse((await invoke('browser_extract',{extractor:'bilibili'})).text)[0].id,'BV1abc123');
  await assert.rejects(invoke('browser_extract',{extractor:'xiaohongshu'}),/does not match/);
  await assert.rejects(invoke('browser_eval_js',{code:'document.cookie'}),/arbitrary code/);
});
test('page text supports continuation without returning form values or cookie APIs',async()=>{
  const {invoke}=page('example.com',[],'a'.repeat(20000)+'end');
  const first=await invoke('browser_get_text',{});assert.ok(first.text.includes('Truncated'));
  assert.equal((await invoke('browser_get_text',{offset:20000})).text,'end');
  assert.equal((await invoke('browser_get_text',{selector:'#missing'})).text,'No element matched selector: #missing');
});
test('fixed Bilibili history parser validates host, session and returns only bounded titles',async()=>{
  const {context,invoke}=page('api.bilibili.com',[],JSON.stringify({code:0,data:{list:[{title:'Watched video',private_metadata:'not returned'}]}}));
  context.location.pathname='/x/web-interface/history/cursor';
  assert.deepEqual(JSON.parse((await invoke('browser_bilibili_history',{limit:1})).text),[{title:'Watched video'}]);
  context.document.body.textContent='{"code":-101}';await assert.rejects(invoke('browser_bilibili_history',{}),/会话无效/);
  context.location.hostname='bilibili.com.evil.example';await assert.rejects(invoke('browser_bilibili_history',{}),/Not a Bilibili/);
});
