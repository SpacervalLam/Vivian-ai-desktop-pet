// Exercise the real chroma-key cutout in isolation (no app deps).
// The full crate's unit-test binary OOMs on this 16GB box, so compile just this module.
import { mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const root = process.cwd();
const dir = path.join(root, 'tmp', 'image-alpha-tests');
await mkdir(path.join(dir, 'src'), { recursive: true });
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-image-alpha-tests"
version="0.1.0"
edition="2021"
[dependencies]
png="0.18"
[workspace]
`);
const module = path.join(root, 'src-tauri/src/network/image_alpha.rs').replaceAll('\\', '/');
await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
#[path="${module}"] pub mod image_alpha;
#[cfg(test)] mod integration {
    use super::image_alpha::*;
    fn png_of(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, w, h);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(rgba).unwrap();
        }
        out
    }
    #[test] fn flat_key_background_becomes_alpha_and_subject_stays_opaque() {
        let (w, h) = (8u32, 8u32);
        let mut rgba = Vec::new();
        for y in 0..h { for x in 0..w {
            if (3..5).contains(&x) && (3..5).contains(&y) { rgba.extend_from_slice(&[220, 30, 30, 255]); }
            else { rgba.extend_from_slice(&[0, 255, 0, 255]); }
        }}
        let (out, key) = cutout_png(&png_of(w, h, &rgba), None).unwrap();
        assert_eq!(key, (0, 255, 0));
        assert!(has_alpha(&out));
        let (dec, _, _) = decode_rgba_png(&out).unwrap();
        assert_eq!(dec[3], 0, "左上角必须被抠掉");
        assert_eq!(dec[(3 * 8 + 3) * 4 + 3], 255, "主体必须不透明");
    }
    #[test]
    fn mismatched_corners_are_refused_rather_than_guessed() {
        let (w, h) = (4u32, 4u32);
        let mut rgba = Vec::new();
        for i in 0..(w * h) {
            if i % w < w / 2 { rgba.extend_from_slice(&[255, 0, 0, 255]); }
            else { rgba.extend_from_slice(&[0, 0, 255, 255]); }
        }
        let err = cutout_png(&png_of(w, h, &rgba), None).unwrap_err();
        assert!(err.contains("四角"), "应拒绝猜测而不是乱抠：{err}");
    }
}
`);
const run = spawnSync('cargo', ['test', '--offline'], { cwd: dir, encoding: 'utf8', timeout: 280000 });
if (run.status !== 0) {
  console.error(run.stdout || '');
  console.error(run.stderr || '');
  throw new Error('image-alpha harness failed');
}
console.log('image-alpha harness: 2 passed');
