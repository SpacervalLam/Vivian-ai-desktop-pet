import { convertFileSrc } from '@tauri-apps/api/core';

let installedAssetRoot: string | null = null;

/** The bundled apartment uses its own model directory in installed builds. */
export function configureApartmentAssets(root: string | null): void {
  installedAssetRoot = root;
}

export function apartmentAssetUrl(path: string): string {
  const relative = path.replace(/^\/?room\//, '').replace(/^\//, '');
  if (!installedAssetRoot) return `/plugins/3d-apartment/room/${relative}`;
  return convertFileSrc(`${installedAssetRoot}\\${relative.replace(/\//g, '\\')}`);
}
