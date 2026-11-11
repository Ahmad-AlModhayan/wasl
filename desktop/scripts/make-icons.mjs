// Draws Wasl's app icon and tray icons as PNG (and the Windows .ico) with no dependencies.
//
//   node scripts/make-icons.mjs
//
// The mark is the Wasl mark from @kahf/design (lib/brand.ts) (24×24 viewBox, round strokes plus two
// arrow heads); colours are the Kahf family palette from @kahf/design (styles/kahf.css). Output goes to
// src-tauri/icons/ (app icon) and src-tauri/icons/tray/ (mac/ and win/{light,dark}/, one PNG per tray badge, FR-B5,
// #69). Re-run after changing it.

import { deflateSync } from 'node:zlib';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const iconsDir = join(root, 'src-tauri', 'icons');
const trayDir = join(iconsDir, 'tray');

// Kahf palette for the app icon (light theme values; the tile carries its own background).
const NAVY = [0x1f, 0x3d, 0x6b]; // --kf-product-kahf (Wasl's accent, .kf-scope-wasl)
const WHITE = [0xff, 0xff, 0xff];

// ------------------------------------------------------------------ geometry

function segDist(px, py, ax, ay, bx, by) {
  const dx = bx - ax, dy = by - ay;
  const l2 = dx * dx + dy * dy;
  let t = l2 === 0 ? 0 : ((px - ax) * dx + (py - ay) * dy) / l2;
  t = Math.max(0, Math.min(1, t));
  const x = ax + t * dx - px, y = ay + t * dy - py;
  return Math.hypot(x, y);
}

function inPolygon(px, py, pts) {
  let inside = false;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    const [xi, yi] = pts[i], [xj, yj] = pts[j];
    if ((yi > py) !== (yj > py) && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/** Points along an SVG circular arc (rx = ry, no rotation), endpoint parametrisation. */
function arcPoints(x1, y1, r, large, sweep, x2, y2) {
  const mx = (x1 - x2) / 2, my = (y1 - y2) / 2;
  const d2 = mx * mx + my * my;
  if (r * r < d2) r = Math.sqrt(d2);
  let k = Math.sqrt(Math.max(0, (r * r - d2) / d2));
  if (large === sweep) k = -k;
  const cxp = k * my, cyp = -k * mx;
  const cx = cxp + (x1 + x2) / 2, cy = cyp + (y1 + y2) / 2;
  const a1 = Math.atan2(y1 - cy, x1 - cx);
  let da = Math.atan2(y2 - cy, x2 - cx) - a1;
  if (sweep && da < 0) da += 2 * Math.PI;
  if (!sweep && da > 0) da -= 2 * Math.PI;
  const n = 24;
  const out = [];
  for (let i = 1; i <= n; i++) {
    const a = a1 + (da * i) / n;
    out.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
  }
  return out;
}

/** Parses the small path subset the mark uses: M, L, A, Z with absolute coordinates. Returns subpaths. */
function parsePath(d) {
  const tokens = d.match(/[MLAZ]|-?\d*\.?\d+/g);
  const subpaths = [];
  let cur = null, i = 0, cmd = null;
  const num = () => parseFloat(tokens[i++]);
  while (i < tokens.length) {
    if (/[MLAZ]/.test(tokens[i])) cmd = tokens[i++];
    if (cmd === 'M') { cur = { pts: [[num(), num()]], closed: false }; subpaths.push(cur); cmd = 'L'; }
    else if (cmd === 'L') cur.pts.push([num(), num()]);
    else if (cmd === 'A') {
      const r = num(); num(); num(); const large = num(); const sweep = num(); const x = num(), y = num();
      const [x1, y1] = cur.pts[cur.pts.length - 1];
      cur.pts.push(...arcPoints(x1, y1, r, !!large, !!sweep, x, y));
    } else if (cmd === 'Z') { cur.closed = true; cmd = null; }
  }
  return subpaths;
}

// The Wasl mark (brand.ts → BRAND.wasl), viewBox 0 0 24 24.
const MARK_STROKES = [
  'M4.40 17.80 A1.7 1.7 0 0 1 2.70 16.10 L2.70 7.70 A1.7 1.7 0 0 1 4.40 6.00 L7.05 6.00',
  'M13.60 6.00 A4.6 4.6 0 0 1 18.20 10.69 A3.6 3.6 0 0 1 17.40 17.80 L10.95 17.80',
].flatMap(parsePath);
const MARK_FILLS = parsePath('M11.05 7.25 L6.45 2.60 L6.45 7.25 Z M6.95 16.55 L11.55 21.20 L11.55 16.55 Z');
const MARK_STROKE = 2.5;
const MARK_FILL_STROKE = 0.9;

/** Is (x, y) in mark space (0..24) covered by the mark? */
function inMark(x, y) {
  for (const sp of MARK_STROKES) {
    for (let k = 1; k < sp.pts.length; k++) {
      const [ax, ay] = sp.pts[k - 1], [bx, by] = sp.pts[k];
      if (segDist(x, y, ax, ay, bx, by) <= MARK_STROKE / 2) return true;
    }
  }
  for (const sp of MARK_FILLS) {
    if (inPolygon(x, y, sp.pts)) return true;
    for (let k = 0; k < sp.pts.length; k++) {
      const [ax, ay] = sp.pts[k], [bx, by] = sp.pts[(k + 1) % sp.pts.length];
      if (segDist(x, y, ax, ay, bx, by) <= MARK_FILL_STROKE / 2) return true;
    }
  }
  return false;
}

function inRoundRect(x, y, x0, y0, x1, y1, r) {
  const cx = Math.max(x0 + r, Math.min(x1 - r, x));
  const cy = Math.max(y0 + r, Math.min(y1 - r, y));
  return Math.hypot(x - cx, y - cy) <= r && x >= x0 && x <= x1 && y >= y0 && y <= y1;
}

// ------------------------------------------------------------------ raster

/** Renders `size`×`size` RGBA by supersampling `paint(x, y) → [r,g,b,a] | null` (x, y in 0..1). */
function render(size, paint) {
  const ss = 4;
  const out = Buffer.alloc(size * size * 4);
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const c = paint((px + (sx + 0.5) / ss) / size, (py + (sy + 0.5) / ss) / size);
          if (c) { const ca = c[3] ?? 1; r += c[0] * ca; g += c[1] * ca; b += c[2] * ca; a += ca; }
        }
      }
      const n = ss * ss, o = (py * size + px) * 4;
      out[o] = a ? Math.round(r / a) : 0;
      out[o + 1] = a ? Math.round(g / a) : 0;
      out[o + 2] = a ? Math.round(b / a) : 0;
      out[o + 3] = Math.round((a / n) * 255);
    }
  }
  return out;
}

