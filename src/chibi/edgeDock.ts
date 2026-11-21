import { frameDurationMs, type ChibiAnimationSpec, type ChibiDirection, type ChibiPlayback } from './motionRegistry';

export type DockSide = ChibiDirection | 'bottom';

export interface EdgeDockPlan {
  side: DockSide;
  walkX: number;
  walkY: number;
  dockX: number;
  dockY: number;
  left: number;
  right: number;
  top: number;
  bottom: number;
  width: number;
  height: number;
  scale: number;
}

// The authored peek puts the eyes/hands inside the screen and the body beyond
// the sprite's vertical centre line. Use physical monitor coordinates throughout.
export function planEdgeDock(positionX: number, width: number, left: number, right: number,
  scale: number, spriteLeft: number, spriteWidth: number, side?: ChibiDirection): EdgeDockPlan {
  const facing = side ?? (positionX + width / 2 < (left + right) / 2 ? 'left' : 'right');
  return planScreenDock({x:positionX,y:0}, {width,height:0},
    {left,right,top:-1e9,bottom:1e9,scale}, {left:spriteLeft,top:0,width:spriteWidth,height:0}, facing);
}

export function dockMotion(side: DockSide): 'edge' | 'edge-bottom' {
  return side === 'bottom' ? 'edge-bottom' : 'edge';
}

export function dockEntryPlayback(spec: ChibiAnimationSpec, side: DockSide): {
  approach: ChibiPlayback;
  settled: ChibiPlayback;
  slideDuration: number;
} {
  const frames = Array.from({length: spec.frames}, (_, index) => index);
  const durations = frames.map(index => frameDurationMs(spec, index));
  const firstDockedFrame = spec.dockedFrame;
  const split = firstDockedFrame !== undefined && Number.isInteger(firstDockedFrame)
    && firstDockedFrame > 0 && firstDockedFrame < frames.length ? firstDockedFrame : frames.length;
  const approach = {frames: frames.slice(0, split), durations: durations.slice(0, split)};
  const settled = {frames: frames.slice(split), durations: durations.slice(split)};
  // Marked grip frames and bottom poses already hold at the edge on their final frame.
  const slideFrames = settled.frames.length === 0 && spec.hold && frames.length > 1
    && (side === 'bottom' || spec.wallContact?.[side]) ? split - 1 : split;
  const slideDuration = durations.slice(0, slideFrames).reduce((time, duration) => time + duration, 0);
  return {approach, settled, slideDuration};
}

export function planScreenDock(position: {x:number;y:number}, size: {width:number;height:number},
  screen: {left:number;right:number;top:number;bottom:number;scale:number},
  sprite: {left:number;top:number;width:number;height:number}, side?: DockSide,
  sideGrip?: (side: ChibiDirection) => number | null): EdgeDockPlan {
  const {left,right,top,bottom,scale} = screen;
  const {width,height} = size;
  const candidates: Array<[DockSide,number]> = [
    ['left',Math.abs(position.x-left)], ['right',Math.abs(right-position.x-width)],
    ['bottom',Math.abs(bottom-position.y-height)],
  ];
  const facing = candidates.some(([candidate]) => candidate === side) ? side! : candidates.sort((a,b) => a[1]-b[1])[0][0];
  const horizontal = facing === 'left' || facing === 'right';
  const walkX = horizontal ? (facing === 'left' ? left : Math.max(left,right-width))
    : Math.max(left,Math.min(right-width,position.x));
  const walkY = horizontal ? Math.max(top,Math.min(bottom-height,position.y))
    : Math.max(top,bottom-height);
  const grip = horizontal ? (sideGrip?.(facing) ?? 50) / 100 : .5;
  return {side:facing,left,right,top,bottom,width,height,scale,walkX,walkY,
    dockX: horizontal ? Math.round((facing === 'left' ? left : right) - (sprite.left+sprite.width*grip)*scale) : walkX,
    dockY: horizontal ? walkY : Math.round(bottom - (sprite.top+sprite.height*grip)*scale),
  };
}

// Clip to the chosen monitor as well as moving offscreen: an adjacent monitor
// must never reveal the supposedly hidden body.
export function dockClip(x: number, plan: EdgeDockPlan, y = plan.walkY): string {
  const left = Math.max(0, (plan.left - x) / plan.scale);
  const right = Math.max(0, (x + plan.width - plan.right) / plan.scale);
  const top = Math.max(0,(plan.top-y)/plan.scale);
  const bottom = Math.max(0,(y+plan.height-plan.bottom)/plan.scale);
  return `inset(${top}px ${right}px ${bottom}px ${left}px)`;
}
