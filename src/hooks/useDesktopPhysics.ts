import { useEffect, useRef, useState, type RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { DesktopPhysics, petFootprint, petBody, ceilingDizzyMs, type TerrainWorld } from '../chibi/desktopPhysics';
import { positioningCoordinator as coordinator } from './positioningCoordinator';

/** A single serialized movement loop; user input and hiding always take precedence. */
export function useDesktopPhysics(enabled: boolean, layer: RefObject<HTMLDivElement>) {
  const [airborne, setAirborne] = useState(false);
  const [error, setError] = useState(false);
  const releaseRef = useRef<{ vx: number; vy: number; session_id?: string } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false, held = false, paused = true, frameAt = performance.now(), sampledAt = 0;
    let engine: DesktopPhysics | null = null;
    let offset = { left: 0, top: 0 };
    let timer: number | undefined;
    const unlisteners: UnlistenFn[] = [];
    let animation: Animation | undefined;
    let lastAirborne = false;
    let dragSessionId: string | undefined;
    let lastPosition: { x: number; y: number } | null = null;
    const win = getCurrentWindow();
    coordinator.physicsEnabled = true;
    coordinator.abortSmartMove?.();
    const busy = () => cancelled || held || coordinator.gameMode || coordinator.dragInFlight || coordinator.fullscreenHidden ||
      coordinator.fullscreenInFlight || coordinator.fleeInFlight || coordinator.smartPositioningInFlight ||
      coordinator.peerConversationInProgress;
    const begin = () => { held = true; dragSessionId = undefined; coordinator.physicsInFlight = false; };
    const end = () => { held = false; };
    window.addEventListener('mousedown', begin, true);
    window.addEventListener('mouseup', end, true);
    window.addEventListener('blur', end);
    // Release momentum belongs only to this pet window, never to the other character.
    const watch = (name: string, handler: (payload: unknown) => void) => {
      void win.listen(name, event => handler(event.payload))
        .then(fn => { if (cancelled) fn(); else unlisteners.push(fn); })
        .catch(() => {});
    };
    watch('drag:released', payload => {
      const release = payload as { vx: number; vy: number; session_id?: string };
      releaseRef.current = release;
      dragSessionId = release.session_id;
      paused = true;
      held = false;
    });
    // A mouseup that lands outside the transparent WebView never reaches the DOM
    // listener, leaving `held` true forever and silently freezing gravity. Both
    // the native drag watchdog and an edge-dock hand-off are authoritative ends.
    watch('drag:cancelled', () => { held = false; });
    watch('dock:changed', () => { held = false; });
    const tick = async () => {
      try {
        if (busy() || !await win.isVisible()) {
          if (coordinator.peerConversationInProgress && lastAirborne) {
            lastAirborne = false;
            coordinator.physicsInFlight = false;
            setAirborne(false);
            layer.current?.style.setProperty('--physics-lean', '0deg');
          }
          paused = true; lastPosition = null; frameAt = performance.now(); return;
        }
        const now = performance.now();
        if (!engine || paused || now - sampledAt >= 100) {
          const [position, size, scale, raw] = await Promise.all([
            win.outerPosition(), win.outerSize(), win.scaleFactor(), invoke<TerrainWorld>('get_desktop_terrain'),
          ]);
          if (busy()) { paused = true; return; }
          // 其它桌宠窗口作为实体参与碰撞；本窗口自身（label 相同）与浮层窗口不参与。
          const world: TerrainWorld = {
            floors: raw.floors,
            windows: [
              ...raw.windows.filter(rect => !rect.pet),
              ...raw.windows.filter(rect => rect.pet && rect.pet !== win.label).map(petBody),
            ],
          };
          if (!world.floors.length) throw new Error('No desktop work area');
          if (coordinator.ambientMoveInFlight && engine) {
            const support = engine.support;
            const old = [...engine.world.windows, ...engine.world.floors].find(rect => rect.id === support);
            const next = [...world.windows, ...world.floors].find(rect => rect.id === support);
            if (!old || !next || old.x !== next.x || old.y !== next.y || old.width !== next.width || old.height !== next.height || !engine.hasVisibleSupport(world, position.x + offset.left)) {
              coordinator.physicsInFlight = true;
            }
            // Cancel walking before taking over a changed platform; never issue competing writes.
            paused = true; frameAt = performance.now(); return;
          }
          const footprint = petFootprint(size.width, size.height);
          offset = footprint;
          const body = { id: win.label, x: position.x + offset.left, y: position.y + offset.top,
            width: footprint.width, height: footprint.height };
          if (!engine || paused || engine.scale !== scale || engine.body.width !== body.width || engine.body.height !== body.height || engine.windowTopOffset !== offset.top) {
            const priorEnclosure = engine?.enclosure;
            engine = new DesktopPhysics(body, scale, offset.top);
            engine.updateWorld(world, .1);
            const release = releaseRef.current;
            if (release) engine.release(release.vx, release.vy);
            else if (priorEnclosure && world.windows.some(rect => rect.id === priorEnclosure && rect.landable !== false && rect.width > body.width + 8 * scale && rect.height > body.height + 8 * scale)) engine.enclosure = priorEnclosure;
            releaseRef.current = null;
            paused = false;
          } else {
            // Grounded frontend walking is allowed to change x, but never receives a second writer.
            engine.body.x = body.x;
            engine.updateWorld(world, (now - sampledAt) / 1000);
            if (coordinator.ambientMoveInFlight) {
              if (Math.abs(engine.body.y - body.y) > 3 * scale || !engine.lane) coordinator.physicsInFlight = true;
            }
          }
          sampledAt = performance.now();
          setError(false);
        }
        if (!engine) return;
        if (coordinator.ambientMoveInFlight) { frameAt = performance.now(); paused = true; return; }
        const result = engine.step((performance.now() - frameAt) / 1000);
        frameAt = performance.now();
        coordinator.physicsInFlight = result.airborne;
        const lane = engine.lane;
        coordinator.physicsLane = lane ? { minX: lane.minX - offset.left, maxX: lane.maxX - offset.left } : null;
        if (result.airborne !== lastAirborne) { lastAirborne = result.airborne; setAirborne(result.airborne); }
        if (busy()) { paused = true; return; }
        const dizzyMs = ceilingDizzyMs(result.ceilingImpact, engine.scale);
        if (dizzyMs !== null) {
          void win.emitTo(win.label, 'drag:dizzy', {
            duration_ms: dizzyMs, reason: 'edge_bounce', impact: result.ceilingImpact / (1000 * engine.scale),
            session_id: dragSessionId,
          }).catch(() => {});
        }
        const position = { x: Math.round(engine.body.x - offset.left), y: Math.round(engine.body.y - offset.top) };
        if (!lastPosition || position.x !== lastPosition.x || position.y !== lastPosition.y) {
          await invoke('set_window_position', position);
          if (cancelled) return;
          lastPosition = position;
        }
        layer.current?.style.setProperty('--physics-lean', `${result.airborne ? Math.max(-12, Math.min(12, engine.vx / (100 * engine.scale))) : 0}deg`);
        if (result.impact > 180 * engine.scale && layer.current && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
          animation?.cancel();
          animation = layer.current.animate([
            { transform: 'scale(1, 1)' }, { transform: 'scale(1.1, .86)', offset: .2 },
            { transform: 'scale(.96, 1.04)', offset: .6 }, { transform: 'scale(1, 1)' },
          ], { duration: 360, easing: 'ease-out' });
        }
      } catch {
        if (cancelled) return;
        paused = true;
        coordinator.physicsInFlight = false;
        coordinator.physicsLane = null;
        setError(true);
      } finally {
        if (!cancelled) timer = window.setTimeout(() => void tick(), paused ? 150 : 16);
      }
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      unlisteners.forEach(fn => fn()); animation?.cancel(); releaseRef.current = null;
      layer.current?.style.removeProperty('--physics-lean');
      window.removeEventListener('mousedown', begin, true);
      window.removeEventListener('mouseup', end, true);
      window.removeEventListener('blur', end);
      coordinator.physicsEnabled = false; coordinator.physicsInFlight = false; coordinator.physicsLane = null;
      setAirborne(false); setError(false);
    };
  }, [enabled, layer]);
  return { airborne, error };
}
