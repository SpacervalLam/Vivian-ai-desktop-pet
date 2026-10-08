/* global MemoryPresentation, api, memActiveChar, memCharList, currentCharId, charName, showModal, hideModal */
(() => {
  const { conversationThreads, sharedEventTimeline, isVisibleMemoryFact, isMemorySummary, splitActionText } = MemoryPresentation;
  const tabs = [
    ['facts', '长期记忆', '事实、偏好和约定', 'M20 4c-3-3-6-1-8 1-2-2-5-4-8-1-4 4 2 10 8 15 6-5 12-11 8-15Z'],
    ['episodes', '共同经历', '跨会话的事件与进展', 'M12 6c-3-3-7-3-9-2v15c3-1 6-1 9 2 3-3 6-3 9-2V4c-2-1-6-1-9 2Zm0 0v15'],
    ['recent', '对话记录', '每次交流的原始发言', 'M21 11a8 8 0 0 1-8 8H7l-4 3V5a2 2 0 0 1 2-2h8a8 8 0 0 1 8 8Z'],
    ['profile', '用户画像', '关于你的了解', 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8ZM4 21v-2a8 8 0 0 1 16 0v2']
  ];
  const phases = { planned: '计划与约定', started: '开始', progressed: '进展', completed: '已完成', cancelled: '已取消' };
  const topicNames = { preference: '偏好', identity: '身份', user_profile: '关于你', project_context: '项目', relationship: '关系', health: '健康', reference: '参考', knowledge: '知识' };
  const basicFields = [['name','姓名'],['age','年龄'],['gender','性别'],['occupation','职业'],['location','所在地'],['birthday','生日']];
  const state = { owner: '', layer: 'facts', query: '', items: [], records: [], profile: null, discovery: null, profileError: '', discoveryError: '', loading: false, request: 0, expanded: new Set(), summaryViews: new Set(), organizing: '', error: '', notice: '' };
  const $ = id => document.getElementById(id);
  const e = text => String(text ?? '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');
  const idAttr = id => e(encodeURIComponent(id));
  const date = value => {
    const time = Number(value); if (!Number.isFinite(time) || time <= 0) return '时间未知';
    return new Date(time < 1e12 ? time * 1000 : time).toLocaleString('zh-CN', { month:'numeric', day:'numeric', hour:'2-digit', minute:'2-digit' });
  };
  function highlighted(value) {
    const text = String(value ?? ''), term = state.query.trim();
    if (!term) return e(text);
    let cursor = 0, index = 0, result = '';
    const lower = text.toLowerCase(), search = term.toLowerCase();
    while ((index = lower.indexOf(search, cursor)) >= 0) {
      result += e(text.slice(cursor,index)) + '<mark class="memory-highlight">' + e(text.slice(index,index+term.length)) + '</mark>';
      cursor = index + term.length;
    }
    return result + e(text.slice(cursor));
  }
  const dialogue = text => splitActionText(text).map(part => part.action ? '<em class="memory-action-text">'+highlighted(part.text)+'</em>' : highlighted(part.text)).join('');
  const button = (action, id, label, disabled = false) => `<button type="button" data-action="${action}" data-id="${idAttr(id)}"${disabled?' disabled':''}>${e(label)}</button>`;
  const discoveryButton = (action,value,label,response='') => `<button type="button" data-action="discovery" data-operation="${action}" data-response="${response}" data-id="${idAttr(value)}">${e(label)}</button>`;
  const matches = text => String(text).toLowerCase().includes(state.query.trim().toLowerCase());
  const empty = (title, copy) => `<div class="memory-empty"><strong>${e(title)}</strong><p>${e(copy)}</p></div>`;
  const endpoint = () => '/api/characters/' + encodeURIComponent(state.owner);

  let projectionCache;
  function derived() {
    if (projectionCache?.owner===state.owner && projectionCache.items===state.items && projectionCache.records===state.records) return projectionCache.value;
    const facts = state.items.filter(isVisibleMemoryFact).sort((a,b) => Number(b.metadata?.ended_at ?? b.created_at) - Number(a.metadata?.ended_at ?? a.created_at));
    const threads = conversationThreads(state.records, state.owner);
    const events = sharedEventTimeline(state.items, state.records);
    const summaries = new Map(state.items.filter(item => isMemorySummary(item) && !item.consolidated && item.metadata?.index_active !== false).map(item => [String(item.metadata?.conversation_id ?? ''), item]));
    const value = { facts, threads, events, summaries };
    projectionCache = {owner:state.owner,items:state.items,records:state.records,value};
    return value;
  }
  function renderControls(counts) {
    $('memTabs').replaceChildren();
    const characters = memCharList.length ? memCharList : [{id:state.owner,name:charName(state.owner)}];
    characters.forEach(character => {
      const b = document.createElement('button'); b.type = 'button'; b.className = 'mem-tab' + (character.id === state.owner ? ' active' : '');
      b.textContent = character.name; b.setAttribute('aria-pressed', String(character.id === state.owner));
      b.onclick = () => { memActiveChar = character.id; void window.loadMemories(); }; $('memTabs').appendChild(b);
    });
    $('memoryLayers').innerHTML = tabs.map(([key,title,subtitle,path]) => `<button class="memory-layer" data-layer="${key}" aria-pressed="${state.layer===key}"><span class="memory-layer-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="${path}"/></svg></span><strong>${title}</strong><small>${subtitle}</small><span class="memory-layer-count">${state.loading?'—':key==='profile'?'↗':counts[key]}</span></button>`).join('');
    $('memoryRefresh').disabled = state.loading;
  }
  function factCard(item, threads) {
    const meta = item.metadata || {}, hooks = (item.open_hooks || []).filter(h => h.closed_at == null);
    const category = hooks.length ? '约定与待办' : item.tags.includes('preference') ? '偏好' : item.tags.includes('relationship') ? '关系' : item.tags.includes('user_profile') ? '关于你' : item.tags.includes('project_context') ? '共同话题' : '事实与资料';
    const quote = typeof meta.source_quote === 'string' ? meta.source_quote : '';
    const original = threads.some(t => t.id === meta.conversation_id);
    const sources = Array.isArray(meta.evidence_sources) ? meta.evidence_sources.filter(s => s && typeof s.quote === 'string' && s.quote !== quote) : [];
    return `<article class="memory-entry"><div class="memory-entry-top"><span class="memory-kind">${category}</span><time>${date(meta.started_at ?? item.created_at)}</time></div>
      ${typeof meta.title==='string'?`<h4>${highlighted(meta.title)}</h4>`:''}<p class="memory-copy">${highlighted(item.content)}</p>
      ${Array.isArray(meta.topics)?`<div class="memory-tags">${meta.topics.filter(t=>typeof t==='string').slice(0,5).map(t=>`<span>${e(topicNames[t]||t)}</span>`).join('')}</div>`:''}
      ${quote && quote!==item.content?`<div class="memory-evidence">来自原话<br>「${highlighted(quote)}」</div>`:''}
      ${hooks.map(h=>`<div class="memory-hook">待跟进 · ${highlighted(h.condition)}</div>`).join('')}
      ${sources.length?`<details class="memory-evidence"><summary>更多原话证据 · ${sources.length} 条</summary>${sources.map(s=>`<p>「${highlighted(s.quote)}」</p>${threads.some(t=>t.id===s.conversation_id)?`<div class="memory-actions">${button('original',s.conversation_id,'查看来源会话')}</div>`:''}`).join('')}</details>`:''}
      <div class="memory-actions">${original?button('original',meta.conversation_id,'查看原始会话 ↗'):`<span>${quote?'原话提取':'未标注证据'}</span>`}</div></article>`;
  }
  function eventCard(event) {
    return `<article class="memory-entry"><h4>${highlighted(event.title)}</h4><div class="memory-thread-meta">${event.conversationIds.length} 次会话 · ${event.progress.length} 条进展</div><ol class="memory-event-progress">${event.progress.map(p=>`<li><div class="memory-entry-top"><span class="memory-phase ${p.phase}">${phases[p.phase]}</span><time>记录于 ${date(p.recorded_at)}</time></div><p class="memory-copy">${highlighted(p.detail)}</p><details class="memory-evidence"><summary>原话依据</summary><p>「${highlighted(p.source_quote)}」</p><div class="memory-actions">${button('original',p.conversationId,'查看来源会话 ↗')}</div></details></li>`).join('')}</ol></article>`;
  }
  function threadCard(thread, summaries) {
    const summary = summaries.get(thread.id), viewSummary = state.summaryViews.has(thread.id) && summary?.content;
    const turns = state.expanded.has(thread.id) || state.query.trim() ? thread.turns : thread.turns.slice(0,6);
    return `<article class="memory-entry" id="mobile-conversation-${idAttr(thread.id)}"><h4>${highlighted(thread.title)}</h4><div class="memory-thread-meta">${thread.turns.length} 条消息 · ${date(thread.startedAt)}${thread.startedAt!==thread.time?' — '+date(thread.time):''}</div>
      ${viewSummary?`<p class="memory-copy">${highlighted(summary.content)}</p>`:turns.map(turn=>`<div class="memory-turn"><span class="memory-speaker" title="${e(turn.audience?'对 '+turn.audience+' 说':'')}">${e(turn.speaker)}</span><div><p>${dialogue(turn.text)}</p>${turn.sticker?`<img class="memory-sticker" loading="lazy" src="/api/characters/${idAttr(turn.sticker.character_id)}/stickers/${idAttr(turn.sticker.id)}/${idAttr(turn.sticker.version)}" alt="${e(turn.sticker.label||'贴纸')}">`:''}</div></div>`).join('')}
      <div class="memory-actions">${!viewSummary&&thread.turns.length>6&&!state.query.trim()?button('expand',thread.id,state.expanded.has(thread.id)?'收起':'展开全部 '+thread.turns.length+' 条消息'):''}
      ${button('organize',thread.id,state.organizing===state.owner+':'+thread.id?'正在整理…':summary?.metadata?.summary_status==='partial'?'继续整理':summary?'更新摘要':'整理这次对话',!!state.organizing)}
      ${summary?.content?button('summary',thread.id,viewSummary?'查看原始会话':'查看会话摘要'):''}</div>
      ${summary?.metadata?.summary_status==='partial'?`<div class="memory-thread-meta">阶段摘要 · ${e(summary.metadata.completed_parts)}/${e(summary.metadata.total_parts)} 段</div>`:''}</article>`;
  }
  function profileCard(fact, index) {
    return `<article class="memory-entry"><h4>${e(fact.label)}${fact.is_pinned?' · 已锁定':''}</h4><p class="memory-copy">${e(fact.content||'未填写')}</p><div class="memory-actions">${(fact.fact_type!=='custom'?button('edit-fact',String(index),'编辑'):'')}${fact.fact_type!=='custom'?button('pin-fact',String(index),fact.is_pinned?'解锁':'锁定'):''}${fact.content?button('delete-fact',String(index),'删除'):''}</div></article>`;
  }
  let profileFacts = [];
  function profileView() {
    if (state.profileError) return empty('用户画像读取失败',state.profileError);
    const profile = state.profile || {}, basic = profile.basic_facts || [], custom = profile.custom_facts || [], recent = profile.recent_state || {};
    profileFacts = basicFields.map(([key,label]) => basic.find(f=>f.fact_type===key)||{fact_type:key,label,content:''}).concat(custom);
    const name = basic.find(f=>f.fact_type==='name')?.content || '关于你';
    const identity = ['age','gender','occupation','location'].map(key=>basic.find(f=>f.fact_type===key)?.content).filter(Boolean);
    const discovery = state.discovery;
    return `<article class="memory-entry memory-profile-hero"><span class="memory-kind">${e(charName(state.owner))} 眼中的你</span><h4 style="margin-top:12px">${e(name)}</h4><p class="memory-note" style="margin-bottom:0">${e(identity.join(' · ')||'随着每一次交流，慢慢了解你。')}</p></article>
      <h3 class="memory-profile-heading">基础身份</h3><div class="memory-profile-grid">${profileFacts.slice(0,6).map(profileCard).join('')}</div>
      <h3 class="memory-profile-heading">近期状态</h3><article class="memory-entry">${[['近期目标',recent.recent_goals],['当前项目',recent.current_projects],['近期偏好',recent.recent_preferences]].filter(([,values])=>values?.length).map(([label,values])=>`<h4>${label}</h4><p class="memory-copy">${values.map(e).join('<br>')}</p>`).join('')||'<p class="memory-note">暂无近期状态，有新的交流时会慢慢丰富起来。</p>'}</article>
      <div class="memory-toolbar"><h3>自由事实</h3><div class="memory-actions">${button('add-fact','','添加')}</div></div>${custom.length?custom.map((f,i)=>profileCard(f,i+6)).join(''):empty('还没有自由事实','可以补充你希望角色记住的事。')}
      <h3 class="memory-profile-heading">兴趣画像</h3><p class="memory-note">可用内容 ${e(discovery?.store_available??0)} · 已推荐 ${e(discovery?.store_recommended??0)} · 探索开放度 ${Math.round((discovery?.exploration_openness??0)*100)}%</p>${state.discoveryError?empty('兴趣画像读取失败',state.discoveryError):`<article class="memory-entry">${discovery?.interests?.length?discovery.interests.map(i=>`<div class="memory-profile-interest"><strong>${e(i.domain)}</strong><span>${Math.round(i.weight*100)}% · ${e(({seed:'画像种子',feedback:'推荐反馈',probe:'兴趣确认'})[i.source]||i.source)}</span></div><div class="memory-actions">${discoveryButton('remove_interest',i.domain,'移除兴趣')}</div>`).join(''):'<p class="memory-note">还没有已确认的兴趣。</p>'}${discovery?.disliked_topics?.length?`<h4>不喜欢的话题</h4><div class="memory-tags">${discovery.disliked_topics.map(t=>`<span>${e(t)}</span><div class="memory-actions">${discoveryButton('remove_dislike',t,'移除')}</div>`).join('')}</div>`:''}</article><form class="memory-discovery-form" data-discovery-form="add_dislike"><input aria-label="不喜欢的话题" placeholder="添加不喜欢的话题" required><button type="submit">添加</button></form><form class="memory-discovery-form" data-discovery-form="import_bangumi"><input aria-label="Bangumi 用户名" placeholder="Bangumi 公开收藏用户名" required><button type="submit">导入</button></form>${discovery?.probes?.length?`<h3 class="memory-profile-heading">待确认的兴趣</h3>${discovery.probes.map(p=>`<article class="memory-entry"><h4>${e(p.domain)}</h4><p class="memory-copy">${e(p.reason)}</p><div class="memory-tags">${(p.specifics||[]).map(t=>`<span>${e(t)}</span>`).join('')}</div><p class="memory-note" style="margin:12px 0 0">已确认 ${p.confirmation_count}/${p.confirmation_threshold} 次</p><div class="memory-actions">${discoveryButton('respond_probe',p.domain,'感兴趣','confirm')}${discoveryButton('respond_probe',p.domain,'不感兴趣','reject')}${discoveryButton('respond_probe',p.domain,'以后再说','defer')}</div></article>`).join('')}`:''}`}`;
  }
  async function discoveryAction(action, value, response = '') {
    const owner = state.owner;
    if (action.startsWith('remove_') && !confirm('移除这项画像记录？')) return;
    state.error=''; state.notice='';
    try {
      const result = await api(endpoint()+'/profile/discovery',{method:'POST',body:JSON.stringify({action,value,response})});
      if (state.owner!==owner) return;
      state.notice=result.result || '兴趣画像已更新。';
      await window.loadMemories();
    } catch(error) { if(state.owner===owner){state.error=error.message;render();} }
  }
  $('memoryList').addEventListener('submit',event=>{
    const form=event.target.closest('[data-discovery-form]');if(!form)return;
    event.preventDefault(); const value=form.querySelector('input').value.trim();if(!value)return;
    const b=form.querySelector('button');b.disabled=true;
    discoveryAction(form.dataset.discoveryForm,value).finally(()=>b.disabled=false);
  });
  function render() {
    const {facts,threads,events,summaries} = derived(), counts = {facts:facts.length,recent:threads.length,episodes:events.length};
    renderControls(counts);
    const tab = tabs.find(t=>t[0]===state.layer);
    $('memoryLayerTitle').textContent = tab[1];
    $('memoryCount').textContent = state.layer==='profile'?'':`${counts[state.layer]} ${state.layer==='recent'?'组':'条'}`;
    $('memorySearchWrap').hidden = state.layer==='profile';
    $('memoryLayerNote').textContent = state.layer==='recent'?'每张卡片保留一次交流的原始消息和会话摘要；具体事件的进展单独汇入共同经历。':state.layer==='episodes'?'同一事件的计划、进展与结果串成时间线。时间表示原话记录时间，计划不代表已经发生。':'';
    $('memoryStatus').textContent = state.error || state.notice; $('memoryStatus').classList.toggle('is-error',!!state.error);
    $('memoryList').setAttribute('aria-busy',String(state.loading));
    if (state.loading) { $('memoryList').innerHTML = empty('正在读取记忆…',''); return; }
    if (state.layer!=='profile' && state.error && !state.items.length && !state.records.length) { $('memoryList').innerHTML = empty('暂时无法读取记忆','请检查电脑端连接后点击刷新重试。'); return; }
    if (state.layer==='profile') { $('memoryList').innerHTML = profileView(); return; }
    const visible = state.layer==='facts'?facts.filter(item=>matches([item.content,item.metadata?.title,item.metadata?.source_quote,...(Array.isArray(item.metadata?.topics)?item.metadata.topics:[]),...(item.open_hooks||[]).map(h=>h.condition)].join(' '))):state.layer==='episodes'?events.filter(event=>matches(event.searchText)):threads.filter(thread=>matches(thread.searchText));
    $('memoryList').innerHTML = visible.length?visible.map(item=>state.layer==='facts'?factCard(item,threads):state.layer==='episodes'?eventCard(item):threadCard(item,summaries)).join(''):empty(state.query?'没有找到匹配的记忆':`还没有${tab[1]}的记录`,state.query?'试试其他关键词':state.layer==='episodes'?'整理对话后，有原话依据的事件会出现在这里。':'有新的交流时，这里会慢慢丰富起来。');
  }
  window.loadMemories = async () => {
    const owner = memActiveChar || currentCharId; if (!owner) return;
    memActiveChar = owner;
    if (state.owner!==owner) { state.owner=owner; state.items=[]; state.records=[]; state.profile=null; state.discovery=null; state.expanded.clear(); state.summaryViews.clear(); state.notice=''; }
    const request = ++state.request; state.loading=true; state.error=''; render();
    const [memory,profile,discovery] = await Promise.allSettled([api(endpoint()+'/memory'),api(endpoint()+'/profile'),api(endpoint()+'/profile/discovery')]);
    if (state.request!==request || state.owner!==owner) return;
    state.loading=false;
    if (memory.status==='fulfilled') { state.items=memory.value.memories||[]; state.records=memory.value.conversations||[]; }
    else state.error = '记忆读取失败：'+memory.reason.message;
    state.profile = profile.status==='fulfilled'?profile.value:null;
    state.profileError = profile.status==='rejected'?profile.reason.message:'';
    state.discovery = discovery.status==='fulfilled'?discovery.value:null;
    state.discoveryError = discovery.status==='rejected'?discovery.reason.message:'';
    render();
  };
  $('memoryLayers').addEventListener('click',event=>{
    const layer = event.target.closest('[data-layer]')?.dataset.layer;
    if (layer) { state.layer=layer; render(); }
  });
  window.selectMemoryLayer = layer => {
    if (!tabs.some(tab=>tab[0]===layer)) return;
    state.layer=layer;
    if (state.owner) render();
  };
  $('memorySearch').addEventListener('input',event=>{ state.query=event.target.value; render(); });
  async function organize(id) {
    if (state.organizing) return;
    const owner=state.owner; state.organizing=owner+':'+id; state.error=''; state.notice=''; render();
    try {
      const summary = await api(endpoint()+'/memory/conversations/'+encodeURIComponent(id)+'/summarize',{method:'POST'});
      if (state.owner!==owner) return;
      state.notice = summary.metadata?.summary_status==='no_content'?'这次交流无需额外摘要，原始消息已保留。':summary.metadata?.summary_status==='partial'?'阶段摘要已保存，剩余内容可以继续整理。':'会话摘要已更新，有依据的事件进展会进入共同经历。';
      await window.loadMemories();
    } catch(error) { if (state.owner===owner) state.error='整理失败：'+error.message; }
    finally { state.organizing=''; render(); }
  }
  function showOriginal(id) {
    state.layer='recent'; state.query=''; $('memorySearch').value=''; state.expanded.add(id); state.summaryViews.delete(id); render();
    requestAnimationFrame(()=>document.getElementById('mobile-conversation-'+encodeURIComponent(id))?.scrollIntoView({block:'start',behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'auto':'smooth'}));
  }
  function editProfileFact(index) {
    const fact = index==null?{fact_type:'custom',label:'自由事实',content:''}:profileFacts[index];
    const owner = state.owner;
    let overlay = $('memoryFactModal');
    if (!overlay) {
      overlay=document.createElement('div');overlay.id='memoryFactModal';overlay.className='modal-overlay hidden';
      overlay.innerHTML='<form class="modal" role="dialog" aria-modal="true" aria-labelledby="memoryFactTitle"><h3 id="memoryFactTitle">编辑画像</h3><div class="field"><label for="memoryFactInput" id="memoryFactLabel"></label><textarea id="memoryFactInput" rows="3" required></textarea></div><p id="memoryFactError" class="memory-status" role="alert"></p><div class="modal-actions"><button type="button" class="btn outline" id="memoryFactCancel">取消</button><button type="submit" class="btn" id="memoryFactSave">保存</button></div></form>';
      document.body.appendChild(overlay);$('memoryFactCancel').onclick=()=>hideModal('memoryFactModal');
      overlay.onclick=event=>{if(event.target===overlay)hideModal('memoryFactModal');};
    }
    $('memoryFactInput').value=fact.content; $('memoryFactLabel').textContent=fact.label; $('memoryFactError').textContent='';
    overlay.querySelector('form').onsubmit=async event=>{
      event.preventDefault();const content=$('memoryFactInput').value.trim();if(!content)return;
      $('memoryFactSave').disabled=true;
      try { await api('/api/characters/'+encodeURIComponent(owner)+'/profile/'+encodeURIComponent(fact.fact_type),{method:'PUT',body:JSON.stringify({content,pinned:!!fact.is_pinned})});hideModal('memoryFactModal');if(state.owner===owner)await window.loadMemories(); }
      catch(error){$('memoryFactError').textContent=error.message;}
      finally{$('memoryFactSave').disabled=false;}
    };
    showModal('memoryFactModal');
  }
  async function mutateFact(index, action) {
    const fact=profileFacts[index],owner=state.owner;
    if (action==='delete-fact' && !confirm('删除这条画像事实？')) return;
    try {
      const path=endpoint()+'/profile/'+encodeURIComponent(fact.fact_type);
      await api(action==='pin-fact'?path+'/pin':path+'?content='+encodeURIComponent(fact.content),action==='pin-fact'?{method:'POST',body:JSON.stringify({pinned:!fact.is_pinned})}:{method:'DELETE'});
      if(state.owner===owner)await window.loadMemories();
    } catch(error){if(state.owner===owner){state.error=error.message;render();}}
  }
  $('memoryList').addEventListener('click',event=>{
    const b=event.target.closest('button[data-action]');if(!b)return;
    const id=decodeURIComponent(b.dataset.id),action=b.dataset.action;
    if(action==='original')showOriginal(id);
    else if(action==='organize')void organize(id);
    else if(action==='expand'||action==='summary'){const set=action==='expand'?state.expanded:state.summaryViews;set.has(id)?set.delete(id):set.add(id);render();}
    else if(action==='discovery'){b.disabled=true;void discoveryAction(b.dataset.operation,id,b.dataset.response).finally(()=>b.disabled=false);}
    else if(action==='add-fact')editProfileFact(null);
    else if(action==='edit-fact')editProfileFact(Number(id));
    else if(action==='pin-fact'||action==='delete-fact')void mutateFact(Number(id),action);
  });
})();
