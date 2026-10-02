import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
export type ReasoningPref = { mode: string; effort?: string | null; budget_tokens?: number | null };
type Info = { known: boolean; supportsDisable: boolean; efforts: string[]; budget: {min:number;max:number} | null; source: string | null; verifiedAt: string | null; preview: unknown };
export default function ReasoningConfigField({label, help, value, onChange, providerType, model, overrides, onOverridesChange, t}: {
  label:string; help?:string; value:ReasoningPref|null|undefined; onChange:(v:ReasoningPref|null)=>void;
  providerType:string; model:string; overrides?:Record<string, unknown>|null;
  onOverridesChange:(v:Record<string, unknown>|null)=>void; t:(k:string)=>string;
}) {
  const {i18n} = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const [info,setInfo] = useState<Info|null>(null);
  const [error,setError] = useState('');
  const [draft,setDraft] = useState('');
  const [draftError,setDraftError] = useState('');
  const [applying,setApplying] = useState(false);
  const prefJson = JSON.stringify(value ?? null), patchJson = JSON.stringify(overrides ?? null);
  useEffect(()=>{setDraft(overrides ? JSON.stringify(overrides,null,2) : '');setDraftError('');},[patchJson]);
  useEffect(()=>{
    let active=true; setInfo(null); setError('');
    const timer=setTimeout(()=>{invoke<Info>('preview_reasoning_config',{providerType,model,preference:JSON.parse(prefJson),overrides:JSON.parse(patchJson)})
      .then(v=>{if(active)setInfo(v);}).catch(e=>{if(active)setError(String(e));});},180);
    return ()=>{active=false;clearTimeout(timer);};
  },[providerType,model,prefJson,patchJson]);
  const mode=(value?.mode ?? 'auto').toLowerCase();
  const effort=(value?.effort ?? '').toLowerCase();
  const apply=async()=>{
    setApplying(true);setDraftError('');
    try {
      const patch = draft.trim() ? JSON.parse(draft) : null;
      await invoke('preview_reasoning_config',{providerType,model,preference:value??null,overrides:patch});
      onOverridesChange(patch);
    } catch(e){setDraftError(String(e));} finally {setApplying(false);}
  };
  const control:React.CSSProperties={padding:'7px 10px',borderRadius:8,border:'1px solid var(--panel-border)',background:'var(--panel-surface)',color:'var(--panel-text)',fontFamily:'inherit'};
  return <div className="settings-field" style={{marginBottom:18}}>
    <label style={{display:'block',marginBottom:7}}>{label}</label>
    <div style={{display:'flex',gap:6}}>{['auto','off','on'].map(m=><button type="button" key={m} disabled={m!=='auto' && (!info?.known || (m==='off'&&!info.supportsDisable))} style={{...control,flex:1,color:mode===m?'var(--panel-accent)':undefined}} onClick={()=>onChange(m==='auto'?{mode:'Auto'}:{mode:m==='on'?'On':'Off'})}>{t(`config.reasoning_mode_${m}`)}</button>)}</div>
    {mode==='on' && info?.known && <div style={{display:'flex',gap:8,marginTop:8}}>
      <select aria-label={label} style={control} value={effort} onChange={e=>onChange({...value,mode:'On',effort:e.target.value||null})}>
        <option value="">{t('config.reasoning_effort_default')}</option>{info.efforts.map(e=><option key={e} value={e}>{t(`config.reasoning_effort_${e}`)}</option>)}
      </select>
      {info.budget && <input aria-label={zh?'思考 token 预算':'Thinking token budget'} type="number" placeholder={zh?'思考 token 预算':'Thinking token budget'} min={info.budget.min} max={info.budget.max} value={value?.budget_tokens??''} style={control} onChange={e=>onChange({...value,mode:'On',budget_tokens:e.target.value?Math.max(info.budget!.min,Math.min(info.budget!.max,Number(e.target.value))):null})}/>}
    </div>}
    <div style={{fontSize:12,color:'var(--panel-text-tertiary)',marginTop:6,lineHeight:1.6}}>
      {help}<br/>{zh?'自动：不发送思考控制参数，采用服务端默认。':'Auto sends no reasoning controls and uses the server default.'}
      {info&&!info.known && <div>{zh?'尚无此模型的能力资料，可通过下面的 JSON 手动配置，并联网核对预设。':'No capability metadata for this model. Configure JSON below or verify presets online.'}</div>}
      {mode==='off'&&info&&!info.supportsDisable&&<div style={{color:'#b45309'}}>{zh?'此模型未确认支持关闭；当前偏好将采用服务端默认，请改为自动。':'Disabling is not confirmed for this model. Select Auto to use its default.'}</div>}
      {mode==='on' && effort && info?.known && !info.efforts.includes(effort) && <div style={{color:'#b45309'}}>{zh?'原有强度不在支持列表中，请重新选择。':'The saved effort is unsupported; select a supported level.'}</div>}
      {info?.source && <div><a href={info.source} target="_blank" rel="noreferrer">{zh?'官方能力来源':'Capability source'}</a> · {info.verifiedAt}</div>}
    </div>
    <details style={{marginTop:8}}><summary>{zh?'高级：思考参数 JSON 覆盖':'Advanced: reasoning JSON override'}</summary>
      <p style={{fontSize:12}}>{zh?'覆盖优先于上面的偏好；null 删除对应字段。仅允许思考参数，不包含消息、密钥或工具。应用后再保存设置。':'Overrides take precedence; null removes a field. Only reasoning parameters are allowed. Apply, then save settings.'}</p>
      <textarea aria-label="Reasoning JSON" value={draft} onChange={e=>setDraft(e.target.value)} rows={5} style={{...control,width:'100%',boxSizing:'border-box',fontFamily:'monospace'}}/>
      <button type="button" disabled={applying} style={control} onClick={()=>void apply()}>{zh?'应用覆盖':'Apply override'}</button>
      <button type="button" style={{...control,marginLeft:8}} onClick={()=>{setDraft('');setDraftError('');onOverridesChange(null);}}>{zh?'清除覆盖':'Clear override'}</button>
      {draftError&&<div role="alert" style={{color:'#dc2626'}}>{draftError}</div>}
    </details>
    <details style={{marginTop:8}}><summary>{zh?'实际思考参数预览':'Reasoning parameter preview'}</summary><pre style={{whiteSpace:'pre-wrap',fontSize:12}}>{info?JSON.stringify(info.preview,null,2):'…'}</pre></details>
    {error&&<div role="alert" style={{color:'#dc2626'}}>{error}</div>}
  </div>;
}
