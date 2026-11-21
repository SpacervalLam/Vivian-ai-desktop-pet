/** Standalone UI entry built into plugins/3d-apartment/ui/room.js. */
import bootHtml from './boot.html?raw';
import { bootMark } from './roomBoot';
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
    document.getElementById('apartment-loading')?.remove();
    document.body.insertAdjacentHTML('beforeend', bootHtml);
    const loader = document.getElementById('boot-loader');
    if (loader) loader.dataset.t = String(performance.now());
    window.setTimeout(() => loader?.remove(), 20000);
    bootMark('plugin:mount');
    root?.unmount();
    root = createRoot(container);
    root.render(<RoomWindow />);
  },
  unmount() {
    root?.unmount();
    root = null;
    document.getElementById('boot-loader')?.remove();
  },
};
