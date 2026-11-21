import { useEffect, type RefObject } from 'react';

const controlSelector = [
  '.phone-lock-action', '.phone-glass-button', '.phone-app-back',
  'button.phone-liquid-glass', 'label.phone-liquid-glass',
  '.phone-liquid-glass button', '.chat-app-navigation button',
  '.chat-app-menu button', '.chat-composer button',
].join(',');
const surfaceSelector = '.phone-liquid-glass,.chat-app-navigation,.chat-app-menu,.chat-composer';

/** Contact light and gel-like flex, independent of layout/entrance transforms. */
export function useLiquidGlassInteractions(rootRef: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
    const animations = new Map<HTMLElement, Animation>();
    let active: { control: HTMLElement; motion: HTMLElement; surfaces: HTMLElement[];
      pointer: number | null; key: string | null; x: number; y: number } | null = null;

    function animate(node: HTMLElement, frames: Keyframe[], duration: number) {
      animations.get(node)?.cancel();
      animations.delete(node);
      if (reducedMotion.matches || !node.isConnected) return;
      const animation = node.animate(frames, { duration, fill: 'forwards', easing: 'cubic-bezier(.2,.8,.2,1)' });
      animations.set(node, animation);
      animation.onfinish = () => {
        if (active?.motion !== node && animations.get(node) === animation) {
          animation.cancel();
          animations.delete(node);
        }
      };
    }

    function position(surface: HTMLElement, x: number, y: number) {
      const bounds = surface.getBoundingClientRect();
      surface.style.setProperty('--glass-contact-x', `${Math.max(0, Math.min(bounds.width, x - bounds.left))}px`);
      surface.style.setProperty('--glass-contact-y', `${Math.max(0, Math.min(bounds.height, y - bounds.top))}px`);
    }

    function release(cancelled = false) {
      if (!active) return;
      const { motion, surfaces } = active;
      const style = getComputedStyle(motion);
      const start = { scale: style.scale, translate: style.translate };
      active = null;
      for (const surface of surfaces) delete surface.dataset.glassPressed;
      animate(motion, cancelled ? [start, { scale: '1', translate: '0px' }] : [
        start, { scale: '1.018 1.012', translate: '0px', offset: .55 },
        { scale: '.998 1', translate: '0px', offset: .8 }, { scale: '1', translate: '0px' },
      ], cancelled ? 180 : 380);
    }

    function press(target: EventTarget | null, pointer: number | null, key: string | null, x?: number, y?: number) {
      if (!(target instanceof Element)) return;
      const control = target.closest<HTMLElement>(controlSelector);
      if (!control || !root!.contains(control) || control.matches(':disabled,[aria-disabled=true]')) return;
      release(true);
      const motion = control.querySelector<HTMLElement>('.phone-app-art') ?? control;
      const surface = control.closest<HTMLElement>(surfaceSelector) ?? control;
      // A child control illuminates its shared navigation material too.
      const surfaces = [...new Set([surface, ...(motion === control ? [control] : [])])];
      const bounds = motion.getBoundingClientRect();
      const contactX = x ?? bounds.left + bounds.width / 2;
      const contactY = y ?? bounds.top + bounds.height / 2;
      const style = getComputedStyle(motion);
      const start = { scale: style.scale, translate: style.translate };
      active = { control, motion, surfaces, pointer, key, x: contactX, y: contactY };
      control.dataset.glassControl = '';
      motion.dataset.glassMotion = '';
      for (const element of surfaces) {
        // Preserve absolute/fixed positioning used by floating menus and gallery bars.
        if (getComputedStyle(element).position === 'static') element.style.position = 'relative';
        element.dataset.glassSurface = '';
        position(element, contactX, contactY);
        element.dataset.glassPressed = '';
      }
      const wide = bounds.width > 120;
      const dx = ((contactX - bounds.left) / Math.max(1, bounds.width) - .5) * 2;
      const dy = ((contactY - bounds.top) / Math.max(1, bounds.height) - .5) * 2;
      animate(motion, [start, { scale: wide ? '.992 .978' : '.97 .945', translate: `${dx}px ${dy}px` }], 100);
    }

    const pointerDown = (event: PointerEvent) => {
      if (event.button === 0 && event.isPrimary) press(event.target, event.pointerId, null, event.clientX, event.clientY);
    };
    const pointerMove = (event: PointerEvent) => {
      if (!active || active.pointer !== event.pointerId) return;
      const bounds = active.control.getBoundingClientRect();
      const outside = event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom;
      if (outside || (event.pointerType !== 'mouse' && Math.hypot(event.clientX - active.x, event.clientY - active.y) > 10)) {
        release(true);
        return;
      }
      for (const surface of active.surfaces) position(surface, event.clientX, event.clientY);
    };
    const pointerUp = (event: PointerEvent) => { if (active?.pointer === event.pointerId) release(); };
    const pointerCancel = (event: PointerEvent) => { if (active?.pointer === event.pointerId) release(true); };
    const keyDown = (event: KeyboardEvent) => {
      if (event.repeat || !['Enter', ' '].includes(event.key) || (event.target instanceof Element && event.target.matches('input,textarea,[contenteditable]'))) return;
      press(event.target, null, event.key);
    };
    const keyUp = (event: KeyboardEvent) => { if (active?.key === event.key) release(); };
    const cancel = () => release(true);
    const focusOut = (event: FocusEvent) => {
      if (event.target instanceof Node && active?.control.contains(event.target)) cancel();
    };
    const changeMotion = () => { cancel(); for (const animation of animations.values()) animation.cancel(); animations.clear(); };
    // Capture observes controls that stop propagation, without intercepting their actions.
    root.addEventListener('pointerdown', pointerDown, true);
    root.addEventListener('keydown', keyDown, true);
    root.addEventListener('focusout', focusOut);
    root.addEventListener('pointerleave', cancel);
    root.addEventListener('wheel', cancel, { passive: true });
    window.addEventListener('pointermove', pointerMove, true);
    window.addEventListener('pointerup', pointerUp, true);
    window.addEventListener('pointercancel', pointerCancel, true);
    window.addEventListener('keyup', keyUp, true);
    window.addEventListener('blur', cancel);
    reducedMotion.addEventListener('change', changeMotion);
    return () => {
      cancel();
      for (const animation of animations.values()) animation.cancel();
      root.removeEventListener('pointerdown', pointerDown, true);
      root.removeEventListener('keydown', keyDown, true);
      root.removeEventListener('focusout', focusOut);
      root.removeEventListener('pointerleave', cancel);
      root.removeEventListener('wheel', cancel);
      window.removeEventListener('pointermove', pointerMove, true);
      window.removeEventListener('pointerup', pointerUp, true);
      window.removeEventListener('pointercancel', pointerCancel, true);
      window.removeEventListener('keyup', keyUp, true);
      window.removeEventListener('blur', cancel);
      reducedMotion.removeEventListener('change', changeMotion);
    };
  }, [rootRef]);
}
