(() => {
  'use strict';
  const root = document.getElementById('mobileAgreements');
  if (!root) return;
  const escape = value => String(value ?? '').replace(/[&<>"']/g, char => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
  const state = { records: [], todos: [], filter: 'active', selected: '', request: 0, error: '', busy: false };
  const endpoint = record => '/api/characters/' + encodeURIComponent(record.owner) + '/agreements';
  const button = (action, record, label) => `<button type="button" class="btn small outline" data-agreement-action="${action}" data-owner="${escape(record.owner)}" data-id="${escape(record.id)}"${state.busy?' disabled':''}>${escape(label)}</button>`;
  function render() {
    const records = state.records.filter(record => (state.filter === 'all' || record.task.status === state.filter) && (!state.selected || state.selected === record.owner + ':' + record.id));
    root.innerHTML = `<details class="mobile-agreements"${state.selected?' open':''}><summary>关注与约定 · ${state.records.filter(r=>r.task.status==='active').length}</summary>
      <p class="mobile-caption">持续关注和维护放在这里；需要完成的具体行动关联到待办。</p>
      <div class="mobile-segments">${['active','candidate','paused','all'].map(value=>`<button type="button" data-agreement-filter="${value}" aria-pressed="${state.filter===value}">${({active:'维护中',candidate:'候选关注',paused:'已暂停',all:'全部'})[value]}</button>`).join('')}</div>
      ${state.selected?'<button type="button" class="btn small outline" data-agreement-clear>查看全部约定</button>':''}
      ${state.error?`<p class="error" role="alert">${escape(state.error)}</p>`:''}
      <div class="mobile-agreement-list">${records.map(record=>{
        const task=record.task;
        const todos=state.todos.filter(todo=>todo.agreement?.id===record.id&&todo.agreement.character_id===record.owner);
        return `<article class="card" data-agreement-card="${escape(record.id)}"><h4>${escape(task.title)}</h4>
          <div class="mobile-caption">${escape(record.owner==='nana'?'Nana':'Vivian')} · ${escape(MemoryPresentation.ongoingLabel(task.status))}</div>
          <p>${escape(task.goal)}</p><p class="mobile-caption">更新线索：${escape(task.trigger)}</p>
          <div class="actions">${task.status==='active'?button('paused',record,'暂停维护'):button('active',record,task.status==='candidate'?'启用约定':'恢复维护')}
          ${!['cancelled','completed'].includes(task.status)?button('cancelled',record,'停止维护'):''}${task.status==='active'?button('todo',record,'新增关联待办'):''}</div>
          <details><summary>对应清单 · ${(task.entries||[]).length}</summary><ul>${(task.entries||[]).map(entry=>`<li><strong>${escape(entry.label)}</strong> · ${escape(MemoryPresentation.ongoingLabel(entry.state))}<p>${escape(entry.detail)}</p><blockquote>${escape(entry.evidence.quote)}</blockquote></li>`).join('')}</ul></details>
          ${todos.length?`<div class="mobile-agreement-todos">${todos.map(todo=>`<div><span>${escape(todo.title)} · ${todo.completed?'已完成':'待完成'}</span>${!todo.completed?`<button type="button" class="btn small outline" data-agreement-complete="${escape(todo.id)}">完成待办</button>`:''}</div>`).join('')}</div>`:''}</article>`;
      }).join('') || '<p class="mobile-caption">暂无对应的关注与约定</p>'}</div>
      <details><summary>新增约定</summary><form id="mobileAgreementForm" class="mobile-agreement-form">
        <label>交给谁<select name="owner"><option value="vivian">Vivian</option><option value="nana">Nana</option></select></label>
        <label>标题<input name="title" required maxlength="100"></label>
        <label>持续维护什么<textarea name="goal" required maxlength="500"></textarea></label>
        <label>遇到什么信息时更新<textarea name="trigger" required maxlength="300"></textarea></label>
        <button class="btn outline"${state.busy?' disabled':''}>保存并启用</button></form></details></details>`;
  }
  async function load() {
    const request=++state.request;
    try {
      const values=await Promise.all(['vivian','nana'].map(async owner=>{
        const result=await api('/api/characters/'+owner+'/agreements');
        return (result.agreements||[]).filter(record=>record.task&&Array.isArray(record.task.entries)).map(record=>({...record,owner}));
      }));
      if(request!==state.request)return;
      state.records=values.flat();state.error='';render();
    } catch(error) {if(request===state.request){state.error=error.message;render();}}
  }
  root.addEventListener('click',async event=>{
    const filter=event.target.closest('[data-agreement-filter]');
    if(filter){state.filter=filter.dataset.agreementFilter;render();root.querySelector('.mobile-agreements').open=true;return;}
    if(event.target.closest('[data-agreement-clear]')){state.selected='';render();root.querySelector('.mobile-agreements').open=true;return;}
    const complete=event.target.closest('[data-agreement-complete]');
    if(complete){complete.disabled=true;await toggleTodo(complete.dataset.agreementComplete);return;}
    const control=event.target.closest('[data-agreement-action]');if(!control||state.busy)return;
    const record=state.records.find(item=>item.id===control.dataset.id&&item.owner===control.dataset.owner);if(!record)return;
    const action=control.dataset.agreementAction;
    if(action==='todo'){
      newTodo();window.agreementTodoOrigin={character_id:record.owner,id:record.id,title:record.task.title};
      document.getElementById('todoModalTitle').textContent='新增关联待办 · '+record.task.title;return;
    }
    state.busy=true;control.disabled=true;
    try {await api(endpoint(record)+'/'+encodeURIComponent(record.id)+'/state',{method:'POST',body:JSON.stringify({revision:record.task.revision,status:action})});await load();}
    catch(error){state.error=error.message;render();}
    finally{state.busy=false;render();root.querySelector('.mobile-agreements').open=true;}
  });
  root.addEventListener('submit',async event=>{
    if(event.target.id!=='mobileAgreementForm')return;
    event.preventDefault();if(state.busy)return;
    const data=new FormData(event.target),owner=String(data.get('owner'));state.busy=true;let saved=false;
    const submit=event.target.querySelector('button');submit.disabled=true;
    try {await api('/api/characters/'+encodeURIComponent(owner)+'/agreements',{method:'POST',body:JSON.stringify({title:data.get('title'),goal:data.get('goal'),trigger:data.get('trigger')})});await load();saved=true;}
    catch(error){state.error=error.message;const errorNode=document.createElement('p');errorNode.className='error';errorNode.setAttribute('role','alert');errorNode.textContent=error.message;event.target.appendChild(errorNode);}
    finally{state.busy=false;if(saved){render();root.querySelector('.mobile-agreements').open=true;}else if(submit.isConnected)submit.disabled=false;}
  });
  window.MobileAgreements={load,setTodos(todos){state.todos=todos;render();},focus(ref){state.selected=ref.character_id+':'+ref.id;state.filter='all';navigateMobile('todo');load().then(()=>root.scrollIntoView({block:'start'}));}};
  render();
})();
