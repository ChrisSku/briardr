// Builds Briardr's app icons from scripts/icon.png, a briard's head on a transparent background,
// placed on a full-bleed dark square: Apple rounds the corners itself. The maskable icon (Android crops
// it to a circle or another shape) shrinks the dog into the 80% safe circle and keeps the background
// full size. briard.png is the dog alone, for the app's brand tile. Also copies the website's icons into site/ and writes the README's image, docs/briardr.png.
// Each PNG then goes through pngquant if it's installed (brew or apt install pngquant): at quality 85–100
// the icons look the same at about a third of the size.
// Usage: bun scripts/icons.ts [outdir]   (renders with the Playwright Chromium the UI tests use)
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { chromium } from 'playwright';

const root = path.resolve(import.meta.dir, '..');
const outArg = process.argv[2];
const out = path.resolve(outArg || path.join(root, 'web', 'public', 'icons'));
const dog = `data:image/png;base64,${fs.readFileSync(path.join(root, 'scripts', 'icon.png')).toString('base64')}`;

// `fill`: the share of the icon's height the dog's opaque area takes up.
const SIZES = [
  { file: 'favicon-16.png', size: 16, fill: 0.9 },
  { file: 'favicon-32.png', size: 32, fill: 0.88 },
  { file: 'apple-touch-icon.png', size: 180, fill: 0.8 },
  { file: 'icon-192.png', size: 192, fill: 0.8 },
  { file: 'icon-512.png', size: 512, fill: 0.8 },
  { file: 'icon-maskable-512.png', size: 512, fill: 0.64 }, // the ears stay inside the safe circle
];

/** The dog alone on transparency, for the app's brand tile (sign-in, the sidebar): 68 pt at 2x. */
const MARK = { file: 'briard.png', size: 136 };
/** The website's copies (site/), written only when building into web/public/icons. */
const SITE = ['favicon-32.png', 'apple-touch-icon.png', 'icon-192.png'];
/** The README's image, also only then: the dog alone on transparency, at twice the 112 px it's shown at. */
const README = { file: path.join(root, 'docs', 'briardr.png'), size: 224 };

fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch();
const page = await browser.newPage();
await page.setContent(`<body style="margin:0"><div id="icon"><img id="dog"></div></body>`);
// The opaque area of the artwork, so the dog is centred and sized by what's visible.
const box = await page.evaluate(async (src) => {
  const img = document.getElementById('dog') as HTMLImageElement;
  img.src = src;
  await img.decode();
  const canvas = new OffscreenCanvas(img.naturalWidth, img.naturalHeight);
  const ctx = canvas.getContext('2d')!;
  ctx.drawImage(img, 0, 0);
  const { data, width, height } = ctx.getImageData(0, 0, img.naturalWidth, img.naturalHeight);
  let x0 = width, y0 = height, x1 = 0, y1 = 0;
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++)
      if (data[(y * width + x) * 4 + 3] > 8) {
        x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y);
      }
  return { x0, y0, w: x1 - x0 + 1, h: y1 - y0 + 1, natural: img.naturalWidth };
}, dog);

/** Renders the dog at `size` px, its opaque area `fill` of the height; `bare` leaves out the background. */
async function render(file: string, size: number, fill: number, bare = false) {
  await page.setViewportSize({ width: size, height: size });
  await page.evaluate(({ size, fill, box, bare }) => {
    Object.assign(document.getElementById('icon')!.style, {
      position: 'relative', overflow: 'hidden', width: `${size}px`, height: `${size}px`,
      background: bare ? 'none' : `radial-gradient(circle ${0.72 * size}px at 50% 40%, #253641, #18222b)`,
    });
    const scale = (fill * size) / box.h;
    Object.assign(document.getElementById('dog')!.style, {
      position: 'absolute', width: `${box.natural * scale}px`,
      left: `${size / 2 - (box.x0 + box.w / 2) * scale}px`, top: `${size / 2 - (box.y0 + box.h / 2) * scale}px`,
    });
  }, { size, fill, box, bare });
  await page.locator('#icon').screenshot({ path: file, omitBackground: bare });
  shrink(file);
}

let pngquant = true;
/** Compresses a PNG in place with pngquant; leaves it as it is when that's missing or wouldn't help. */
function shrink(file: string) {
  if (!pngquant) return;
  const r = spawnSync('pngquant', ['--quality=85-100', '--speed=1', '--strip', '--skip-if-larger', '--force', '--output', file, file]);
  if (r.error) {
    pngquant = false;
    console.warn('pngquant isn’t installed: the icons are left uncompressed (brew or apt install pngquant)');
  }
}

for (const { file, size, fill } of SIZES) await render(path.join(out, file), size, fill);
const whole = (0.96 * box.h) / Math.max(box.w, box.h); // the whole dog fits, ears included
await render(path.join(out, MARK.file), MARK.size, whole, true);
if (!outArg) await render(README.file, README.size, whole, true);
await browser.close();
if (!outArg) for (const file of SITE) fs.copyFileSync(path.join(out, file), path.join(root, 'site', file));
console.log(`icons written to ${out}${outArg ? '' : `, site/ and ${path.relative(root, README.file)}`}`);
