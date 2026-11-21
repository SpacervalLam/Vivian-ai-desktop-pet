/** Track each reply until its first visible text, independently of generation completion. */
export class ReplyWaiting {
  private streams = new Map<string, boolean>();
  start(id: string): void {
    if (id && !this.streams.has(id)) this.streams.set(id, true);
  }
  text(id: string, text: string): boolean {
    if (!text.trim()) return false;
    if (id) this.streams.set(id, false);
    return true;
  }
  finish(id: string): void { this.streams.delete(id); }
  /**
   * Drop every pending stream.
   *
   * Used by the watchdog: when the backend dies mid-request no terminal event ever
   * arrives, so `finish(id)` is never called and the pet would loop the thinking
   * pose forever. Clearing here lets the caller fall back to the resting pose.
   */
  clear(): void { this.streams.clear(); }
  get thinking(): boolean {
    const states = [...this.streams.values()];
    // An actively speaking reply takes precedence over another queued reply.
    return states.includes(true) && !states.includes(false);
  }
}