const CRC = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});
function crc32(buf) {
  let c = -1;
  for (const byte of buf) c = CRC[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
function png(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0)),
  ]);
}
/** A Windows .ico holding PNG images (Vista and later read these). */
function ico(images) {
  const head = Buffer.alloc(6 + 16 * images.length);
  head.writeUInt16LE(0, 0); head.writeUInt16LE(1, 2); head.writeUInt16LE(images.length, 4);
  let offset = head.length;
  images.forEach(({ size, data }, i) => {
    const e = 6 + 16 * i;
    head[e] = size >= 256 ? 0 : size; head[e + 1] = size >= 256 ? 0 : size;
    head.writeUInt16LE(1, e + 4); head.writeUInt16LE(32, e + 6);
    head.writeUInt32LE(data.length, e + 8); head.writeUInt32LE(offset, e + 12);
    offset += data.length;
  });
  return Buffer.concat([head, ...images.map((i) => i.data)]);
}

/** A macOS .icns holding PNG images, keyed by the OSType for each pixel size. */
function icns(images) {
  const entries = images.map(({ type, data }) => {
    const head = Buffer.alloc(8);
    head.write(type, 0, 'ascii'); head.writeUInt32BE(data.length + 8, 4);
    return Buffer.concat([head, data]);
  });
  const head = Buffer.alloc(8);
  head.write('icns', 0, 'ascii'); head.writeUInt32BE(8 + entries.reduce((n, e) => n + e.length, 0), 4);
  return Buffer.concat([head, ...entries]);
}

// ------------------------------------------------------------------ the icons

/** The app icon: the white mark on a navy rounded square. */
function appIcon(x, y) {
  if (!inRoundRect(x, y, 0.04, 0.04, 0.96, 0.96, 0.22)) return null;
  const m = 0.16; // margin around the mark
  return inMark(((x - m) / (1 - 2 * m)) * 24, ((y - m) / (1 - 2 * m)) * 24) ? WHITE : NAVY;
}

/** The macOS app icon: the same tile on Apple's icon grid (824 of 1024 px, centred), so it sits with the
 *  other icons in the Dock and Finder. */
function macAppIcon(x, y) {
  const inset = 100 / 1024;
  if (!inRoundRect(x, y, inset, inset, 1 - inset, 1 - inset, 0.18)) return null;
  const m = inset + 0.13;
  return inMark(((x - m) / (1 - 2 * m)) * 24, ((y - m) / (1 - 2 * m)) * 24) ? WHITE : NAVY;
}

