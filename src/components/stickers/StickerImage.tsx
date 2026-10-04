import React,{useEffect,useState} from 'react';
import {stickerSource,stickerLabel} from './stickers';
import {useTranslation} from 'react-i18next';
import type {StickerRef} from '../../types';
export default function StickerImage({sticker,size=144}:{sticker:StickerRef;size?:number}){
 useTranslation();
 const label=stickerLabel(sticker);
 const [src,setSrc]=useState<string>();const [failed,setFailed]=useState(false);
 useEffect(()=>{let active=true;setSrc(undefined);setFailed(false);stickerSource(sticker).then(s=>{if(active)setSrc(s);}).catch(()=>{if(active)setFailed(true);});return()=>{active=false;};},[sticker.id,sticker.version,sticker.character_id]);
 if(failed)return <span role="img" aria-label={label} title={label} style={{fontSize:Math.min(13,size/2),color:'inherit'}}>{size<40?'▧':`[${label}]`}</span>;
 return <div style={{width:size,height:size,display:'flex',alignItems:'center',justifyContent:'center'}}>{src?<img src={src} alt={label} title={label} width={size} height={size} loading="lazy" draggable={false} style={{objectFit:'contain',background:'transparent'}} onError={()=>setFailed(true)}/>:<span style={{opacity:.5,fontSize:12}}>…</span>}</div>;
}
