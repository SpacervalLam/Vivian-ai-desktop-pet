import { useEffect, useRef, useState, type RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { DesktopPhysics, petFootprint, type TerrainWorld } from '../chibi/desktopPhysics';
import { positioningCoordinator as coordinator } from './positioningCoordinator';

/** A single serialized movement loop; user input and hiding always take precedence. */
export function useDesktopPhysics(enabled: boolean, layer: RefObject<HTMLDivElement>) {
  const [airborne, setAirborne] = useState(false);
  const [error, setError] = useState(false);
  const releaseRef = useRef<{ vx: number; vy: number } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false, held = false, paused = true, frameAt = performance.now(), sampledAt = 0;
    let engine: DesktopPhysics | null = null;
    let offset = { left: 0, top: 0 };
    let timer: number | undefined, unlisten: UnlistenFn | undefined;
    let animation: Animation | undefined;
    let lastAirborne = false;
    let lastPosition: { x: number; y: number } | null = null;
    const win = getCurrentWindow();
    coordinator.physicsEnabled = true;
    coordinator.abortSmartMove?.();
    const busy = () => cancelled || held || coordinator.dragInFlight || coordinator.fullscreenHidden ||
      coordinator.fullscreenInFlight || coordinator.fleeInFlight || coordinator.smartPositioningInFlight;
    const begin = () => { held = true; coordinator.physicsInFlight = false; };
    const end = () => { held = false; };
    window.addEventListener('mousedown', begin, true);
    window.addEventListener('mouseup', end, true);
    window.addEventListener('blur', end);
    void listen<{ vx: number; vy: number }>('drag:released', event => {
      releaseRef.current = event.payload;
      paused = true;
      held = false;
    }).then(fn => { if (cancelled) fn(); else unlisten = fn; }).catch(() => {});
    const tick = async () => {
      try {
        if (busy() || !await win.isVisible()) {
          paused = true; lastPosition = null; frameAt = performance.now(); return;
        }
        const now = performance.now();
        if (!engine || paused || now - sampledAt >= 100) {
          const [position, size, scale, world] = await Promise.all([
            win.outerPosition(), win.outerSize(), win.scaleFactor(), invoke<TerrainWorld>('get_desktop_terrain'),
          ]);
          if (busy()) { paused = true; return; }
          if (!world.floors.length) throw new Error('No desktop work area');
          if (coordinator.ambientMoveInFlight && engine) {
            const support = engine.support;
            const old = [...engine.world.windows, ...engine.world.floors].find(rect => rect.id === support);
            const next = [...world.windows, ...world.floors].find(rect => rect.id === support);
            if (!old || !next || old.x !== next.x || old.y !== next.y || old.width !== next.width || old.height !== next.height) {
              coordinator.physicsInFlight = true;
            }
            // Cancel walking before taking over a changed platform; never issue competing writes.
            paused = true; frameAt = performance.now(); return;
          }
          const footprint = petFootprint(size.width, size.height);
          offset = footprint;
          const body = { id: win.label, x: position.x + offset.left, y: position.y + offset.top,
            width: footprint.width, height: footprint.height };
          if (!engine || paused || engine.scale !== scale || engine.body.width !== body.width || engine.body.height !== body.height) {
            engine = new DesktopPhysics(body, scale);
            engine.updateWorld(world, .1);
            const release = releaseRef.current;
            engine.release(release?.vx ?? 0, release?.vy ?? 0);
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
      unlisten?.(); animation?.cancel(); releaseRef.current = null;
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