/**
 * Tray icons (#69, design D6): the bare mark, no tile, plus a badge in the bottom-right corner for the states that
 * need a look. The badge's shape says the state, so it reads without colour; the base is never red, yellow or green.
 *
 * - macOS: template images (black and alpha only). macOS recolours them for light and dark menu bars; the badge is a
 *   disc with its glyph cut out.
 * - Windows: one set for a light taskbar (dark mark) and one for a dark taskbar (white mark), chosen from the
 *   taskbar's own theme (SystemUsesLightTheme), not the app's. The badge is a coloured disc with its glyph on top.
 *   Linux uses the dark-taskbar set.
 */
const BADGES = ['base', 'warning', 'error', 'paused', 'offline'];
const BLACK = [0, 0, 0];

// Badge glyphs, in badge-centred coordinates (badge radius 0.24).
const bang = (x, y) => (Math.abs(x) < 0.035 && y > -0.14 && y < 0.04) || Math.hypot(x, y - 0.11) < 0.04;
const cross = (x, y) => Math.max(Math.abs(x), Math.abs(y)) < 0.12 && (Math.abs(x - y) < 0.05 || Math.abs(x + y) < 0.05);
const bars = (x, y) => Math.abs(y) < 0.11 && ((x > -0.1 && x < -0.03) || (x > 0.03 && x < 0.1));
const dash = (x, y) => Math.abs(y) < 0.035 && Math.abs(x) < 0.12;
const GLYPHS = { warning: bang, error: cross, paused: bars, offline: dash };

/** The mark, filling the icon, with a clear ring cut where a badge sits. */
function trayMark(badge, markColour, disc, glyphColour) {
  return (x, y) => {
    const bx = 0.77, by = 0.77, br = 0.23;
    const db = Math.hypot(x - bx, y - by);
    if (badge !== 'base') {
      if (db <= br) {
        const cut = GLYPHS[badge](x - bx, y - by);
        if (cut) return glyphColour; // null for a template: the glyph is a hole
        return disc;
      }
      if (db <= br + 0.07) return null; // the clear ring
    }
    const m = 0.03;
    return inMark(((x - m) / (1 - 2 * m)) * 24, ((y - m) / (1 - 2 * m)) * 24) ? markColour : null;
  };
}

// Windows 11's own status colours per taskbar theme (Fluent "caution", "critical", neutral).
const WIN = {
  light: { mark: [0x1c, 0x1c, 0x1c], glyph: WHITE, warning: [0x9d, 0x5d, 0x00], error: [0xc4, 0x2b, 0x1c], paused: [0x5c, 0x5c, 0x5c], offline: [0x5c, 0x5c, 0x5c] },
  dark: { mark: WHITE, glyph: [0x1c, 0x1c, 0x1c], warning: [0xfc, 0xe1, 0x00], error: [0xff, 0x99, 0xa4], paused: [0xc8, 0xc8, 0xc8], offline: [0xc8, 0xc8, 0xc8] },
};

rmSync(trayDir, { recursive: true, force: true });
mkdirSync(join(trayDir, 'mac'), { recursive: true });
for (const badge of BADGES) {
  // 18 pt in the menu bar at 2x.
  writeFileSync(join(trayDir, 'mac', `${badge}.png`), png(36, render(36, trayMark(badge, BLACK, BLACK, null))));
}
for (const [theme, c] of Object.entries(WIN)) {
  mkdirSync(join(trayDir, 'win', theme), { recursive: true });
  for (const badge of BADGES) {
    const paint = trayMark(badge, c.mark, c[badge] ?? null, c.glyph);
    writeFileSync(join(trayDir, 'win', theme, `${badge}.png`), png(32, render(32, paint)));
  }
}
const sizes = [16, 24, 32, 48, 64, 128, 256];
const app = Object.fromEntries(sizes.map((s) => [s, png(s, render(s, appIcon))]));
writeFileSync(join(iconsDir, '32x32.png'), app[32]);
writeFileSync(join(iconsDir, '128x128.png'), app[128]);
writeFileSync(join(iconsDir, 'icon.png'), app[256]);
writeFileSync(join(iconsDir, 'icon.ico'), ico(sizes.map((s) => ({ size: s, data: app[s] }))));
const mac = Object.fromEntries([16, 32, 64, 128, 256, 512, 1024].map((s) => [s, png(s, render(s, macAppIcon))]));
writeFileSync(join(iconsDir, 'icon.icns'), icns([
  { type: 'icp4', data: mac[16] }, { type: 'ic11', data: mac[32] },
  { type: 'icp5', data: mac[32] }, { type: 'ic12', data: mac[64] },
  { type: 'ic07', data: mac[128] }, { type: 'ic13', data: mac[256] },
  { type: 'ic08', data: mac[256] }, { type: 'ic14', data: mac[512] },
  { type: 'ic09', data: mac[512] }, { type: 'ic10', data: mac[1024] },
]));
console.log(`icons written to ${iconsDir}`);
