export interface PanelFloat { start(angle:number,bounds:{minX:number;maxX:number;minY:number;maxY:number}):void;stop():void;readonly position:{x:number;y:number}; }
export function createPanelFloat(write:(x:number,y:number)=>void):PanelFloat;
