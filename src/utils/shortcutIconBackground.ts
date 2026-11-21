/** Opaque artwork extends its dominant edge; cutouts get the most distinct RGB backdrop. */
export function shortcutIconBackground(pixels: Uint8ClampedArray, width: number, height: number): string {
  const buckets = new Map<string, { weight: number; r: number; g: number; b: number }>();
  let transparentEdge = false;
  let r = 0, g = 0, b = 0, total = 0;
  const band = Math.max(1, Math.round(Math.min(width, height) / 12));
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    const i = (y * width + x) * 4, alpha = pixels[i + 3] / 255;
    r += pixels[i] * alpha; g += pixels[i + 1] * alpha; b += pixels[i + 2] * alpha; total += alpha;
    const horizontal = x < band || x >= width - band, vertical = y < band || y >= height - band;
    if (!horizontal && !vertical) continue;
    if (alpha < .98) transparentEdge = true;
    const weight = (horizontal && vertical ? 8 : 1) * alpha;
    const key = [pixels[i], pixels[i + 1], pixels[i + 2]].map(v => Math.round(v / 24)).join(',');
    const bucket = buckets.get(key) ?? { weight: 0, r: 0, g: 0, b: 0 };
    bucket.weight += weight; bucket.r += pixels[i] * weight; bucket.g += pixels[i + 1] * weight; bucket.b += pixels[i + 2] * weight;
    buckets.set(key, bucket);
  }
  if (!total) return '#f2f2f7';
  if (transparentEdge) {
    // Squared RGB distance from all visible pixels is maximal at a cube vertex.
    return `rgb(${[r, g, b].map(value => value / total < 127.5 ? 255 : 0).join(', ')})`;
  }
  const winner = [...buckets.values()].sort((a, b) => b.weight - a.weight)[0];
  return winner?.weight ? `rgb(${[winner.r, winner.g, winner.b].map(value => Math.round(value / winner.weight)).join(', ')})` : '#f2f2f7';
}
