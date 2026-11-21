export interface PetRect {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * 可走区间 + 两端是不是**真正的墙**。
 *
 * `walls` 必须和区间一起返回：区间会被同伴窗口截断，而截断处不是可以倚靠的东西。
 * 只把数字交出去，调用方（`planRoam`）就分不出「屏幕边」和「隔壁桌宠的窗口边」，
 * 会走到同伴面前对着空气侧身倚靠。
 */
export interface AmbientLane {
  minX: number;
  maxX: number;
  walls: { left: boolean; right: boolean };
}

/** 1px 容差：区间端点是整数坐标，同伴夹取后再四舍五入会差一点点。 */
const WALL_TOLERANCE_PX = 0.5;

/** Restrict optional wandering to free space on the current side of a visible peer. */
export function ambientLane(
  pet: PetRect,
  peers: readonly PetRect[],
  minX: number,
  maxX: number,
  gap: number,
): AmbientLane {
  // Window rectangles include transparent margins. Use the same visible-body
  // geometry as physics, then convert the result back to window coordinates.
  const offset = petFootprint(pet.width, pet.height);
  const body = { ...pet, x: pet.x + offset.left, y: pet.y + offset.top, width: offset.width, height: offset.height };
  minX += offset.left;
  maxX += offset.left;
  // Endpoints the pet may reach without any peer in the way: only these are walls.
  const reachableMinX = minX;
  const reachableMaxX = maxX;
  const center = body.x + body.width / 2;
  for (const peer of peers) {
    const footprint = petFootprint(peer.width, peer.height);
    const other = { ...peer, x: peer.x + footprint.left, y: peer.y + footprint.top, width: footprint.width, height: footprint.height };
    if (other.id === pet.id || body.y + body.height <= other.y || other.y + other.height <= body.y) continue;
    const peerCenter = other.x + other.width / 2;
    const onLeft = peerCenter < center || (peerCenter === center && other.id < pet.id);
    // Already overlapping pets can separate; do not move them further into one another.
    if (onLeft) minX = Math.max(minX, Math.min(body.x, other.x + other.width + gap));
    else maxX = Math.min(maxX, Math.max(body.x, other.x - body.width - gap));
  }
  return {
    minX: minX - offset.left,
    maxX: maxX - offset.left,
    walls: {
      left: minX <= reachableMinX + WALL_TOLERANCE_PX,
      right: maxX >= reachableMaxX - WALL_TOLERANCE_PX,
    },
  };
}
import { petFootprint } from './desktopPhysics';
