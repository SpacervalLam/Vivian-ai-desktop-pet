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
  get thinking(): boolean {
    const states = [...this.streams.values()];
    // An actively speaking reply takes precedence over another queued reply.
    return states.includes(true) && !states.includes(false);
  }
}
