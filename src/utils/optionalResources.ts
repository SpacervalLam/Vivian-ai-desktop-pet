import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import packs from '../optional-resources.json';

export type ResourcePackId = 'fonts' | 'stickers';
interface PackStatus { id: ResourcePackId; installed: boolean; root: string | null }
const development = import.meta.env?.DEV ?? true;
let status: PackStatus[] = development
  ? packs.map(pack => ({ id: pack.id as ResourcePackId, installed: true, root: null })) : [];

export function resourcePackInstalled(id: ResourcePackId): boolean {
  return status.some(pack => pack.id === id && pack.installed);
}
/** Core URLs stay unchanged; optional URLs resolve to the installed pack only. */
export function optionalAssetSource(source: string): string | null {
  const path = source.replace(/^\//, '');
  const pack = packs.find(pack => pack.files.includes(path));
  if (!pack) return source;
  const installed = status.find(row => row.id === pack.id && row.installed);
  if (!installed) return null;
  return development ? source : installed.root ? convertFileSrc(`${installed.root}/${path}`) : null;
}
export async function initializeOptionalResources(): Promise<void> {
  if (!development) {
    try { status = await invoke<PackStatus[]>('optional_resource_status'); }
    catch { status = []; }
  }
  const font = optionalAssetSource('/fonts/ma-shan-zheng.woff2');
  if (font) {
    void new FontFace('Ma Shan Zheng', `url("${font}")`, { weight: '400', style: 'normal', display: 'swap' })
      .load().then(face => document.fonts.add(face)).catch(() => { /* System font fallback. */ });
  }
}
