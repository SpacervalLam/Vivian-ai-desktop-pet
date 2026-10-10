export interface PetRect {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Restrict optional wandering to free space on the current side of a visible peer. */
export function ambientLane(
  pet: PetRect,
  peers: readonly PetRect[],
  minX: number,
  maxX: number,
  gap: number,
): { minX: number; maxX: number } {
  const center = pet.x + pet.width / 2;
  for (const peer of peers) {
    if (peer.id === pet.id || pet.y + pet.height <= peer.y || peer.y + peer.height <= pet.y) continue;
    const peerCenter = peer.x + peer.width / 2;
    const onLeft = peerCenter < center || (peerCenter === center && peer.id < pet.id);
    // Already overlapping pets can separate; do not move them further into one another.
    if (onLeft) minX = Math.max(minX, Math.min(pet.x, peer.x + peer.width + gap));
    else maxX = Math.min(maxX, Math.max(pet.x, peer.x - pet.width - gap));
  }
  return { minX, maxX };
}
