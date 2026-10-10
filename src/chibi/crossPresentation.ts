export interface CrossPresentationEvent {
  stream_id?: string;
  speaker_id?: string;
  listener_id?: string;
  text?: string;
  expression?: string;
  motion?: string;
}

export type CrossMotion = 'talk' | 'listen';

/** Presentation ownership follows stream identity, separately from physical actions. */
export class CrossPresentation {
  private streams = new Map<string, { motion: CrossMotion; peer: string }>();
  private closed = new Set<string>();

  constructor(private readonly characterId: string) {}

  update(event: CrossPresentationEvent, chunk = false): boolean {
    const id = event.stream_id;
    if (!id || this.closed.has(id) || (chunk && !event.text?.trim())) return false;
    const speaker = event.speaker_id?.toLowerCase();
    const listener = event.listener_id?.toLowerCase();
    if (speaker !== this.characterId && listener !== this.characterId) return false;
    const motion = speaker === this.characterId ? 'talk' : 'listen';
    const peer = speaker === this.characterId ? listener : speaker;
    if (!peer) return false;
    // A chunk can start presentation when its start event was missed during loading.
    this.streams.delete(id);
    this.streams.set(id, { motion, peer });
    return true;
  }

  finish(id?: string): boolean {
    if (!id) return false;
    const changed = this.streams.delete(id);
    this.close(id);
    return changed;
  }

  removePeer(peer: string): void {
    for (const [id, stream] of this.streams) {
      if (stream.peer === peer.toLowerCase()) this.finish(id);
    }
  }

  clear(): void {
    for (const id of this.streams.keys()) this.close(id);
    this.streams.clear();
  }

  private close(id: string): void {
    this.closed.add(id);
    // Keep late chunks from reviving interrupted speech without retaining all history.
    if (this.closed.size > 128) this.closed.delete(this.closed.values().next().value!);
  }

  get motion(): CrossMotion | null {
    const streams = [...this.streams.values()];
    return streams[streams.length - 1]?.motion ?? null;
  }
}
