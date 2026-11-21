/* global api, charName, charAvatarHtml, memCharList, currentCharId, goHome, showModal, hideModal, toast, loadCharacters, loadNotes, currentNoteId, noteOwnerId, noteEditingOwnerId, noteBlocksDirty, renderNoteBlocks */
(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  const escape = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const segment = value => encodeURIComponent(value);
  const characters = () => typeof memCharList !== 'undefined' ? memCharList : [];
  const defaultOwner = () => currentCharId || characters()[0]?.id || '';
  const state = {
    page: 'home', mind: {owner:'', seq:0}, diary: {owner:'', seq:0, entries:[], selected:null},
    work: {id:null, seq:0, listSeq:0, sessions:[], detail:null, signature:'', questionId:null,
      process:null, processSignature:'', processFilter:'all', processOpen:false}, planFilter:'pending',
  };
  const sending = new Set();
  let refreshTimer, connectionBusy = false, restoringHistory = false;
  const storage = {
    get(key) { try { return sessionStorage.getItem('vivian-mobile:' + key) || ''; } catch { return ''; } },
    set(key,value) { try { if(value) sessionStorage.setItem('vivian-mobile:' + key,value); else sessionStorage.removeItem('vivian-mobile:' + key); } catch { /* Keep the current input usable when storage is unavailable. */ } },
  };
  const empty = (title,detail,action) => `<div class="mobile-empty"><span class="mobile-empty-mark" aria-hidden="true">✧</span><strong>${escape(title)}</strong><p>${escape(detail)}</p>${action?`<button class="btn outline" data-mobile-action="${action}">重新加载</button>`:''}</div>`;
  const loading = () => '<div class="mobile-loading" role="status"><span></span>加载中…</div>';
  const time = value => {
    const n = Number(value), date = new Date(n < 1e12 ? n * 1000 : n);
    return Number.isFinite(n) && n > 0 ? date.toLocaleString('zh-CN',{month:'numeric',day:'numeric',hour:'2-digit',minute:'2-digit'}) : '';
  };
  const text = value => escape(value).replace(/\n/g,'<br>');
  const inputDateTime = value => {
    if(!value)return '';
    const date=new Date(value);if(!Number.isFinite(date.getTime()))return '';
    const pad=number=>String(number).padStart(2,'0');
    return date.getFullYear()+'-'+pad(date.getMonth()+1)+'-'+pad(date.getDate())+'T'+pad(date.getHours())+':'+pad(date.getMinutes());
  };
  function prose(value) {
    return String(value || '').split(/(```[\s\S]*?```)/g).map(part => part.startsWith('```')
      ? '<pre><code>' + escape(part.replace(/^```[^\n]*\n?/, '').replace(/```$/, '')) + '</code></pre>'
      : escape(part).split(/\n\s*\n/).filter(Boolean).map(p=>'<p>'+p.replace(/^#{1,6} /gm,'').replace(/\*\*([^*]+)\*\*/g,'<strong>$1</strong>').replace(/`([^`\n]+)`/g,'<code>$1</code>').replace(/\n/g,'<br>')+'</p>').join('')).join('');
  }
  async function request(path,options={}) {
    const controller = new AbortController(), timer = setTimeout(()=>controller.abort(), 15000);
    try { return await api(path,{...options,signal:controller.signal}); }
    catch(error) { if(error.name==='AbortError') throw new Error('连接超时，请检查电脑是否在线后重试。'); throw error; }
    finally { clearTimeout(timer); }
  }
  function owners(id,kind) {
    const owner = kind === 'notes' ? noteOwnerId || defaultOwner() : state[kind].owner;
    $(id).innerHTML = characters().map(char=>`<button type="button" data-owner-kind="${kind}" data-owner="${escape(char.id)}" aria-pressed="${char.id===owner}"><span class="owner-dot owner-${char.id==='nana'?'nana':'vivian'}"></span>${escape(char.name||charName(char.id))}</button>`).join('');
  }
  function renderOwnerControls() {
    for (const kind of ['mind','diary']) {
      if (!state[kind].owner || !characters().some(char=>char.id===state[kind].owner)) state[kind].owner=defaultOwner();
      owners(kind+'Owners',kind);
    }
    owners('noteOwners','notes');
    const selected=$('workCharacter').value;
    $('workCharacter').replaceChildren(...characters().map(char=>new Option(char.name||charName(char.id),char.id)));
    $('workCharacter').value=characters().some(char=>char.id===selected)?selected:defaultOwner();
  }

  const emotions = {joy:'开心',happy:'开心',sadness:'低落',sad:'低落',anger:'生气',fear:'不安',curiosity:'好奇',closeness:'亲近',loneliness:'想有人陪',calm:'平静',neutral:'平静',excitement:'兴奋',surprise:'惊讶',trust:'安心',tired:'疲倦'};
  const metric = (label,value,max=1) => {
    if (value === null || value === undefined || !Number.isFinite(Number(value))) return '';
    const percent = Math.round(Math.max(0,Math.min(1,Number(value)/max))*100);
    return `<div class="mobile-meter"><div><span>${label}</span><strong>${percent}%</strong></div><div role="meter" aria-label="${label}" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${percent}"><i style="width:${percent}%"></i></div></div>`;
  };
  async function loadMind(quiet=false) {
    const owner=state.mind.owner, seq=++state.mind.seq;
    if (!owner) { $('mindContent').innerHTML=empty('正在等待角色','连接电脑后就能查看。','reconnect'); return; }
    if(!quiet) $('mindContent').innerHTML=loading();
    const results = await Promise.allSettled(['mind','mood','relationship','presence'].map(path=>request('/api/characters/'+segment(owner)+'/'+path)));
    if(seq!==state.mind.seq || owner!==state.mind.owner) return;
    if(results.every(r=>r.status==='rejected')) {
      $('mindStatus').textContent=''; $('mindContent').innerHTML=empty('状态加载失败',results[0].reason.message,'refresh-mind'); return;
    }
    const [mind,mood,relation,presence]=results.map(r=>r.status==='fulfilled'?r.value:null);
    const missing=results.filter(r=>r.status==='rejected').length;
    $('mindStatus').textContent=(missing?'部分状态暂时不可用 · ':'')+'更新于 '+new Date().toLocaleTimeString('zh-CN',{hour:'2-digit',minute:'2-digit'});
    const emotion=mood?.primary_emotion;
    $('mindContent').innerHTML=`<article class="mobile-mind-hero owner-${owner==='nana'?'nana':'vivian'}"><div class="mobile-mind-person">${charAvatarHtml(owner,'mobile-mind-avatar')}<div><span class="mobile-eyebrow">此刻 · ${escape(presence?.display_zh||'状态未提供')}</span><h3>${escape(charName(owner))}</h3></div><span class="mobile-mood-pill">${escape(emotions[emotion]||emotion||'心情未提供')}</span></div><blockquote>${mind?.inner_monologue_enabled===false?'未开启内心独白。':text(mind?.current_thought||'暂无当前想法。')}</blockquote></article>
      <article class="mobile-card"><div class="mobile-section-heading"><h3>心情与精力</h3><span>此刻状态</span></div>${mood?metric('情绪活跃',mood.arousal)+metric('疲倦程度',mood.fatigue)+metric('压力程度',mood.stress):'<p class="mobile-caption">心情状态暂时不可用。</p>'}</article>
      <article class="mobile-card"><div class="mobile-section-heading"><h3>关注与目标</h3><span>当前关注</span></div>${mind?.attention_top?.length?`<div class="mobile-tags">${mind.attention_top.map(item=>`<span>${escape(item.entity)}</span>`).join('')}</div>`:'<p class="mobile-caption">暂时没有特别关注与目标。</p>'}${mind?.goals?.length?`<ul class="mobile-goals">${mind.goals.map(goal=>`<li>${escape(goal.description)}</li>`).join('')}</ul>`:''}</article>
      <article class="mobile-card"><div class="mobile-section-heading"><h3>关系状态</h3><span>${Number.isFinite(relation?.interaction_count)?relation.interaction_count+' 次交流':'关系状态'}</span></div>${relation?metric('信任',relation.trust,100)+metric('亲近',relation.intimacy,100)+metric('熟悉',relation.familiarity,100):'<p class="mobile-caption">关系状态暂时不可用。</p>'}<button class="mobile-text-button" data-mobile-action="open-memory">查看记忆 →</button></article>`;
  }

  async function loadDiary() {
    const owner=state.diary.owner, seq=++state.diary.seq;
    state.diary.selected=null; $('diaryIndex').hidden=false; $('diaryReader').hidden=true;
    $('diaryList').innerHTML=loading(); $('diaryCount').textContent='';
    if(!owner) { $('diaryList').innerHTML=empty('正在等待角色','连接电脑后就能翻阅日记。','reconnect'); return; }
    try {
      const data=await request('/api/characters/'+segment(owner)+'/diary');
      if(seq!==state.diary.seq) return;
      state.diary.entries=(data.diaries||[]).sort((a,b)=>String(b.date).localeCompare(String(a.date)) || (b.created_at||0)-(a.created_at||0));
      renderDiary();
    } catch(error) { if(seq===state.diary.seq) $('diaryList').innerHTML=empty('日记没能加载出来',error.message,'refresh-diary'); }
  }
  function renderDiary() {
    const query=$('diarySearch').value.trim().toLowerCase(), date=$('diaryDate').value;
    const entries=state.diary.entries.filter(entry=>(!date||entry.date===date)&&(!query||[entry.content,...(entry.key_events||[])].join(' ').toLowerCase().includes(query)));
    $('diaryCount').textContent=entries.length+' 篇日记'+(query||date?' · 已筛选':'');
    $('diaryList').innerHTML=entries.length?entries.map(entry=>`<button class="mobile-diary-card" data-diary-id="${escape(entry.id)}"><div><time>${escape(entry.date)}</time><span>${escape(emotions[entry.mood_tag]||({neutral:'平静'})[entry.mood_tag]||'日常手记')}</span></div><h3>${escape(entry.key_events?.[0]||'日记')}</h3><p>${escape(String(entry.content||'').replace(/[#*`]/g,'').slice(0,110))}${entry.content?.length>110?'…':''}</p><footer><span>${escape(charName(state.diary.owner))} · ${escape(entry.word_count??String(entry.content||'').length)} 字</span><span>查看日记 ↗</span></footer></button>`).join(''):empty(query||date?'没有找到这一天的记录':'暂无日记','可切换角色、日期或搜索关键词。');
  }
  function showDiary(id) {
    const entry=state.diary.entries.find(item=>String(item.id)===id); if(!entry)return;
    state.diary.selected=id; $('diaryIndex').hidden=true; $('diaryReader').hidden=false;
    $('diaryReader').innerHTML=`<span class="mobile-eyebrow">${escape(charName(state.diary.owner))} 的日记</span><h2>${escape(entry.date)}</h2><p class="mobile-caption">${escape(entry.word_count??'')} 字 · ${escape(time(entry.created_at))}</p>${entry.key_events?.length?`<ul class="mobile-diary-events">${entry.key_events.map(item=>`<li>${escape(item)}</li>`).join('')}</ul>`:''}<div class="mobile-prose">${prose(entry.content)}</div><button class="btn outline" data-mobile-action="diary-list">返回日记列表</button>`;
    $('diaryBody').scrollTop=0;
  }
  function diaryBack() {
    if(state.diary.selected) {state.diary.selected=null;$('diaryIndex').hidden=false;$('diaryReader').hidden=true;}
    else goHome();
  }

  const statusLabel = session => session.pending_question?'等你回应':({running:'进行中',idle:'可继续',canceled:'已停止'})[session.status]||'可继续';
  const PROCESS_FILTERS = [
    {id:'all',label:'全部'},{id:'failed',label:'失败'},{id:'files',label:'文件'},{id:'think',label:'思考'},
  ];
  const PROCESS_GLYPH = {edit:'✎',read:'▤',search:'⌕',run:'▸',browse:'◍',plan:'☰',image:'▣',other:'⚙'};
  const PROCESS_SUMMARY_KEYS = new Set(['path','file_path','file','target','directory','dir','pattern','query','glob','keyword','q','command','script','cmd','url']);
  const processGlyph = kind => PROCESS_GLYPH[kind] || PROCESS_GLYPH.other;

  /** 对话流只保留真正的对话消息；工具、思考、旁白都进「过程」。 */
  function conversationOf(data) {
    return (data.messages||[]).filter(message => ['user','assistant','error','notice'].includes(message.role) && String(message.content||'').trim());
  }
  function processSummaryLine(process) {
    if(!process) return '';
    const parts=[], summary=window.WorkProcess?.summaryText(process.summary);
    if(summary) parts.push(summary);
    if(process.counts.thinking) parts.push('思考 '+process.counts.thinking+' 段');
    if(process.counts.failed) parts.push(process.counts.failed+' 步失败');
    return parts.join(' · ');
  }
  function uniqueProcessFiles(process) {
    const files=new Set();
    for(const call of process?.calls||[]) {
      if(window.WorkProcess.FILE_TOOLS.has(call.name)&&call.path) files.add(String(call.path).replace(/\\/g,'/').toLowerCase());
    }
    return files.size;
  }
  function processSignatureOf(process) {
    if(!process) return '';
    const rows=process.items.map(item=>{
      if(item.type==='tool') {
        const info=item.call.info||{};
        return ['t',item.call.id,item.weight,item.emptyCount||0,!!item.call.hasResult,!!item.call.success,info.meta||'',info.state||'',(info.files||[]).length,Boolean(item.call.result)];
      }
      if(item.type==='think') return ['k',item.index,String(item.content||'').length];
      if(item.type==='say') return ['s',item.index,String(item.content||'').length];
      return ['p',item.phase,item.count,item.files,item.failed];
    });
    return JSON.stringify([rows,process.steps,process.running,process.counts]);
  }
  function renderProcessEntry() {
    const process=state.work.process, entry=$('workProcessEntry'), line=processSummaryLine(process);
    const summary=$('workProcessSummaryText'), sheetSummary=$('workProcessSummaryTextSheet');
    if(summary) summary.textContent=line;
    if(sheetSummary) sheetSummary.textContent=line;
    const rows=process?process.items.filter(item=>item.type!=='phase').length:0;
    const content=rows>0;
    if(entry) entry.hidden=!content;
    if(!content||!entry) return;
    const status=$('workProcessStatus');
    status.className='mobile-process-entry-status';
    if(process.running) {status.textContent='进行中';status.classList.add('is-running');}
    else if(process.counts.failed) {status.textContent=process.counts.failed+' 步失败';status.classList.add('is-failed');}
    else status.textContent=rows+' 步';
  }
  function visibleProcessItems(process) {
    const filter=state.work.processFilter, items=process.items||[], WP=window.WorkProcess;
    if(filter==='all') return items;
    return items.filter(item=>{
      if(item.type==='phase') return false;
      if(filter==='think') return item.type==='think';
      if(item.type!=='tool') return false;
      const call=item.call, info=call.info||{};
      if(filter==='failed') return call.success===false||info.state==='failed';
      return WP.FILE_TOOLS.has(call.name);
    });
  }
  function renderProcessFilters(process) {
    const filters=$('workProcessFilters'); if(!filters) return;
    const counts={all:process.items.filter(item=>item.type!=='phase').length, failed:process.counts.failed, files:uniqueProcessFiles(process), think:process.counts.thinking};
    filters.innerHTML=PROCESS_FILTERS.map(item=>`<button type="button" data-process-filter="${item.id}" aria-pressed="${state.work.processFilter===item.id}">${item.label}${counts[item.id]?`<span class="process-filter-count">${counts[item.id]}</span>`:''}</button>`).join('');
  }
  function renderProcessTool(item, openKeys) {
    const WP=window.WorkProcess, call=item.call, info=call.info||{}, failed=call.success===false||info.state==='failed', pending=info.state==='pending'||info.state==='stopped';
    const merged=item.emptyCount>1?' · '+item.emptyCount+' 次':'';
    const key='tool:'+(call.id||item.index);
    const files=info.files||[];
    const extra=WP.detailPairs(call).filter(([name])=>!PROCESS_SUMMARY_KEYS.has(name)).slice(0,8);
    const output=String(call.result||'').trim(), preview=output.length>1200?output.slice(0,1200)+'…':output;
    const range=(call.args?.start_line!=null&&call.args?.end_line!=null)?String(call.args.start_line)+'-'+String(call.args.end_line):'';
    const detail=[
      extra.length?`<div class="process-detail">${extra.map(([name,value])=>`<div class="process-detail-row"><span class="process-detail-key">${escape(name)}</span><span class="process-detail-value">${escape(value)}</span></div>`).join('')}</div>`:'',
      files.length?`<div class="process-files">${files.slice(0,8).map(file=>`<div class="process-file"><span class="process-file-icon" aria-hidden="true">◫</span><span class="process-file-name" title="${escape(file)}">${escape(WP.displayPath(file,item.call.cwd))}</span>${range?`<span class="process-file-range">${escape(range)}</span>`:''}</div>`).join('')}${files.length>8?`<span class="process-file-more">还有 ${files.length-8} 个文件</span>`:''}</div>`:'',
      preview?`<pre class="process-output">${escape(preview)}</pre>`:'',
    ].join('');
    const hasDetail=!!detail;
    const head=`<span class="process-tool-label">${escape(WP.toolLabel(call.name))}</span><span class="process-tool-meta${failed?' is-failed':pending?' is-pending':''}">${escape((info.meta||'')+merged)}</span>${hasDetail?'<span class="process-chevron" aria-hidden="true">›</span>':''}${call.argsText?`<span class="process-tool-arg" title="${escape(call.argsText)}">${escape(call.argsText)}</span>`:''}`;
    const icon=`<span class="process-item-icon" aria-hidden="true">${processGlyph(call.kind)}</span>`;
    const classes=`process-item process-tool${failed?' is-failed':''}`;
    return hasDetail
      ? `<details class="${classes}" data-key="${escape(key)}"${openKeys.has(key)?' open':''}>${icon}<summary>${head}</summary><div class="process-tool-body">${detail}</div></details>`
      : `<div class="${classes} is-static">${icon}<div class="process-tool-head">${head}</div></div>`;
  }
  function renderProcessThink(item, openKeys) {
    const key='think:'+item.index, one=String(item.content||'').replace(/\s+/g,' ').trim(), preview=one.length>80?one.slice(0,79)+'…':one;
    const open=state.work.processFilter==='think'||openKeys.has(key);
    return `<details class="process-item process-think" data-key="${escape(key)}"${open?' open':''}><span class="process-item-icon" aria-hidden="true">◍</span><summary><span class="process-think-label">思考过程</span><span class="process-think-preview">${escape(preview)}</span><span class="process-chevron" aria-hidden="true">›</span></summary><div class="process-think-body">${escape(String(item.content||'').trim())}</div></details>`;
  }
  function renderProcessSay(item, openKeys) {
    const key='say:'+item.index, one=String(item.content||'').replace(/\s+/g,' ').trim(), preview=one.length>80?one.slice(0,79)+'…':one;
    return `<details class="process-item process-say" data-key="${escape(key)}"${openKeys.has(key)?' open':''}><span class="process-item-icon" aria-hidden="true">✦</span><summary><span class="process-say-label">说明</span><span class="process-say-preview">${escape(preview)}</span><span class="process-chevron" aria-hidden="true">›</span></summary><div class="process-say-body mobile-prose">${prose(item.content)}</div></details>`;
  }
  function renderProcessSheet(reset=false) {
    const list=$('workProcessList'), process=state.work.process;
    if(!state.work.processOpen||!list) return;
    if(!process) {list.innerHTML='<div class="process-sheet-empty">还没有工具调用记录。</div>';return;}
    renderProcessFilters(process);
    const signature=processSignatureOf(process);
    if(!reset&&signature===state.work.processSignature) return;
    const openKeys=new Set([...list.querySelectorAll('details[data-key][open]')].map(element=>element.dataset.key));
    const scroll=reset?0:list.scrollTop;
    state.work.processSignature=signature;
    const items=visibleProcessItems(process);
    list.innerHTML=items.length?`<div class="process-list">${items.map(item=>{
      if(item.type==='phase') return `<div class="process-phase"><span>${escape(window.WorkProcess.phaseLabel(item.phase))}</span><span class="process-phase-count">${item.count} 步${item.failed?' · '+item.failed+' 失败':''}</span></div>`;
      if(item.type==='think') return renderProcessThink(item,openKeys);
      if(item.type==='say') return renderProcessSay(item,openKeys);
      return renderProcessTool(item,openKeys);
    }).join('')}</div>`:`<div class="process-sheet-empty">${state.work.processFilter==='all'?'还没有工具调用记录。':'这个筛选下没有内容。'}</div>`;
    list.scrollTop=scroll;
  }
  function openProcessSheet() {
    if(!state.work.process||!state.work.process.items.length) return;
    state.work.processOpen=true;
    state.work.processSignature='';
    $('workProcessSheet').hidden=false;
    renderProcessSheet();
    $('workProcessClose')?.focus({preventScroll:true});
  }
  function closeProcessSheet() {
    if(!state.work.processOpen) return;
    state.work.processOpen=false;
    $('workProcessSheet').hidden=true;
    const entry=$('workProcessEntry');
    if(entry&&!entry.hidden) entry.focus({preventScroll:true});
  }
  function updateWorkProcess(data) {
    state.work.process=window.WorkProcess?window.WorkProcess.build(data.messages,data.working_directory,data.status):null;
    renderProcessEntry();
    renderProcessSheet();
  }
  function renderWorkConversation(data) {
    const container=$('workMessages');
    const follow=!state.work.signature||container.scrollHeight-container.scrollTop-container.clientHeight<100;
    const scroll=container.scrollTop;
    const messages=conversationOf(data);
    const hasProcess=!!state.work.process&&state.work.process.items.some(item=>item.type!=='phase');
    container.innerHTML=(messages.length?messages.map(message=>{
      const mine=message.role==='user';
      const label=mine?'你':message.role==='error'?'错误':message.role==='notice'?'提示':charName(data.char_id);
      return `<article class="mobile-work-message${mine?' from-user':''}${message.role==='error'?' is-error':''}${message.role==='notice'?' is-notice':''}"><header>${escape(label)}<time>${escape(time(message.timestamp))}</time></header><div class="mobile-prose">${prose(message.content)}</div></article>`;
    }).join(''):`<div class="mobile-work-placeholder">${hasProcess?'工具调用与思考已收进顶部「过程」，完成后会在这里给出回复。':'在下方输入任务，工作智能体会在这里回复。'}</div>`)
      + (data.work_todos?.length?`<details class="mobile-work-plan"><summary>任务进度 · ${data.work_todos.length} 项</summary><ul>${data.work_todos.map(item=>`<li>${item.status==='completed'?'✓':'○'} ${escape(item.title||item.content||item.description)}</li>`).join('')}</ul></details>`:'')
      + (data.deliverables?.length?`<details class="mobile-work-plan"><summary>产物文件 · ${data.deliverables.length}</summary><ul>${data.deliverables.map(file=>`<li>${escape(file)}</li>`).join('')}</ul></details>`:'');
    container.scrollTop=follow?container.scrollHeight:scroll;
  }

  function renderWorkList() {
    const query=$('workSearch').value.trim().toLowerCase();
    const sessions=state.work.sessions.filter(s=>!query||[s.title,s.working_directory,charName(s.char_id)].join(' ').toLowerCase().includes(query));
    $('workList').innerHTML=sessions.length?sessions.map(s=>`<button class="mobile-session-card" data-work-id="${escape(s.session_id)}"><div><span class="mobile-eyebrow">${escape(charName(s.char_id))}</span><span class="mobile-work-status ${s.pending_question?'needs-answer':s.status==='running'?'is-running':''}">${statusLabel(s)}</span></div><h3>${escape(s.title||'新的工作会话')}</h3><p>${escape(s.working_directory||'未绑定工作目录')}</p><footer><span>${escape(s.message_count??0)} 条记录</span><time>${escape(time(s.updated_at))}</time></footer></button>`).join(''):empty(query?'没有找到这个会话':'暂无工作会话','新建会话或继续已有任务。');
  }
  async function loadWorkList(quiet=false) {
    const seq=++state.work.listSeq;
    if(!quiet)$('workList').innerHTML=loading();
    try { const data=await request('/api/work/sessions');if(seq!==state.work.listSeq)return;state.work.sessions=data.sessions||[];renderWorkList(); }
    catch(error) { if(seq===state.work.listSeq)$('workList').innerHTML=empty('工作会话暂时无法读取',error.message,'refresh-work'); }
  }
  function rememberWorkDraft() { if(state.work.id)storage.set('work:'+state.work.id,$('workInput').value); }
  async function openWork(id) {
    rememberWorkDraft();closeProcessSheet();
    state.work.id=id;state.work.signature='';state.work.questionId=null;state.work.detail=null;
    state.work.process=null;state.work.processSignature='';state.work.processFilter='all';
    $('workIndex').hidden=true;$('workDetail').hidden=false;$('workQuestion').hidden=true;$('workError').textContent='';
    $('workMessages').innerHTML=loading();$('workMeta').replaceChildren();$('workPageTitle').textContent='工作会话';
    $('workProcessEntry').hidden=true;$('workProcessList').replaceChildren();
    $('workInput').value=storage.get('work:'+id);$('workSend').disabled=true;$('workStop').hidden=true;
    await loadWorkDetail(id);scheduleRefresh();
  }
  function workBack() {
    if(!state.work.id){goHome();return;}
    closeProcessSheet();rememberWorkDraft();state.work.id=null;state.work.seq++;
    state.work.process=null;state.work.processSignature='';
    $('workIndex').hidden=false;$('workDetail').hidden=true;$('workPageTitle').textContent='工作台';
    loadWorkList();scheduleRefresh();
  }
  async function loadWorkDetail(id,quiet=false) {
    const seq=++state.work.seq;
    try {
      const data=await request('/api/work/sessions/'+segment(id));
      if(seq!==state.work.seq||id!==state.work.id)return;
      state.work.detail=data;$('workError').textContent='';
      $('workPageTitle').textContent=data.title||'工作会话';
      $('workMeta').innerHTML=`<span>${escape(charName(data.char_id))}</span><span class="mobile-work-status ${data.status==='running'?'is-running':''}">${statusLabel(data)}</span><span class="mobile-work-directory">${escape(data.working_directory||'未绑定工作目录')}</span>`;
      updateWorkProcess(data);
      const signature=JSON.stringify([conversationOf(data),data.deliverables,data.work_todos,data.status]);
      if(signature!==state.work.signature) {
        state.work.signature=signature;
        renderWorkConversation(data);
      }
      renderQuestion(data.pending_question);
      $('workSend').disabled=sending.has(id)||data.status==='running';
      $('workStop').hidden=data.status!=='running';
      $('workSendHint').textContent=data.pending_question?'请先回应上方的问题':data.status==='running'?'正在工作，完成后可继续发送':'消息发送给当前工作会话';
    } catch(error) { if(seq!==state.work.seq||id!==state.work.id)return;$('workError').textContent='连接中断：'+error.message;if(!quiet)$('workMessages').innerHTML=empty('会话暂时无法打开',error.message,'refresh-work'); }
  }
  function renderQuestion(question) {
    if(!question) {state.work.questionId=null;$('workQuestion').hidden=true;return;}
    if(question.question_id===state.work.questionId)return;
    state.work.questionId=question.question_id;$('workQuestion').hidden=false;
    $('workQuestion').innerHTML=`<form id="workAnswerForm"><span class="mobile-eyebrow">等你回应</span><h3>${escape(question.question)}</h3>${question.context?`<p class="mobile-caption">${text(question.context)}</p>`:''}<div class="mobile-answer-options">${(question.options||[]).map((option,index)=>`<label><input type="${question.multi_select?'checkbox':'radio'}" name="workAnswer" value="${index}"><span><strong>${escape(option.label)}</strong>${option.description?`<small>${escape(option.description)}</small>`:''}</span></label>`).join('')}</div><label class="sr-only" for="workCustomAnswer">补充回答</label><textarea id="workCustomAnswer" rows="2" placeholder="也可以写下你的想法"></textarea><div class="mobile-toolbar"><button type="button" class="mobile-text-button" data-mobile-action="skip-question">跳过这次提问</button><button type="submit" class="btn">确认回答</button></div></form>`;
    $('workAnswerForm').onsubmit=event=>{event.preventDefault();answerQuestion(false);};
    $('workCustomAnswer').oninput=()=>{if(!question.multi_select&&$('workCustomAnswer').value.trim())$('workAnswerForm').querySelectorAll('input:checked').forEach(input=>input.checked=false);};
    $('workAnswerForm').onchange=()=>{if(!question.multi_select)$('workCustomAnswer').value='';};
  }
  async function answerQuestion(skip) {
    const id=state.work.id, question=state.work.detail?.pending_question;
    if(!id||!question)return;
    const selected=skip?[]:[...$('workAnswerForm').querySelectorAll('input:checked')].map(input=>question.options[Number(input.value)].label);
    const custom=skip?null:$('workCustomAnswer').value.trim()||null;
    if(!skip&&!selected.length&&!custom){$('workError').textContent='请选择一个方向，或写下你的回答。';return;}
    const buttons=[...$('workAnswerForm').querySelectorAll('button')];buttons.forEach(button=>button.disabled=true);
    try { await request('/api/work/sessions/'+segment(id)+'/answer',{method:'POST',body:JSON.stringify({question_id:question.question_id,answer:{selected,custom}})});await loadWorkDetail(id,true); }
    catch(error) { if(id===state.work.id)$('workError').textContent=error.message; }
    finally { buttons.forEach(button=>button.disabled=false); }
  }
  $('workComposer').onsubmit=async event=>{
    event.preventDefault();const id=state.work.id, message=$('workInput').value.trim();
    if(!id||!message||sending.has(id)||state.work.detail?.status==='running')return;
    sending.add(id);$('workSend').disabled=true;$('workError').textContent='';rememberWorkDraft();
    try {
      await request('/api/work/sessions/'+segment(id)+'/send',{method:'POST',body:JSON.stringify({message})});
      storage.set('work:'+id,'');if(id===state.work.id){$('workInput').value='';await loadWorkDetail(id,true);}
    } catch(error) {if(id===state.work.id)$('workError').textContent='没有发送成功，内容已保留。'+error.message;}
    finally {sending.delete(id);if(id===state.work.id)$('workSend').disabled=state.work.detail?.status==='running';}
  };
  $('workInput').oninput=rememberWorkDraft;
  $('workNewForm').onsubmit=async event=>{
    event.preventDefault();const button=$('workCreate');if(button.disabled)return;button.disabled=true;$('workNewError').textContent='';
    try {const data=await request('/api/work/sessions',{method:'POST',body:JSON.stringify({character_id:$('workCharacter').value,working_directory:$('workDirectory').value.trim()})});hideModal('workNewModal');await openWork(data.session_id);}
    catch(error){$('workNewError').textContent=error.message;}finally{button.disabled=false;}
  };

  function filterNotes() {
    const query=$('noteSearch').value.trim().toLowerCase();
    const cards=[...$('noteList').querySelectorAll('.note-swipe')];
    cards.forEach(card=>card.hidden=!!query&&!card.textContent.toLowerCase().includes(query));
    $('noteNoMatch')?.remove();
    if(cards.length&&cards.every(card=>card.hidden))$('noteList').insertAdjacentHTML('beforeend',`<div id="noteNoMatch">${empty('没有找到这篇笔记','试试其他标题或标签。')}</div>`);
  }
  const noteDraftKey = (owner,id) => 'note:'+owner+':'+(id||'new');
  function rememberNoteDraft() {
    const owner=noteEditingOwnerId||noteOwnerId||defaultOwner();
    if(!owner||$('noteModal').classList.contains('hidden'))return;
    storage.set(noteDraftKey(owner,currentNoteId),JSON.stringify({title:$('noteTitle').value,blocks:noteBlocksDirty}));
  }
  function restoreNoteDraft() {
    const raw=storage.get(noteDraftKey(noteEditingOwnerId||noteOwnerId||defaultOwner(),currentNoteId));
    if(!raw)return;
    try {const draft=JSON.parse(raw);if(typeof draft.title==='string'&&Array.isArray(draft.blocks)){$('noteTitle').value=draft.title;noteBlocksDirty=draft.blocks;renderNoteBlocks();toast('已恢复未保存的笔记草稿');}}catch{/* Invalid drafts never replace loaded note content. */}
  }
  function clearNoteDraft(owner,id) { storage.set(noteDraftKey(owner,id),''); }
  function decorateNotes() {
    filterNotes();
    $('noteList').querySelectorAll('.note-swipe').forEach(card=>{
      if(card.querySelector('.mobile-note-actions'))return;
      const row=document.createElement('div');row.className='mobile-note-actions';
      for(const [label,action] of [['阅读全文',()=>window.viewNoteHtml(card.dataset.id)],['编辑笔记',()=>window.openNote(card.dataset.id)]]){
        const button=document.createElement('button');button.className='mobile-text-button';button.textContent=label;button.onclick=event=>{event.stopPropagation();action();};row.append(button);
      }
      card.querySelector('.note-front').append(row);
    });
  }
  function filterPlans() {
    const query=$('planSearch').value.trim().toLowerCase(), cards=[...$('todoList').querySelectorAll('.todo-item')];
    let shown=0;cards.forEach(card=>{const completed=card.classList.contains('completed');card.hidden=(state.planFilter!=='all'&&completed!==(state.planFilter==='completed'))||!!query&&!card.textContent.toLowerCase().includes(query);if(!card.hidden)shown++;});
    $('planSummary').textContent=cards.filter(card=>!card.classList.contains('completed')).length+' 项待完成 · 当前显示 '+shown+' 项';
    $('planNoMatch')?.remove();if(cards.length&&!shown)$('todoList').insertAdjacentHTML('beforeend',`<div id="planNoMatch">${empty('暂无匹配的待办','切换筛选或添加待办。')}</div>`);
  }

  async function checkConnection() {
    if(connectionBusy)return;connectionBusy=true;
    try {const health=await request('/api/health');const ready=health.initialized!==false;$('mobileConnection').textContent=ready?'已连接电脑 · 内容实时同步':'电脑正在准备，请稍候';$('mobileConnection').classList.toggle('is-online',ready);$('connectionBanner').hidden=ready;if(ready&&!characters().length)await loadCharacters();}
    catch(error){$('mobileConnection').textContent='连接已断开 · 点击重新连接';$('mobileConnection').classList.remove('is-online');$('connectionBanner').hidden=false;}
    finally{connectionBusy=false;}
  }
  function scheduleRefresh() {
    clearTimeout(refreshTimer);if(document.hidden)return;
    refreshTimer=setTimeout(async()=>{
      if(document.hidden)return;
      if(state.page==='work') {if(state.work.id)await loadWorkDetail(state.work.id,true);else await loadWorkList(true);}
      else if(state.page==='mind')await loadMind(true);
      await checkConnection();scheduleRefresh();
    },state.page==='work'&&state.work.id?3000:15000);
  }
  function onPageChange(page) {
    if(page!=='work')closeProcessSheet();
    rememberWorkDraft();state.page=page;
    if(page==='mind')loadMind();if(page==='diary')loadDiary();if(page==='work'){if(state.work.id)loadWorkDetail(state.work.id);else loadWorkList();}
    if(!restoringHistory&&location.hash!=='#'+page)history.pushState({mobilePage:page},'','#'+page);
    scheduleRefresh();
  }
  document.addEventListener('mobile:pagechange',event=>onPageChange(event.detail));
  document.addEventListener('mobile:characters-ready',()=>{renderOwnerControls();if(state.page==='mind')loadMind();if(state.page==='diary')loadDiary();if(state.page==='notes')loadNotes();});
  document.addEventListener('click',async event=>{
    const preset=event.target.closest('[data-reminder-minutes],[data-reminder-tomorrow]');
    if(preset){const date=new Date();if(preset.hasAttribute('data-reminder-tomorrow')){date.setDate(date.getDate()+1);date.setHours(9,0,0,0);}else date.setMinutes(date.getMinutes()+Number(preset.dataset.reminderMinutes));$('taskTime').value=inputDateTime(date);return;}
    const owner=event.target.closest('[data-owner-kind]');
    if(owner){const kind=owner.dataset.ownerKind;if(kind==='notes'){noteOwnerId=owner.dataset.owner;owners('noteOwners','notes');loadNotes();}else{state[kind].owner=owner.dataset.owner;owners(kind+'Owners',kind);if(kind==='mind')loadMind();else loadDiary();}return;}
    const diary=event.target.closest('[data-diary-id]');if(diary){showDiary(diary.dataset.diaryId);return;}
    const session=event.target.closest('[data-work-id]');if(session){openWork(session.dataset.workId);return;}
    const filter=event.target.closest('#planFilters [data-filter]');if(filter){state.planFilter=filter.dataset.filter;$('planFilters').querySelectorAll('button').forEach(button=>button.setAttribute('aria-pressed',button===filter));filterPlans();return;}
    const processFilter=event.target.closest('[data-process-filter]');if(processFilter){state.work.processFilter=processFilter.dataset.processFilter;renderProcessSheet(true);return;}
    const button=event.target.closest('[data-mobile-action]');if(!button)return;
    switch(button.dataset.mobileAction){
      case 'refresh-mind':loadMind();break;case 'refresh-diary':loadDiary();break;
      case 'clear-diary-date':$('diaryDate').value='';renderDiary();break;case 'diary-list':diaryBack();break;
      case 'refresh-work':if(state.work.id)loadWorkDetail(state.work.id);else loadWorkList();break;
      case 'new-work':$('workNewError').textContent='';$('workDirectory').value='';showModal('workNewModal');break;
      case 'stop-work':{const id=state.work.id;button.disabled=true;try{await request('/api/work/sessions/'+segment(id)+'/stop',{method:'POST'});if(id===state.work.id)await loadWorkDetail(id,true);}catch(error){if(id===state.work.id)$('workError').textContent=error.message;}finally{button.disabled=false;}break;}
      case 'skip-question':answerQuestion(true);break;case 'open-memory':window.navigateMobile('memory');break;
      case 'open-process':openProcessSheet();break;case 'close-process':closeProcessSheet();break;
      case 'reconnect':await checkConnection();if(!characters().length)await loadCharacters();renderOwnerControls();onPageChange(state.page);break;
    }
  });
  $('diarySearch').oninput=renderDiary;$('diaryDate').onchange=renderDiary;$('workSearch').oninput=renderWorkList;
  $('noteSearch').oninput=filterNotes;$('planSearch').oninput=filterPlans;
  $('noteModal').addEventListener('input',rememberNoteDraft);
  $('noteModal').addEventListener('change',rememberNoteDraft);
  $('noteModal').addEventListener('click',rememberNoteDraft);
  $('mobileConnection').onclick=checkConnection;
  for(const header of document.querySelectorAll('.home-header,.page-header')) {
    const button=document.createElement('button');button.type='button';button.className='mobile-navigation-button';button.setAttribute('aria-label','打开导航');button.innerHTML='<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M4 6h16M4 12h12M4 18h16"/></svg>';button.onclick=window.openNavigation;header.append(button);
  }
  document.addEventListener('visibilitychange',()=>{if(document.hidden){rememberWorkDraft();clearTimeout(refreshTimer);}else{checkConnection();scheduleRefresh();}});
  window.addEventListener('pagehide',()=>{rememberWorkDraft();rememberNoteDraft();clearTimeout(refreshTimer);});
  window.addEventListener('pageshow',()=>{checkConnection();scheduleRefresh();});
  window.addEventListener('online',()=>{checkConnection();scheduleRefresh();});
  window.addEventListener('offline',()=>{$('connectionBanner').hidden=false;$('mobileConnection').textContent='网络已断开';});
  window.addEventListener('popstate',()=>{const page=location.hash.slice(1)||'home';restoringHistory=true;window.navigateMobile(['home','wechat','memory','mind','diary','notes','todo','work'].includes(page)?page:'home');restoringHistory=false;});
  document.addEventListener('keydown',event=>{
    if(event.key!=='Escape')return;
    if(state.work.processOpen){event.stopImmediatePropagation();closeProcessSheet();return;}
    if(!document.querySelector('.modal-overlay:not(.hidden)')&&!$('navigationDrawer').classList.contains('open')){
      if(state.page==='diary'&&state.diary.selected){event.stopImmediatePropagation();diaryBack();}
      else if(state.page==='work'&&state.work.id){event.stopImmediatePropagation();workBack();}
    }
  },true);
  window.MobileUI={diaryBack,workBack,decorateNotes,filterPlans,filterNotes,restoreNoteDraft,clearNoteDraft,inputDateTime};
  renderOwnerControls();checkConnection();scheduleRefresh();
  const initial=location.hash.slice(1);if(['wechat','memory','mind','diary','notes','todo','work'].includes(initial)){restoringHistory=true;window.navigateMobile(initial);restoringHistory=false;}else history.replaceState({mobilePage:'home'},'','#home');
})();
