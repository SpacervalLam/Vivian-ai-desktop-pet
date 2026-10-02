import React,{useEffect,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {useTranslation} from 'react-i18next';
import StickerImage from './StickerImage';
import type {StickerRef} from '../../types';
export default function StickerSettings(){
 const {i18n}=useTranslation();const zh=i18n.language.startsWith('zh');
 const [character,setCharacter]=useState('vivian');const [rows,setRows]=useState<StickerRef[]>([]);const [frequency,setFrequency]=useState('occasional');
 const [error,setError]=useState('');const [busy,setBusy]=useState(false);const [replace,setReplace]=useState('');const [label,setLabel]=useState('');const [meaning,setMeaning]=useState('');
 const reload=async()=>{const r=await invoke<{frequency:string;stickers:StickerRef[]}>('list_stickers',{characterId:character});setRows(r.stickers);setFrequency(r.frequency);};
 useEffect(()=>{let active=true;setError('');setRows([]);setReplace('');setLabel('');setMeaning('');invoke<{frequency:string;stickers:StickerRef[]}>('list_stickers',{characterId:character}).then(r=>{if(active){setRows(r.stickers);setFrequency(r.frequency);}}).catch(e=>{if(active)setError(String(e));});return()=>{active=false;};},[character]);
 const change=async(value:string)=>{setBusy(true);setError('');try{await invoke('set_sticker_frequency',{characterId:character,frequency:value});setFrequency(value);}catch(e){setError(String(e));}finally{setBusy(false);}};
 const importImage=async()=>{setBusy(true);setError('');try{const path=await open({multiple:false,filters:[{name:'PNG',extensions:['png']}]});if(!path||Array.isArray(path))return;await invoke('import_sticker',{characterId:character,sourcePath:path,label,meaning,replaceId:replace||null});await reload();setReplace('');setLabel('');setMeaning('');}catch(e){setError(String(e));}finally{setBusy(false);}};
 const style:React.CSSProperties={padding:'7px 10px',borderRadius:8,border:'1px solid var(--panel-border)',background:'var(--panel-surface)',color:'var(--panel-text)',fontFamily:'inherit'};
 return <details style={{margin:'18px 0',padding:12,border:'1px solid var(--panel-border)',borderRadius:12}}><summary>{zh?'聊天表情包':'Chat stickers'}</summary>
  <p style={{fontSize:12,color:'var(--panel-text-secondary)',lineHeight:1.6}}>{zh?'角色自主选择，应用控制频率；用于聊天窗口和桌面气泡。这里的设置即时生效。偶尔：间隔五轮；正常：间隔三轮。明确要求发送时可跳过冷却，关闭时始终不发。':'Characters choose stickers; the app limits frequency in ChatWindow and desktop bubbles. Changes apply immediately. Occasional: five replies apart; normal: three. Explicit requests bypass cooldown; Off always disables stickers.'}</p>
  <div style={{display:'flex',gap:8}}><select aria-label={zh?'角色':'Character'} value={character} disabled={busy} onChange={e=>setCharacter(e.target.value)} style={style}><option value="vivian">Vivian</option><option value="nana">Nana</option></select>
  <select aria-label={zh?'表情包频率':'Sticker frequency'} value={frequency} disabled={busy} onChange={e=>void change(e.target.value)} style={style}><option value="off">{zh?'关闭':'Off'}</option><option value="occasional">{zh?'偶尔':'Occasional'}</option><option value="normal">{zh?'正常':'Normal'}</option></select></div>
  <div style={{display:'grid',gridTemplateColumns:'repeat(auto-fill,minmax(85px,1fr))',gap:8,margin:'12px 0'}}>{rows.map(row=><div key={row.id} style={{display:'flex',alignItems:'center',flexDirection:'column',fontSize:11}}><StickerImage sticker={row} size={80}/><span>{row.label}</span></div>)}</div>
  <details><summary>{zh?'导入或替换 PNG 贴纸':'Import or replace a PNG sticker'}</summary><div style={{display:'grid',gap:8,marginTop:10}}>
  <select aria-label={zh?'替换目标':'Replacement target'} value={replace} style={style} disabled={busy} onChange={e=>{const row=rows.find(r=>r.id===e.target.value);setReplace(e.target.value);setLabel(row?.label??'');setMeaning(row?.meaning??'');}}><option value="">{zh?'新增贴纸':'Add sticker'}</option>{rows.map(row=><option key={row.id} value={row.id}>{zh?'替换':'Replace'} · {row.label}</option>)}</select>
  <input aria-label={zh?'贴纸名称':'Sticker label'} placeholder={zh?'名称（最多 24 字）':'Label (up to 24 characters)'} value={label} maxLength={24} onChange={e=>setLabel(e.target.value)} style={style}/>
  <textarea aria-label={zh?'贴纸含义':'Sticker meaning'} placeholder={zh?'含义和适用场景，供角色选择（最多 120 字）':'Meaning and suitable situations (up to 120 characters)'} value={meaning} maxLength={120} onChange={e=>setMeaning(e.target.value)} style={style}/>
  <button type="button" disabled={busy||!label.trim()||!meaning.trim()} onClick={()=>void importImage()} style={style}>{zh?'选择 PNG 并导入':'Select PNG and import'}</button><small>{zh?'最多 2 MB、2048×2048 像素；替换后，历史消息仍使用旧版本。':'Up to 2 MB and 2048×2048 pixels. Historical messages keep the previous version.'}</small>
  </div></details>{error&&<p role="alert" style={{color:'#dc2626'}}>{error}</p>}
 </details>;
}
