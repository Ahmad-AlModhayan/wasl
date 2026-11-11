import { describe, expect, it } from 'vitest';
import { chromeInsets, toolbarHeight, type Chrome } from './index';

const left: Chrome = { lights: { x: 20, y: 18, width: 54, height: 16 }, windowWidth: 980, opaque: false };
// macOS running Wasl in Arabic: AppKit's own place, on the right.
const right: Chrome = { lights: { x: 906, y: 6, width: 54, height: 16 }, windowWidth: 980, opaque: false };

describe('chromeInsets', () => {
  it('keeps the start clear when the lights are on the left of an English page', () => {
    expect(chromeInsets(left, 'ltr')).toEqual({ start: 86, end: 0 });
  });

  it('keeps the end clear when Wasl is in Arabic on an English macOS', () => {
    expect(chromeInsets(left, 'rtl')).toEqual({ start: 0, end: 86 });
  });

  it('keeps the start clear when macOS runs Wasl in Arabic', () => {
    expect(chromeInsets(right, 'rtl')).toEqual({ start: 86, end: 0 });
  });

  it('keeps the end clear for an English page when macOS runs Wasl in Arabic', () => {
    expect(chromeInsets(right, 'ltr')).toEqual({ start: 0, end: 86 });
  });

  it('keeps nothing clear in full screen or without traffic lights', () => {
    expect(chromeInsets({ ...left, lights: null }, 'ltr')).toEqual({ start: 0, end: 0 });
    expect(chromeInsets({ ...left, windowWidth: 0 }, 'rtl')).toEqual({ start: 0, end: 0 });
  });
});

describe('toolbarHeight', () => {
  it('is the design 52px when the lights fit centred in it', () => {
    expect(toolbarHeight(left)).toBe(52);
    expect(toolbarHeight({ ...left, lights: null })).toBe(52);
  });

  it('grows when the lights sit lower than the strip allows', () => {
    expect(toolbarHeight({ ...left, lights: { x: 20, y: 24, width: 54, height: 16 } })).toBe(64);
  });
});
