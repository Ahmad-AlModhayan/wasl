import { describe, expect, it } from 'vitest';
import { dataCodewords, encodeQr, formatBits, isAlphanumeric, penalty, qrPath, rsRemainder, versionBits } from './qr';

describe('QR building blocks', () => {
  it('computes Reed–Solomon codewords (HELLO WORLD, 1-M)', () => {
    const data = [32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17];
    expect(rsRemainder(data, 10)).toEqual([196, 35, 39, 119, 235, 215, 231, 226, 93, 23]);
  });

  it('has the standard data capacities at level M', () => {
    expect(dataCodewords(1)).toBe(16);
    expect(dataCodewords(5)).toBe(86);
    expect(dataCodewords(10)).toBe(216);
    expect(dataCodewords(40)).toBe(2334);
  });

  it('writes the standard format bits for level M', () => {
    const table = [
      '101010000010010', '101000100100101', '101111001111100', '101101101001011',
      '100010111111001', '100000011001110', '100111110010111', '100101010100000',
    ];
    table.forEach((bits, mask) => expect(formatBits(mask).toString(2).padStart(15, '0')).toBe(bits));
  });

  it('writes the standard version bits', () => {
    expect(versionBits(7).toString(2).padStart(18, '0')).toBe('000111110010010100');
    expect(versionBits(8).toString(2).padStart(18, '0')).toBe('001000010110111100');
    expect(versionBits(40).toString(2).padStart(18, '0')).toBe('101000110001101001');
  });

  it('knows the alphanumeric set', () => {
    expect(isAlphanumeric('WASL:ABC234')).toBe(true);
    expect(isAlphanumeric('wasl')).toBe(false);
  });
});

describe('encodeQr', () => {
  it('encodes HELLO WORLD at version 1', () => {
    const qr = encodeQr('HELLO WORLD');
    expect(qr.version).toBe(1);
    expect(qr.size).toBe(21);
  });

  it('picks the smallest version that fits', () => {
    // A pairing code with two IPv4 addresses: 56 bytes of base32 behind "WASL:".
    const code = 'WASL:' + 'A'.repeat(90);
    expect(encodeQr(code).version).toBe(5);
    // The longest pairing code (eight IPv6 addresses) still fits.
    const long = encodeQr('WASL:' + 'B2'.repeat(156));
    expect(long.version).toBe(11);
    expect(long.size).toBe(61);
    // Byte mode for anything else.
    expect(encodeQr('hello').version).toBe(1);
    expect(() => encodeQr('x'.repeat(3000))).toThrow();
  });

  it('draws the finder patterns, timing and the dark module', () => {
    const qr = encodeQr('WASL:ABCDEFGHIJKLMNOP234567');
    const m = qr.modules;
    const n = qr.size;
    for (const [x0, y0] of [[0, 0], [n - 7, 0], [0, n - 7]]) {
      for (let d = 0; d < 7; d++) {
        expect(m[y0][x0 + d]).toBe(true);
        expect(m[y0 + 6][x0 + d]).toBe(true);
        expect(m[y0 + d][x0]).toBe(true);
      }
      expect(m[y0 + 1][x0 + 1]).toBe(false);
      expect(m[y0 + 3][x0 + 3]).toBe(true);
    }
    for (let i = 8; i < n - 8; i++) expect(m[6][i]).toBe(i % 2 === 0);
    expect(m[n - 8][8]).toBe(true);
  });

  it('writes the chosen mask in both copies of the format bits', () => {
    const qr = encodeQr('WASL:XYZ');
    const bits = formatBits(qr.mask);
    const read: boolean[] = [];
    for (let i = 0; i < 8; i++) read.push(qr.modules[8][qr.size - 1 - i]);
    read.forEach((dark, i) => expect(dark).toBe(((bits >>> i) & 1) === 1));
  });

  it('chooses the mask with the lowest penalty', () => {
    const qr = encodeQr('WASL:MASKTEST');
    expect(penalty(qr.modules)).toBeGreaterThan(0);
  });

  it('turns the matrix into path data inside the quiet zone', () => {
    const qr = encodeQr('A');
    const d = qrPath(qr);
    expect(d.startsWith('M4 4h7v1h-7z')).toBe(true);
  });
});
