import React from 'react';

interface Props {
  pageKey: string;
  depth: number;
  children: React.ReactNode;
}

interface PageSnapshot {
  element: HTMLDivElement;
  scroll: Array<{ element: Element; top: number; left: number }>;
}

/** Keep one live React tree; the departing page is a short-lived, inert DOM snapshot. */
export default class ChatPageTransition extends React.Component<Props> {
  private content = React.createRef<HTMLDivElement>();
  private stage = React.createRef<HTMLDivElement>();
  private animations: Animation[] = [];
  private departing: HTMLDivElement | null = null;

  getSnapshotBeforeUpdate(previous: Props): PageSnapshot | null {
    const content = this.content.current;
    if (previous.pageKey === this.props.pageKey || !content ||
      window.matchMedia('(prefers-reduced-motion: reduce)').matches || !content.animate) return null;

    // Capture before React replaces the old page, including virtualized list offsets
    // and unsaved form values. Never mount a second set of listeners or effects.
    const element = content.cloneNode(true) as HTMLDivElement;
    element.setAttribute('aria-hidden', 'true');
    element.setAttribute('inert', '');
    const originals = [content, ...content.querySelectorAll('*')];
    const copies = [element, ...element.querySelectorAll('*')];
    const scroll: PageSnapshot['scroll'] = [];
    originals.forEach((original, index) => {
      const copy = copies[index];
      copy.removeAttribute('id');
      copy.removeAttribute('autofocus');
      if (original.scrollTop || original.scrollLeft) {
        scroll.push({ element: copy, top: original.scrollTop, left: original.scrollLeft });
      }
      if (original instanceof HTMLInputElement && copy instanceof HTMLInputElement) {
        copy.value = original.value;
        copy.checked = original.checked;
      } else if (original instanceof HTMLTextAreaElement && copy instanceof HTMLTextAreaElement) {
        copy.value = original.value;
      } else if (original instanceof HTMLSelectElement && copy instanceof HTMLSelectElement) {
        copy.selectedIndex = original.selectedIndex;
      } else if (original instanceof HTMLCanvasElement && copy instanceof HTMLCanvasElement) {
        try { copy.getContext('2d')?.drawImage(original, 0, 0); } catch { /* A closed canvas may have no frame. */ }
      }
    });
    // The header menu should not travel across the screen after choosing a tool.
    element.querySelectorAll('[role="menu"]').forEach(menu => menu.remove());
    return { element, scroll };
  }

  componentDidUpdate(previous: Props, _state: unknown, snapshot: PageSnapshot | null) {
    if (previous.pageKey === this.props.pageKey) return;
    this.clearTransition();
    const content = this.content.current;
    if (!snapshot || !content || !this.stage.current) return;

    const backwards = this.props.depth < previous.depth;
    const departing = snapshot.element;
    departing.classList.add('chat-page-departing');
    departing.style.zIndex = backwards ? '2' : '1';
    content.style.zIndex = backwards ? '1' : '2';
    this.departing = departing;
    this.stage.current.appendChild(departing);
    snapshot.scroll.forEach(({ element, top, left }) => {
      element.scrollTop = top;
      element.scrollLeft = left;
    });

    const options: KeyframeAnimationOptions = {
      duration: 380, easing: 'cubic-bezier(0.32, 0.72, 0, 1)', fill: 'both',
    };
    this.animations = [
      content.animate([
        { transform: backwards ? 'translateX(-28%)' : 'translateX(100%)', filter: backwards ? 'brightness(0.82)' : 'brightness(1)' },
        { transform: 'translateX(0)', filter: 'brightness(1)' },
      ], options),
      departing.animate([
        { transform: 'translateX(0)', filter: 'brightness(1)' },
        { transform: backwards ? 'translateX(100%)' : 'translateX(-28%)', filter: backwards ? 'brightness(1)' : 'brightness(0.82)' },
      ], options),
    ];
    // Old promises can settle after a newer navigation has started.
    void Promise.all(this.animations.map(animation => animation.finished)).then(() => {
      if (this.departing === departing) this.clearTransition();
    }).catch(() => { /* Interrupted navigation or unmount. */ });
  }

  private clearTransition = () => {
    this.animations.forEach(animation => animation.cancel());
    this.animations = [];
    this.departing?.remove();
    this.departing = null;
    if (this.content.current) this.content.current.style.zIndex = '';
  };

  componentWillUnmount() { this.clearTransition(); }

  render() {
    return <div className="chat-page-stage" ref={this.stage}>
      <div className="chat-page-content" ref={this.content}>{this.props.children}</div>
    </div>;
  }
}
