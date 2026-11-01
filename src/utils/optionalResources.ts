import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import manifest from '../optional-resources.json';

export interface PackDownload { filename: string; url: string; approx_mb: number }
export interface PackDefinition {
  id: string;
  label: string;
  version: string;
  files: string[];
  dev_root?: string;
  /** Present when the pack is published on its own instead of being bundled by Setup. */
  download?: PackDownload;
}
export type ResourcePackId = string;
export interface PackStatus { id: ResourcePackId; installed: boolean; root: string | null }
export interface PackInstallProgress {
  id: ResourcePackId;
  phase: 'downloading' | 'verifying' | 'done';
  downloaded: number;
  total: number | null;
}
export const optionalResourcePacks = manifest as PackDefinition[];
const development = import.meta.env?.DEV ?? true;
let status: PackStatus[] = [];

export function resourcePackInstalled(id: ResourcePackId): boolean {
  return status.some(pack => pack.id === id && pack.installed);
}
export function optionalResourceStatus(): PackStatus[] {
  return status;
}
/** Core URLs stay unchanged; optional URLs resolve to the installed pack only. */
export function optionalAssetSource(source: string): string | null {
  const path = source.replace(/^\//, '');
  const pack = optionalResourcePacks.find(pack => pack.files.includes(path));
  if (!pack) return source;
  const installed = status.find(row => row.id === pack.id && row.installed);
  if (!installed) return null;
  return development ? source : installed.root ? convertFileSrc(`${installed.root}/${path}`) : null;
}
/**
 * Optional packs are resolved from disk or the per-user install directory. Never
 * assume availability: a stale or partial pack must fall back to the core assets.
 */
export async function refreshOptionalResources(): Promise<void> {
  try {
    status = await invoke<PackStatus[]>('optional_resource_status');
  } catch {
    status = development
      ? optionalResourcePacks.filter(pack => !pack.download).map(pack => ({ id: pack.id, installed: true, root: null }))
      : [];
  }
}
export async function initializeOptionalResources(): Promise<void> {
  await refreshOptionalResources();
  const font = optionalAssetSource('/fonts/ma-shan-zheng.woff2');
  if (font) {
    // Both the room theme and comic lettering use the installed resource pack.
    // Finish loading before mounting the room so its entrance uses this font.
    await Promise.all(['Ma Shan Zheng', 'Mind Comic'].map(async family => {
      try {
        const face = await new FontFace(family, `url("${font}")`, { weight: '400', style: 'normal', display: 'swap' }).load();
        document.fonts.add(face);
      } catch { /* System font fallback when the optional pack is unavailable. */ }
    }));
  }
}
/** Download a published pack, or install a ZIP chosen by the user. */
export async function installOptionalResource(id: ResourcePackId, source?: string): Promise<boolean> {
  await invoke('install_optional_resource', source ? { id, source } : { id });
  await refreshOptionalResources();
  return resourcePackInstalled(id);
}
