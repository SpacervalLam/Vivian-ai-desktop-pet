// Sprite registration helpers: measure opaque artwork, excluding low-alpha matte.
export function bounds(pixels, width, height) {
  let x0=width,y0=height,x1=-1,y1=-1;
  for(let y=0;y<height;y++)for(let x=0;x<width;x++)if(pixels[(y*width+x)*4+3]>200){
    x0=Math.min(x0,x);x1=Math.max(x1,x);y0=Math.min(y0,y);y1=Math.max(y1,y);
  }
  return {x0,y0,x1,y1,width:x1-x0+1,height:y1-y0+1};
}

export function faceBounds(pixels,width,height,box,previous) {
  const mask=new Uint8Array(width*height),components=[];
  // The central upper-body region excludes hands, boots and most prop artwork.
  const maxY=Math.min(height,Math.round(box.y0+box.height*.62));
  for(let y=box.y0;y<maxY;y++)for(let x=box.x0;x<=box.x1;x++){
    const k=(y*width+x)*4,r=pixels[k],g=pixels[k+1],b=pixels[k+2];
    if(pixels[k+3]>220&&r>190&&g>150&&b>130&&r>g+5&&g>b&&r-b<85)mask[y*width+x]=1;
  }
  // Thin antialiased bridges can connect cheek colour to the cream collar.
  // Separate those regions before selecting the facial component.
  const original=mask.slice();
  for(let y=1;y<height-1;y++)for(let x=1;x<width-1;x++)if(original[y*width+x]){
    for(let dy=-1;dy<=1;dy++)for(let dx=-1;dx<=1;dx++)if(!original[(y+dy)*width+x+dx])mask[y*width+x]=0;
  }
  for(let k=0;k<mask.length;k++)if(mask[k]){
    const queue=[k];mask[k]=0;let x0=width,y0=height,x1=0,y1=0;
    for(let p=0;p<queue.length;p++){
      const at=queue[p],x=at%width,y=Math.floor(at/width);
      x0=Math.min(x0,x);x1=Math.max(x1,x);y0=Math.min(y0,y);y1=Math.max(y1,y);
      for(const n of [x?at-1:-1,x<width-1?at+1:-1,at-width,at+width])if(n>=0&&n<mask.length&&mask[n]){mask[n]=0;queue.push(n);}
    }
    components.push({x0,x1,y0,y1,width:x1-x0+1,height:y1-y0+1,count:queue.length});
  }
  const score=c=>{
    if(!previous)return Math.log(c.count);
    const dx=((c.x0+c.x1-previous.x0-previous.x1)/2)/previous.width;
    const dy=((c.y0+c.y1-previous.y0-previous.y1)/2)/previous.height;
    return Math.log(c.count)-2*dx*dx-2*dy*dy;
  };
  const face=components.filter(c=>c.width>8&&c.count>=50).sort((a,b)=>score(b)-score(a))[0];
  if(!face||face.count<50)throw new Error('Could not measure face for registration');
  return {...face,x0:face.x0-1,x1:face.x1+1,y0:face.y0-1,y1:face.y1+1,width:face.width+2,height:face.height+2};
}

export function cleanMatte(pixels,width,height) {
  // Only retain soft edge pixels within two pixels of genuinely opaque artwork.
  // This removes detached tinted haze without raising opacity or recolouring art.
  const support=new Uint8Array(width*height);
  for(let y=0;y<height;y++)for(let x=0;x<width;x++)if(pixels[(y*width+x)*4+3]>=200){
    for(let dy=-2;dy<=2;dy++)for(let dx=-2;dx<=2;dx++){
      const xx=x+dx,yy=y+dy;if(xx>=0&&xx<width&&yy>=0&&yy<height)support[yy*width+xx]=1;
    }
  }
  for(let k=0;k<support.length;k++)if(!support[k]||pixels[k*4+3]<8)pixels.fill(0,k*4,k*4+4);
}

export function headCenter(pixels,width,height,face,box) {
  let left=width,right=0;
  const top=Math.max(box.y0,Math.floor(face.y1-face.width*1.8));
  for(let y=top;y<=face.y1;y++)for(let x=box.x0;x<=box.x1;x++)if(pixels[(y*width+x)*4+3]>200){left=Math.min(left,x);right=Math.max(right,x);}
  return (left+right)/2;
}
