// A small QR code encoder (ISO/IEC 18004) for the device pairing code: one segment in alphanumeric
// mode (upper case, digits, space and $%*+-./:) or byte mode (UTF-8), error correction level M,
// versions 1–40, the mask with the lowest penalty. Pure functions, no DOM; qrPath() turns the matrix
// into SVG path data. Follows the structure of Project Nayuki's QR Code generator (MIT).

const ALNUM = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:';

/** Error correction codewords per block at level M, by version (index 0 unused). */
const ECC_PER_BLOCK_M = [
  -1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28,
  28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
];
/** Error correction blocks at level M, by version. */
const BLOCKS_M = [
  -1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33,
  35, 37, 38, 40, 43, 45, 47, 49,
];
/** Level M's two format bits. */
const FORMAT_M = 0;

export interface Qr {
  version: number;
  size: number;
  mask: number;
  /** modules[y][x], true = dark. */
  modules: boolean[][];
}

export function isAlphanumeric(text: string): boolean {
  for (const c of text) if (!ALNUM.includes(c)) return false;
  return true;
}

/** Encodes `text` at the smallest version that holds it. Throws when it is too long for version 40. */
export function encodeQr(text: string): Qr {
  const alnum = isAlphanumeric(text);
  const bytes = alnum ? [] : [...new TextEncoder().encode(text)];
  const count = alnum ? text.length : bytes.length;
  for (let version = 1; version <= 40; version++) {
    const capacityBits = dataCodewords(version) * 8;
    const countBits = alnum ? (version <= 9 ? 9 : version <= 26 ? 11 : 13) : version <= 9 ? 8 : 16;
    const bits: number[] = [];
    push(bits, alnum ? 0b0010 : 0b0100, 4);
    push(bits, count, countBits);
    if (alnum) {
      let i = 0;
      for (; i + 1 < text.length; i += 2) push(bits, ALNUM.indexOf(text[i]) * 45 + ALNUM.indexOf(text[i + 1]), 11);
      if (i < text.length) push(bits, ALNUM.indexOf(text[i]), 6);
    } else {
      for (const b of bytes) push(bits, b, 8);
    }
    if (bits.length > capacityBits) continue;
    push(bits, 0, Math.min(4, capacityBits - bits.length)); // terminator
    push(bits, 0, (8 - (bits.length % 8)) % 8);
    for (let pad = 0xec; bits.length < capacityBits; pad ^= 0xec ^ 0x11) push(bits, pad, 8);
    const data: number[] = [];
    for (let i = 0; i < bits.length; i += 8) {
      let b = 0;
      for (let j = 0; j < 8; j++) b = (b << 1) | bits[i + j];
      data.push(b);
    }
    return build(version, addEcc(version, data));
  }
  throw new Error('Text too long for a QR code');
}

function push(bits: number[], value: number, len: number): void {
  for (let i = len - 1; i >= 0; i--) bits.push((value >>> i) & 1);
}

// ---------------------------------------------------------------- capacity

/** Modules left for data and error correction once the function patterns are drawn. */
export function rawDataModules(version: number): number {
  let n = (16 * version + 128) * version + 64;
  if (version >= 2) {
    const align = Math.floor(version / 7) + 2;
    n -= (25 * align - 10) * align - 55;
    if (version >= 7) n -= 36;
  }
  return n;
}

export function dataCodewords(version: number): number {
  return Math.floor(rawDataModules(version) / 8) - ECC_PER_BLOCK_M[version] * BLOCKS_M[version];
}

// ---------------------------------------------------------------- Reed–Solomon over GF(256), x^8+x^4+x^3+x^2+1

function gfMul(x: number, y: number): number {
  let z = 0;
  for (let i = 7; i >= 0; i--) {
    z = (z << 1) ^ ((z >>> 7) * 0x11d);
    z ^= ((y >>> i) & 1) * x;
  }
  return z;
}

function rsDivisor(degree: number): number[] {
  const result = new Array<number>(degree).fill(0);
  result[degree - 1] = 1;
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < degree; j++) {
      result[j] = gfMul(result[j], root);
      if (j + 1 < degree) result[j] ^= result[j + 1];
    }
    root = gfMul(root, 0x02);
  }
  return result;
}

