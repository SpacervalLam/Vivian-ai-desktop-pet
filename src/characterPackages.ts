import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { registerCharacters } from './characters';
import type { ChibiAnimationSpec } from './chibi/motionRegistry';

type FrameTable = Record<string, number[][]>;
interface CharacterPlacement {
  scale: number;
  translateY: number;
}
export interface CharacterBubbleTheme {
  background: string;
  textColor: string;
  tailColor: string;
  shadow: string;
  crossBorderColor: string;
}
interface CharacterVisuals {
  accent?: string;
  bubbleTheme?: CharacterBubbleTheme;
  diaryArt?: string;
  notificationNotes?: [number, number];
  defaultSize?: CharacterPlacement;
  bottomDock?: CharacterPlacement;
}
export interface CharacterPackageManifest {
  pending_update?: boolean;
  atlas_path?: string;
  avatar_path?: string;
  independent_directions?: boolean;
  format: number;
  id: string;
  name: string;
  version: string;
  files: Record<string, string>;
  anchors: FrameTable;
  scales: FrameTable;
  animations?: Record<string, Omit<ChibiAnimationSpec, 'kind' | 'characterOverrides'>>;
  presentation?: CharacterVisuals & Record<string, unknown>;
}
interface LoadedPackage {
  manifest: CharacterPackageManifest;
  root: string;
}
export interface CharacterPackageStatus {
  active: LoadedPackage[];
  installed: CharacterPackageManifest[];
  errors?: string[];
  initialized?: string[];
  initializing?: boolean;
}

function indexPackages(packages: LoadedPackage[]) {
  const manifests = new Map<string, CharacterPackageManifest>();
  const assetRoots = new Map<string, string>();
  const anchors = new Map<string, number[][]>();
  const scales = new Map<string, number[][]>();
  // Insert in reverse so the first active package still owns each shared path.
  for (const { manifest, root } of [...packages].reverse()) {
    manifests.set(manifest.id, manifest);
    for (const path of Object.keys(manifest.files)) assetRoots.set(path, root);
    for (const [path, frames] of Object.entries(manifest.anchors ?? {})) anchors.set(path, frames);
    for (const [path, frames] of Object.entries(manifest.scales ?? {})) scales.set(path, frames);
  }
  return { manifests, assetRoots, anchors, scales };
}

let registry = indexPackages([]);
let requestedRevision = 0;
let appliedRevision = 0;

export async function initializeCharacterPackages(): Promise<void> {
  const revision = ++requestedRevision;
  const { active } = await invoke<CharacterPackageStatus>('character_package_status');
  if (revision < appliedRevision) return;
  registry = indexPackages(active);
  registerCharacters(active.map(pkg => pkg.manifest));
  appliedRevision = revision;
}

export function characterAnimation(id: string, name: string) {
  return registry.manifests.get(id)?.animations?.[name];
}

export function characterAssetSource(source: string): string | null {
  const path = `assets/${source.replace(/^\//, '')}`;
  const root = registry.assetRoots.get(path);
  return root === undefined ? null : convertFileSrc(`${root}/${path}`);
}
export function characterFrameData(path: string, kind: 'anchors' | 'scales') {
  return registry[kind].get(path);
}
export function characterPresentation(id: string, kind: 'defaultSize' | 'bottomDock') {
  return registry.manifests.get(id)?.presentation?.[kind];
}
export function characterVisuals(id: string): CharacterVisuals {
  return registry.manifests.get(id)?.presentation ?? {};
}
