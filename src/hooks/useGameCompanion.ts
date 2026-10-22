import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { currentMonitor, getCurrentWindow, PhysicalPosition } from '@tauri-apps/api/window';
import { positioningCoordinator as coordinator } from './positioningCoordinator';
import { gamePosition } from '../chibi/gamePosition';
import { getCharacterId } from '../characterContext';

interface GameStatus { active: boolean; process: string; revision: number }

export function useGameCompanion(modelReady: boolean) {
  const [game, setGame] = useState<GameStatus>({ active: false, process: '', revision: -1 });
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | undefined;
    const apply = (next: GameStatus) => {
      if (!cancelled) setGame(previous => next.revision >= previous.revision ? next : previous);
    };
    void listen<GameStatus>('companion:game-mode', e => apply(e.payload)).then(fn => {
      if (cancelled) fn(); else off = fn;
      return invoke<{ game: GameStatus }>('companion_status');
    }).then(result => apply(result.game)).catch(() => {});
    return () => { cancelled = true; off?.(); };
  }, []);

  useEffect(() => {
    coordinator.gameMode = game.active;
    if (game.active) coordinator.abortSmartMove?.();
    return () => { coordinator.gameMode = false; };
  }, [game.active]);

  // Spectator mode must not steal clicks from the game, including in transparent window padding.
  // Change native styles only at mode transitions to avoid periodic WebView redraws.
  useEffect(() => {
    const win = getCurrentWindow();
    void win.setIgnoreCursorEvents(game.active).catch(() => {});
    return () => { void win.setIgnoreCursorEvents(false).catch(() => {}); };
  }, [game.active]);

  useEffect(() => {
    if (!game.active || !modelReady) return;
    let cancelled = false, pending = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let original: PhysicalPosition | undefined;
    let parked: { x: number; y: number } | undefined;
    let inFlight: Promise<void> = Promise.resolve();
    let movedAt = 0;
    const win = getCurrentWindow();
    const busy = () => cancelled || coordinator.dragInFlight || coordinator.fleeInFlight
      || coordinator.fullscreenInFlight || coordinator.fullscreenHidden || coordinator.ambientMoveInFlight
      || coordinator.physicsInFlight || coordinator.smartPositioningInFlight;
    const check = async () => {
      if (pending || busy()) return;
      pending = true;
      try {
        if (!await win.isVisible() || await win.isFocused()) return;
        const [position, size, monitor, cursor] = await Promise.all([
          win.outerPosition(), win.outerSize(), currentMonitor(), invoke<{ x: number; y: number }>('get_cursor_position'),
        ]);
        if (busy() || !monitor) return;
        original ??= position;
        const near = cursor.x > position.x - 140 && cursor.x < position.x + size.width + 140
          && cursor.y > position.y - 140 && cursor.y < position.y + size.height + 140;
        if (parked && (!near || Date.now() - movedAt < 3000)) return;
        const area = { x: monitor.position.x, y: monitor.position.y, width: monitor.size.width, height: monitor.size.height };
        const next = gamePosition(area, size, cursor, getCharacterId() === 'nana' ? 1 : 0);
        if (busy()) return;
        // Update before IPC so an exit during the request can still restore after it settles.
        parked = next;
        await win.setPosition(new PhysicalPosition(next.x, next.y));
        movedAt = Date.now();
      } catch { /* Window may have closed or gone offline. */ }
      finally { pending = false; }
    };
    const loop = async () => {
      inFlight = check();
      await inFlight;
      if (!cancelled) timer = setTimeout(() => void loop(), 500);
    };
    void loop();
    return () => {
      cancelled = true;
      clearTimeout(timer);
      // Restore only an automatically parked pet; preserve subsequent user positioning.
      void inFlight.then(async () => {
        if (!original || !parked) return;
        const position = await win.outerPosition();
        if (position.x === parked.x && position.y === parked.y && !coordinator.gameMode && !coordinator.dragInFlight
          && !coordinator.fullscreenInFlight && !coordinator.fullscreenHidden && !coordinator.smartPositioningInFlight)
          await win.setPosition(original);
      }).catch(() => {});
    };
  }, [game.active, modelReady]);
  return game;
}
