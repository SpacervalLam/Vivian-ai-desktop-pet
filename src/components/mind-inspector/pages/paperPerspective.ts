import type { CSSProperties } from 'react';

// Writable corners of diary-paper-v35 in its displayed SVG image. The portrait
// art is center-cropped to the SVG's square viewBox before the panel scales it.
// Keep this calibration tied to that image rather than the outer comic frame.
export const diaryWritingCorners = [
  [220 / 1122, (290 - 140) / 1122],
  [835 / 1122, (282 - 140) / 1122],
  [909 / 1122, (1148 - 140) / 1122],
  [218 / 1122, (1160 - 140) / 1122],
] as const;

const writingCorners = {
  diary: diaryWritingCorners,
  planner: [
    [242 / 1122, (273 - 140) / 1122], [895 / 1122, (276 - 140) / 1122],
    [942 / 1122, (1170 - 140) / 1122], [222 / 1122, (1173 - 140) / 1122],
  ],
  notebook: [
    [260 / 1122, (290 - 140) / 1122], [919 / 1122, (293 - 140) / 1122],
    [934 / 1122, (1127 - 140) / 1122], [262 / 1122, (1135 - 140) / 1122],
  ],
} as const;

/** Cover the frame with the 1120×1400 notebook artwork without distortion.
 * SVG viewBox coordinates need compensating for the outer nonuniform scale. */
export function notebookPaperLayout(width: number, height: number) {
  if (width <= 0 || height <= 0) return { x: 0, y: 0, width: 100, height: 100 };
  const imageWidth = Math.max(width, height * .8);
  const imageHeight = imageWidth / .8;
  return { x: (width - imageWidth) / width * 50, y: 0, width: imageWidth / width * 100, height: imageHeight / height * 100 };
}

/** Project a live rectangular writing surface onto the illustrated paper. */
export function paperWritingStyle(source: keyof typeof writingCorners, width: number, height: number): CSSProperties | undefined {
  if (width <= 0 || height <= 0) return;
  const image = source === 'notebook' ? notebookPaperLayout(width, height) : undefined;
  const [p0, p1, p2, p3] = writingCorners[source].map(([x, y]) => image
    ? [(image.x + x * image.width) / 100 * width, (y * 1122 + 140) / 1402 * image.height / 100 * height]
    : [x * width, y * height]);
  const dx1 = p1[0] - p2[0], dx2 = p3[0] - p2[0];
  const dy1 = p1[1] - p2[1], dy2 = p3[1] - p2[1];
  const dx3 = p0[0] - p1[0] + p2[0] - p3[0];
  const dy3 = p0[1] - p1[1] + p2[1] - p3[1];
  const det = dx1 * dy2 - dx2 * dy1;
  const g = (dx3 * dy2 - dx2 * dy3) / det;
  const h = (dx1 * dy3 - dx3 * dy1) / det;
  const logicalWidth = image ? image.width / 100 * width * .61 : width * .61;
  const logicalHeight = image ? image.height / 100 * height * .77 * 1122 / 1402 : height * .77;
  const matrix = [
    (p1[0] - p0[0] + g * p1[0]) / logicalWidth,
    (p1[1] - p0[1] + g * p1[1]) / logicalWidth, 0, g / logicalWidth,
    (p3[0] - p0[0] + h * p3[0]) / logicalHeight,
    (p3[1] - p0[1] + h * p3[1]) / logicalHeight, 0, h / logicalHeight,
    0, 0, 1, 0, p0[0], p0[1], 0, 1,
  ];
  return { width: logicalWidth, height: logicalHeight, transform: `matrix3d(${matrix.join(',')})` };
}
