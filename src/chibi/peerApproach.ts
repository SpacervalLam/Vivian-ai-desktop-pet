import type { PetRect } from './ambientLane';
import { petFootprint } from './desktopPhysics';

export interface PeerWorkArea {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

/** Place the moving pet beside the listener's visible body, with both feet aligned. */
export function peerApproachPosition(source: PetRect, target: PetRect, peers: readonly PetRect[], area: PeerWorkArea) {
  const own = petFootprint(source.width, source.height);
  const other = petFootprint(target.width, target.height);
  const gap = 12 * area.scale;
  const minX = area.x;
  const maxX = area.x + area.width - source.width;
  const minY = area.y;
  const maxY = area.y + area.height - source.height;
  if (minX > maxX || minY > maxY) return null;

  const targetLeft = target.x + other.left;
  const baseline = target.y + other.top + other.height;
  const y = Math.round(Math.max(minY, Math.min(maxY, baseline - own.top - own.height)));
  const left = Math.floor(targetLeft - gap - own.width - own.left);
  const right = Math.ceil(targetLeft + other.width + gap - own.left);
  const choices = [left, right]
    .filter(x => x >= minX && x <= maxX)
    .filter(x => peers.every(peer => {
      if (peer.id === source.id || peer.id === target.id) return true;
      const body = petFootprint(peer.width, peer.height);
      const peerX = peer.x + body.left;
      const peerY = peer.y + body.top;
      return x + own.left + own.width <= peerX || peerX + body.width <= x + own.left
        || y + own.top + own.height <= peerY || peerY + body.height <= y + own.top;
    }))
    .sort((a, b) => Math.abs(a - source.x) - Math.abs(b - source.x));
  return choices.length ? { x: choices[0], y } : null;
}
