/** Standalone UI entry built into plugins/3d-apartment/ui/room.js. */
import { StrictMode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { configureApartmentAssets } from './apartmentAssets';
import RoomWindow from './RoomWindow';

declare global {
  interface Window {
    VivianApartment?: {
      mount(container: HTMLElement, assetRoot: string): void;
      unmount(): void;
    };
  }
}

let root: Root | null = null;
window.VivianApartment = {
  mount(container, assetRoot) {
    configureApartmentAssets(assetRoot);
    root?.unmount();
    root = createRoot(container);
    root.render(<StrictMode><RoomWindow /></StrictMode>);
  },
  unmount() {
    root?.unmount();
    root = null;
  },
};