/** The error correction codewords for one block. */
export function rsRemainder(data: number[], degree: number): number[] {
  const divisor = rsDivisor(degree);
  const result = new Array<number>(degree).fill(0);
  for (const b of data) {
    const factor = b ^ (result.shift() as number);
    result.push(0);
    for (let i = 0; i < degree; i++) result[i] ^= gfMul(divisor[i], factor);
  }
  return result;
}

/** Splits the data into blocks, adds each block's error correction, and interleaves them. */
function addEcc(version: number, data: number[]): number[] {
  const blocks = BLOCKS_M[version];
  const eccLen = ECC_PER_BLOCK_M[version];
  const raw = Math.floor(rawDataModules(version) / 8);
  const shortBlocks = blocks - (raw % blocks);
  const shortLen = Math.floor(raw / blocks);
  const all: number[][] = [];
  for (let i = 0, k = 0; i < blocks; i++) {
    const dat = data.slice(k, k + shortLen - eccLen + (i < shortBlocks ? 0 : 1));
    k += dat.length;
    const ecc = rsRemainder(dat, eccLen);
    if (i < shortBlocks) dat.push(0); // placeholder, skipped below
    all.push(dat.concat(ecc));
  }
  const out: number[] = [];
  for (let i = 0; i < all[0].length; i++) {
    for (let j = 0; j < all.length; j++) {
      if (i !== shortLen - eccLen || j >= shortBlocks) out.push(all[j][i]);
    }
  }
  return out;
}

// ---------------------------------------------------------------- format and version information

/** The 15 format bits for level M and `mask`, BCH-coded and XOR-masked. */
export function formatBits(mask: number): number {
  const data = (FORMAT_M << 3) | mask;
  let rem = data;
  for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
  return ((data << 10) | rem) ^ 0x5412;
}

/** The 18 version bits (versions 7 and up). */
export function versionBits(version: number): number {
  let rem = version;
  for (let i = 0; i < 12; i++) rem = (rem << 1) ^ ((rem >>> 11) * 0x1f25);
  return (version << 12) | rem;
}

function alignmentPositions(version: number): number[] {
  if (version === 1) return [];
  const n = Math.floor(version / 7) + 2;
  const step = version === 32 ? 26 : Math.ceil((version * 4 + 4) / (n * 2 - 2)) * 2;
  const out = [6];
  for (let pos = version * 4 + 17 - 7; out.length < n; pos -= step) out.splice(1, 0, pos);
  return out;
}

// ---------------------------------------------------------------- the matrix

const MASKS: Array<(x: number, y: number) => boolean> = [
  (x, y) => (x + y) % 2 === 0,
  (_x, y) => y % 2 === 0,
  (x) => x % 3 === 0,
  (x, y) => (x + y) % 3 === 0,
  (x, y) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0,
  (x, y) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x, y) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x, y) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

function build(version: number, codewords: number[]): Qr {
  const size = version * 4 + 17;
  const grid = (): boolean[][] => Array.from({ length: size }, () => new Array<boolean>(size).fill(false));
  const modules = grid();
  const fixed = grid();
  const set = (x: number, y: number, dark: boolean) => {
    modules[y][x] = dark;
    fixed[y][x] = true;
  };

  // Timing patterns, then finders (with separators) and alignment patterns over them.
  for (let i = 0; i < size; i++) {
    set(6, i, i % 2 === 0);
    set(i, 6, i % 2 === 0);
  }
  for (const [cx, cy] of [[3, 3], [size - 4, 3], [3, size - 4]]) {
    for (let dy = -4; dy <= 4; dy++) {
      for (let dx = -4; dx <= 4; dx++) {
        const x = cx + dx;
        const y = cy + dy;
        if (x < 0 || y < 0 || x >= size || y >= size) continue;
        const d = Math.max(Math.abs(dx), Math.abs(dy));
        set(x, y, d !== 2 && d !== 4);
      }
    }
  }
  const align = alignmentPositions(version);
  const last = align.length - 1;
  align.forEach((ax, i) =>
    align.forEach((ay, j) => {
      if ((i === 0 && j === 0) || (i === 0 && j === last) || (i === last && j === 0)) return;
      for (let dy = -2; dy <= 2; dy++)
        for (let dx = -2; dx <= 2; dx++) set(ax + dx, ay + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
    }),
  );
  drawFormat(modules, fixed, size, 0); // reserves the cells; redrawn with the real mask below
  if (version >= 7) {
    const bits = versionBits(version);
    for (let i = 0; i < 18; i++) {
      const dark = ((bits >>> i) & 1) === 1;
      const a = size - 11 + (i % 3);
      const b = Math.floor(i / 3);
      set(a, b, dark);
      set(b, a, dark);
    }
  }

  // Codewords in the zigzag, two columns at a time from the right, skipping the vertical timing column.
  let i = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let vert = 0; vert < size; vert++) {
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const upward = ((right + 1) & 2) === 0;
        const y = upward ? size - 1 - vert : vert;
        if (!fixed[y][x] && i < codewords.length * 8) {
          modules[y][x] = ((codewords[i >>> 3] >>> (7 - (i & 7))) & 1) === 1;
          i++;
        }
      }
    }
  }

  let best = 0;
  let bestPenalty = Infinity;
  for (let m = 0; m < 8; m++) {
    const candidate = modules.map((row) => row.slice());
    applyMask(candidate, fixed, m);
    drawFormat(candidate, fixed, size, m);
    const p = penalty(candidate);
    if (p < bestPenalty) {
      best = m;
      bestPenalty = p;
    }
  }
  applyMask(modules, fixed, best);
  drawFormat(modules, fixed, size, best);
  return { version, size, mask: best, modules };
}

