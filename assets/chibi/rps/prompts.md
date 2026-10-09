# RPS sprite generation

Generated with the built-in `image_gen` tool on 2026-10-08, with `transparent_background: true`. Full-resolution source PNGs are preserved alongside this file. Runtime outputs are uniformly scaled and padded to 512×512; all three gestures use shared bounds per character and align to that character's clipboard sprite foot baseline. No chroma-key extraction was needed.

## Vivian rock

Use case: identity-preserve. Asset type: single desktop companion game sprite, Vivian rock hand. Edit the reference sprite: preserve exact character identity, blonde hair, blue eyes, halo, ears, wing ornaments, pink-white dress, chibi proportions, line art, shading, full body scale and foot baseline. Remove the paper completely. Raise one hand at chest height in an unmistakable closed FIST (rock in rock-paper-scissors), thumb folded across four curled fingers, facing viewer and clearly visible. Other hand relaxed. Cheerful competitive small smile. Exactly ONE character ONE pose, centered, full body including halo and both feet, generous transparent margin, square canvas. Real transparent alpha background, no green screen, no labels, no text, no props, no extra fingers. Retain composition and scale of the 512x512 reference as closely as possible.

Reference: `public/chibi/motion/vivian/vivian-clipboard-sheet.webp`.

## Nana rock

Use case: identity-preserve. Asset type: single transparent desktop game sprite, Nana rock. Edit reference: preserve exact silver lavender hair, purple pink eyes, fox ears, fluffy tail, lavender floral dress, flower hair ornaments, full body chibi proportions, line art and colors. Remove paper entirely. Raise one hand at chest height in a clearly readable closed FIST (rock for rock-paper-scissors), thumb folded across the four curled fingers; palm-side fist facing viewer. Other arm relaxed at side. Cheerful little competitive smile. ONE character, ONE pose, full body including ears and both feet, centered square transparent alpha canvas, generous padding. Preserve original character size and foot baseline as closely as possible. No halo, no wings, no text, no green screen, no additional objects, no extra fingers.

Reference: `public/chibi/motion/nana/nana-clipboard-sheet.webp`.

## Vivian scissors / paper

Use case: identity-preserve. Asset type: transparent single game sprite, Vivian {hand}. Change ONLY the raised hand in the reference from a fist to {gesture}. This is rock-paper-scissors, hand at same chest height with readable anatomically correct gesture. Preserve exact face, cheerful smile, hair, halo, wings, clothing, body, free hand, pose, pixel placement, full-body framing, feet baseline, line art and colors. ONE character ONE pose, square transparent alpha canvas. No text, no props, no green background. Match the reference character size and placement exactly.

Reference: generated `vivian-rock-source.png`.

## Nana scissors / paper

Use case: identity-preserve. Asset type: transparent single game sprite, Nana {hand}. Change ONLY the raised hand in the reference from a fist to {gesture}. This is rock-paper-scissors, hand at same chest height with readable anatomically correct gesture. Preserve exact face, cheerful smile, silver lavender hair, purple pink eyes, fox ears, fluffy tail, purple floral clothing, body, free hand, pose, pixel placement, full-body framing, feet baseline, line art and colors. ONE character ONE pose, square transparent alpha canvas. No halo, no text, no props, no green background. Match the reference character size and placement exactly.

Reference: generated `nana-rock-source.png`.

Each template was expanded for two separate calls:

- `hand = scissors`, `gesture = SCISSORS: exactly index and middle fingers extended in a clear V, ring and pinky curled into palm and held by thumb`
- `hand = paper`, `gesture = PAPER: open palm facing viewer with all FIVE fingers visible and separated, thumb clearly visible`
