import React,{useEffect,useState} from 'react';
import {stickerSource} from './stickers';
import type {StickerRef} from '../../types';
export default function StickerImage({sticker,size=144}:{sticker:StickerRef;size?:number}){
 const [src,setSrc]=useState<string>();const [failed,setFailed]=useState(false);
 useEffect(()=>{let active=true;setSrc(undefined);setFailed(false);stickerSource(sticker).then(s=>{if(active)setSrc(s);}).catch(()=>{if(active)setFailed(true);});return()=>{active=false;};},[sticker.id,sticker.version,sticker.character_id]);
 if(failed)return <span role="img" aria-label={sticker.meaning} title={sticker.label} style={{fontSize:Math.min(13,size/2),color:'inherit'}}>{size<40?'▧':`[${sticker.label}]`}</span>;
 return <div style={{width:size,height:size,display:'flex',alignItems:'center',justifyContent:'center'}}>{src?<img src={src} alt={sticker.label} title={sticker.meaning} width={size} height={size} loading="lazy" draggable={false} style={{objectFit:'contain',background:'transparent'}} onError={()=>setFailed(true)}/>:<span style={{opacity:.5,fontSize:12}}>…</span>}</div>;
}