function applyMask(modules: boolean[][], fixed: boolean[][], mask: number): void {
  const f = MASKS[mask];
  for (let y = 0; y < modules.length; y++)
    for (let x = 0; x < modules.length; x++) if (!fixed[y][x] && f(x, y)) modules[y][x] = !modules[y][x];
}

function drawFormat(modules: boolean[][], fixed: boolean[][], size: number, mask: number): void {
  const bits = formatBits(mask);
  const bit = (i: number) => ((bits >>> i) & 1) === 1;
  const set = (x: number, y: number, dark: boolean) => {
    modules[y][x] = dark;
    fixed[y][x] = true;
  };
  // Around the top-left finder.
  for (let i = 0; i <= 5; i++) set(8, i, bit(i));
  set(8, 7, bit(6));
  set(8, 8, bit(7));
  set(7, 8, bit(8));
  for (let i = 9; i < 15; i++) set(14 - i, 8, bit(i));
  // Split between the other two finders.
  for (let i = 0; i < 8; i++) set(size - 1 - i, 8, bit(i));
  for (let i = 8; i < 15; i++) set(8, size - 15 + i, bit(i));
  set(8, size - 8, true); // the dark module
}

/** The standard's penalty score (lower is easier to read). */
export function penalty(m: boolean[][]): number {
  const n = m.length;
  let score = 0;
  const line = (get: (i: number) => boolean) => {
    // Runs of five or more.
    let run = 1;
    for (let i = 1; i <= n; i++) {
      if (i < n && get(i) === get(i - 1)) run++;
      else {
        if (run >= 5) score += 3 + (run - 5);
        run = 1;
      }
    }
    // Finder-like 1011101 with four light modules on either side.
    for (let i = 0; i + 11 <= n; i++) {
      const w = Array.from({ length: 11 }, (_, k) => get(i + k));
      const core = (o: number) => w[o] && !w[o + 1] && w[o + 2] && w[o + 3] && w[o + 4] && !w[o + 5] && w[o + 6];
      if (core(0) && !w[7] && !w[8] && !w[9] && !w[10]) score += 40;
      if (!w[0] && !w[1] && !w[2] && !w[3] && core(4)) score += 40;
    }
  };
  for (let y = 0; y < n; y++) line((i) => m[y][i]);
  for (let x = 0; x < n; x++) line((i) => m[i][x]);
  let dark = 0;
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      if (m[y][x]) dark++;
      if (x + 1 < n && y + 1 < n) {
        const c = m[y][x];
        if (m[y][x + 1] === c && m[y + 1][x] === c && m[y + 1][x + 1] === c) score += 3;
      }
    }
  }
  const total = n * n;
  score += (Math.ceil(Math.abs(dark * 20 - total * 10) / total) - 1) * 10;
  return score;
}

/** SVG path data for the dark modules, one unit per module, offset by a quiet zone of `margin`. */
export function qrPath(qr: Qr, margin = 4): string {
  const parts: string[] = [];
  qr.modules.forEach((row, y) => {
    let x = 0;
    while (x < qr.size) {
      if (!row[x]) {
        x++;
        continue;
      }
      const start = x;
      while (x < qr.size && row[x]) x++;
      parts.push(`M${start + margin} ${y + margin}h${x - start}v1h-${x - start}z`);
    }
  });
  return parts.join('');
}
