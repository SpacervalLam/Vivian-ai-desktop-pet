export function createPanelFloat(write) {
 let x=0,y=0,frame=0;
 const stop=()=>{cancelAnimationFrame(frame);frame=0;};
 const start=(angle,bounds)=>{
  stop();const dx=Math.cos(angle),dy=Math.sin(angle);
  const ray=(value,direction,min,max)=>Math.abs(direction)<1e-8?Infinity:(direction>0?max-value:min-value)/direction;
  const distance=Math.max(0,Math.min(ray(x,dx,bounds.minX,bounds.maxX),ray(y,dy,bounds.minY,bounds.maxY)));
  const fromX=x,fromY=y,started=performance.now();
  const tick=now=>{const travel=Math.min(distance,(now-started)*.0012);x=Math.max(bounds.minX,Math.min(bounds.maxX,fromX+dx*travel));y=Math.max(bounds.minY,Math.min(bounds.maxY,fromY+dy*travel));write(x,y);if(travel<distance)frame=requestAnimationFrame(tick);else frame=0;};
  frame=requestAnimationFrame(tick);
 };
 return {start,stop,get position(){return {x,y}}};
}
