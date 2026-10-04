import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { ChevronDown, SlidersHorizontal, Code2 } from 'lucide-react';
import './ReasoningConfigField.css';
import { decodeRequestBody, encodeRequestBody, dynamicRequestTemplate, type RequestBodyMode } from './requestBodyConfig';
export type ReasoningPref = { mode: string; effort?: string | null; budget_tokens?: number | null };
type Info = { known: boolean; supportsDisable: boolean; efforts: string[]; budget: {min:number;max:number} | null; source: string | null; verifiedAt: string | null; preview: unknown; baseBody: Record<string, unknown>; sampling?: { temperaturePath?: string | null; maxTokensPath?: string | null }; adapterOrigin?: string };
export default function ReasoningConfigField({label, value, onChange, providerType, model, overrides, onOverridesChange, t, temperature=0.7, maxTokens=2048, sendTemperature=true, sendMaxTokens=true}: {
  label:string; help?:string; value:ReasoningPref|null|undefined; onChange:(v:ReasoningPref|null)=>void;
  providerType:string; model:string; overrides?:Record<string, unknown>|null;
  onOverridesChange:(v:Record<string, unknown>|null)=>void; t:(k:string)=>string;
  temperature?:number; maxTokens?:number; sendTemperature?:boolean; sendMaxTokens?:boolean;
}) {
  const {i18n} = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const [info,setInfo] = useState<Info|null>(null);
  const [error,setError] = useState('');
  const [draft,setDraft] = useState('');
  const [draftError,setDraftError] = useState('');
  const [applying,setApplying] = useState(false);
  const [bodyMode,setBodyMode] = useState<RequestBodyMode>('merge');
  const [draftPreview,setDraftPreview] = useState<Info|null>(null);
  const [sampleDraft,setSampleDraft] = useState('');
  const [edited,setEdited] = useState(false);
  const [previewing,setPreviewing] = useState(false);
  const identity = `${providerType}\n${model}`;
  const prefJson = JSON.stringify(value ?? null), patchJson = JSON.stringify(overrides ?? null);
  const requestIdentity = JSON.stringify([identity,prefJson,patchJson,temperature,maxTokens,sendTemperature,sendMaxTokens]);
  const currentIdentity = useRef(requestIdentity); currentIdentity.current = requestIdentity;
  useEffect(()=>{const decoded=decodeRequestBody(JSON.parse(patchJson));setBodyMode(decoded.mode);setDraft(patchJson==='null'?'':JSON.stringify(decoded.body,null,2));setDraftError('');setDraftPreview(null);setEdited(false);},[patchJson,identity]);
  useEffect(()=>{setSampleDraft('');setDraftPreview(null);},[identity]);
  useEffect(()=>{setDraftPreview(null);},[prefJson,temperature,maxTokens,sendTemperature,sendMaxTokens]);
  useEffect(()=>{
    let active=true; setInfo(null); setError('');
    const timer=setTimeout(()=>{invoke<Info>('preview_reasoning_config',{providerType,model,preference:JSON.parse(prefJson),overrides:JSON.parse(patchJson),temperature,maxTokens,sendTemperature,sendMaxTokens})
      .then(v=>{if(active)setInfo(v);}).catch(e=>{if(active)setError(String(e));});},180);
    return ()=>{active=false;clearTimeout(timer);};
  },[providerType,model,prefJson,patchJson,temperature,maxTokens,sendTemperature,sendMaxTokens]);
  useEffect(()=>{
    let active=true;
    setPreviewing(false);
    if(!edited || applying)return;
    const timer=setTimeout(async()=>{
      setPreviewing(true);
      try {
        const patch=encodeRequestBody(bodyMode,draft);
        const result=await invoke<Info>('preview_reasoning_config',{providerType,model,preference:JSON.parse(prefJson),overrides:patch,temperature,maxTokens,sendTemperature,sendMaxTokens,sampleBody:sampleDraft.trim()?JSON.parse(sampleDraft):null});
        if(active){setDraftPreview(result);setDraftError('');}
      }catch(e){if(active)setDraftError(String(e));}
      finally{if(active)setPreviewing(false);}
    },3000);
    return()=>{active=false;clearTimeout(timer);};
  },[edited,applying,draft,bodyMode,sampleDraft,requestIdentity]);
  const mode=(value?.mode ?? 'auto').toLowerCase();
  const effort=(value?.effort ?? '').toLowerCase();
  const apply=async()=>{
    setApplying(true);setDraftError('');
    try {
      const patch = encodeRequestBody(bodyMode,draft);
      const result=await invoke<Info>('preview_reasoning_config',{providerType,model,preference:value??null,overrides:patch,temperature,maxTokens,sendTemperature,sendMaxTokens,sampleBody:sampleDraft.trim()?JSON.parse(sampleDraft):null});
      if(currentIdentity.current!==requestIdentity)return;
      onOverridesChange(patch);setInfo(result);setDraftPreview(null);setEdited(false);
    } catch(e){if(currentIdentity.current===requestIdentity)setDraftError(String(e));} finally {setApplying(false);}
  };
  const shown=draftPreview??info;
  const control:React.CSSProperties={padding:'7px 10px',borderRadius:8,border:'1px solid var(--panel-border)',background:'var(--panel-surface)',color:'var(--panel-text)',fontFamily:'inherit'};
  return <div className="settings-field reasoning-config" style={{marginBottom:18}}>
    <label style={{display:'block',marginBottom:7}}>{label}</label>
    <select aria-label={label} style={{...control,width:'100%'}} value={mode} onChange={e=>onChange({mode:e.target.value==='auto'?'Auto':e.target.value==='on'?'On':'Off'})}>
      {['auto','off','on'].map(m=><option key={m} value={m} disabled={m!=='auto' && (!info?.known || (m==='off'&&!info.supportsDisable))}>{t(`config.reasoning_mode_${m}`)}</option>)}
    </select>
    {mode==='on' && info?.known && <div style={{display:'grid',gap:8,marginTop:8}}>
      <select aria-label={zh?'思考强度':'Reasoning effort'} style={{...control,width:'100%'}} value={effort} onChange={e=>onChange({...value,mode:'On',effort:e.target.value||null})}>
        <option value="">{t('config.reasoning_effort_default')}</option>{info.efforts.map(e=><option key={e} value={e}>{t(`config.reasoning_effort_${e}`)}</option>)}
      </select>
      {info.budget && <input aria-label={zh?'思考 token 预算':'Thinking token budget'} type="number" placeholder={zh?'思考 token 预算':'Thinking token budget'} min={info.budget.min} max={info.budget.max} value={value?.budget_tokens??''} style={control} onChange={e=>onChange({...value,mode:'On',budget_tokens:e.target.value?Math.max(info.budget!.min,Math.min(info.budget!.max,Number(e.target.value))):null})}/>}
    </div>}
    <div className="reasoning-help">
      <div>{mode==='auto' ? (zh?'自动：由服务商决定是否思考及思考强度，应用不发送思考控制参数。':'Auto: the provider decides whether and how deeply to reason. No reasoning controls are sent.') : mode==='off' ? (zh?'关闭：请求模型停用思考，仅适用于支持关闭思考的模型。':'Off: requests reasoning to be disabled, for models that support it.') : (zh?'开启：请求模型启用思考；可用的强度和 token 预算取决于模型支持情况。未指定的选项按模型适配配置或服务商默认值处理。':'On: requests reasoning with the effort and token budget supported by this model. Unspecified options follow adapter settings or provider defaults.')}</div>
      {overrides && <div>{zh?'已设置 JSON 覆盖，最终参数可能与上面的选择不同。请在 API 请求配置中核对。':'JSON overrides are active and may change these choices. Check API request configuration.'}</div>}
      {info&&!info.known && <div>{zh?'尚无此模型的能力资料，可通过下面的 JSON 手动配置，并联网核对预设。':'No capability metadata for this model. Configure JSON below or verify presets online.'}</div>}
      {mode==='off'&&info&&!info.supportsDisable&&<div style={{color:'#b45309'}}>{zh?'此模型未确认支持关闭思考；关闭请求可能不会生效，建议改为自动。':'Disabling reasoning is not confirmed for this model and may not take effect. Select Auto.'}</div>}
      {mode==='on' && effort && info?.known && !info.efforts.includes(effort) && <div style={{color:'#b45309'}}>{zh?'原有强度不在支持列表中，请重新选择。':'The saved effort is unsupported; select a supported level.'}</div>}
    </div>
    <details className="reasoning-advanced"><summary>
      <SlidersHorizontal size={16} aria-hidden="true"/><span className="reasoning-summary-copy"><strong>{zh?'API 请求配置':'API request configuration'}</strong><span>{zh?'完整请求体 · 动态模板 · 示例预览':'Full request body · Dynamic templates · Example preview'}</span></span>
      {overrides && <span className="reasoning-badge">{zh?'已覆盖':'Overrides active'}</span>}<ChevronDown size={16} className="reasoning-chevron" aria-hidden="true"/>
    </summary>
     <div className="reasoning-advanced-body">
      <div className="reasoning-advanced-grid">
       <section className="reasoning-editor"><h4><Code2 size={15} aria-hidden="true"/>{zh?'自定义请求体':'Custom request body'}</h4>
        <label className="reasoning-mode-label">{zh?'覆盖方式':'Override mode'}<select style={control} value={bodyMode} disabled={applying} onChange={e=>{setBodyMode(e.target.value as RequestBodyMode);setDraftPreview(null);setEdited(true);setDraftError('');}}>
         <option value="merge">{zh?'合并覆盖（推荐）':'Merge overrides (recommended)'}</option><option value="replace">{zh?'完全替换':'Replace entire body'}</option>{bodyMode==='legacy'&&<option value="legacy">{zh?'旧版：思考与采样参数':'Legacy: reasoning and sampling'}</option>}
        </select></label>
        <p>{bodyMode==='replace'?(zh?'仅发送模板生成的 JSON，不补回表单字段。null 保留为 JSON 空值。':'Sends only the rendered template; form fields are not restored. null remains a JSON null.'):bodyMode==='merge'?(zh?'覆盖任意请求字段，对象递归合并，数组整体替换；null 删除字段。':'Overrides any body field. Objects merge recursively, arrays replace entirely, and null deletes a field.'):(zh?'保留旧版参数限制与发送开关优先级；切换到合并覆盖可编辑任意字段。':'Preserves legacy restrictions and sending switch precedence. Choose merge to edit any field.')}</p>
        <textarea aria-label={zh?'自定义请求体 JSON':'Custom request body JSON'} spellCheck={false} disabled={applying} placeholder={info?JSON.stringify(dynamicRequestTemplate(info.baseBody),null,2):'{}'} value={draft} onChange={e=>{setDraft(e.target.value);setDraftPreview(null);setEdited(true);setDraftError('');}} rows={12}/>
        {!draft.trim()&&<p>{zh?'输入区内显示动态模板参考，不会作为覆盖发送。合并模式下留空并应用可清除覆盖。':'The empty editor shows a dynamic template reference; it is not sent. Apply an empty merge configuration to clear overrides.'}</p>}
        <div className="reasoning-actions">
        <button type="button" disabled={applying||!draft.trim()} onClick={()=>{try{const config=encodeRequestBody(bodyMode,draft);setDraft(JSON.stringify(decodeRequestBody(config).body,null,2));setDraftError('');setEdited(true);setDraftPreview(null);}catch(e){setDraftError(String(e));}}}>{zh?'格式化 JSON':'Format JSON'}</button>
        <button type="button" className="reasoning-apply" disabled={applying} onClick={()=>void apply()}>{applying?(zh?'校验中…':'Validating…'):(zh?'应用配置':'Apply configuration')}</button></div>
        <p>{zh?'完整覆盖优先于思考、温度、输出长度及其发送开关。应用后再保存设置。API 地址与认证请求头仍使用连接配置。请求体和响应格式需符合所选协议；改变 stream 或工具结构可能影响流式输出和工具执行。':'Full overrides take precedence over reasoning, temperature, output limits and their sending switches. Apply, then save settings. Endpoint and authentication headers use connection settings. Body and response formats must match the selected protocol; changing stream or tools may affect streaming and tool execution.'}</p>
        <details className="reasoning-template-help"><summary>{zh?'动态字段怎么写？':'How do dynamic fields work?'}</summary><p>{zh?'使用 {"$requestRef":"/messages"} 引用本次原始请求的字段，保留其 JSON 类型。路径采用 JSON Pointer；不存在的字段得到 null。{"$requestRef":""} 引用完整原始请求。此标记与 JSON Schema 的 $ref 不冲突。':'Use {"$requestRef":"/messages"} to copy a field from the current generated body, preserving its JSON type. Paths use JSON Pointer; missing fields yield null. {"$requestRef":""} copies the entire original body. JSON Schema $ref is unchanged.'}</p><p>{zh?'Chat Completions 消息：/messages；Responses 输入：/input；Anthropic 系统提示：/system；Gemini 内容：/contents；Agents 配置：/agent。固定消息不会随对话更新。':'Chat Completions messages: /messages; Responses input: /input; Anthropic system prompt: /system; Gemini contents: /contents; Agents configuration: /agent. Literal messages do not update with the conversation.'}</p></details>
        {draftError&&<div role="alert" className="reasoning-error">{draftError}</div>}
       </section>
       <section className="reasoning-preview"><h4>{zh?'完整请求体预览':'Full request body preview'}</h4><p aria-live="polite">{previewing?(zh?'正在预览草稿…':'Previewing draft…'):draftPreview?(zh?'草稿预览：尚未应用或保存。':'Draft preview: not applied or saved.'):edited?(zh?'停止编辑 3 秒后自动预览；当前结果尚未更新。':'Preview updates after 3 seconds without edits; the result below has not updated yet.'):(zh?'已应用配置的示例预览，不含真实对话。':'Example preview using applied settings, without real conversation.')}</p><pre aria-busy={previewing}>{shown?JSON.stringify(shown.preview,null,2):error?(zh?'预览加载失败':'Preview failed to load'):(zh?'正在加载预览…':'Loading preview…')}</pre>
        <details className="reasoning-template-help"><summary>{zh?'编辑示例输入（不保存）':'Edit example input (not saved)'}</summary><textarea aria-label={zh?'示例输入 JSON':'Example input JSON'} spellCheck={false} disabled={applying} rows={8} placeholder={info?JSON.stringify(info.baseBody,null,2):'{}'} value={sampleDraft} onChange={e=>{setSampleDraft(e.target.value);setDraftPreview(null);setEdited(true);setDraftError('');}}/><p>{zh?'留空使用协议默认示例。停止编辑 3 秒后自动预览，不会发送 API 请求。':'Leave empty for the default protocol example. Preview updates after 3 seconds without edits; no API request is sent.'}</p></details>
       </section>
      </div>
      <div className="reasoning-adapter">
       <strong>{zh?'模型适配信息':'Model adapter'}</strong>
       {info?.adapterOrigin==='compatibility' && <span>{zh?'使用内置兼容配置；实际支持情况以服务商为准。':'Using built-in compatibility settings; actual support depends on the provider.'}</span>}
       {info?.sampling && <span>{zh?'请求字段映射':'Request field mapping'}: temperature → {info.sampling.temperaturePath ?? '—'} · max tokens → {info.sampling.maxTokensPath ?? '—'}</span>}
       {info?.source && <span><a href={info.source} target="_blank" rel="noreferrer">{zh?'查看能力来源':'View capability source'}</a>{info.verifiedAt && ` · ${zh?'核对日期':'Verified'}: ${info.verifiedAt}`}</span>}
      </div>
     </div>
    </details>
    {error&&<div role="alert" style={{color:'#dc2626'}}>{error}</div>}
  </div>;
}
