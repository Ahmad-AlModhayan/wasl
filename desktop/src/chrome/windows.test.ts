import { describe, expect, it } from 'vitest';
import { DICTS } from '../i18n';
import { GLYPHS, captionFace, glyphSource, type Face } from './captionButtons';
import { backdropFor, isAltSpace, physicalArea, shouldDrawTitleBar } from './windows';

const WINDOWS = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0';
const MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)';
const LINUX = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)';

describe('caption buttons', () => {
  it('use the Segoe Fluent Icons chrome glyphs', () => {
    expect(GLYPHS).toEqual({ minimize: '', maximize: '', restore: '', close: '' });
  });

  it('turn maximize into restore while the window is maximized', () => {
    expect(captionFace('maximize', false)).toBe('maximize');
    expect(captionFace('maximize', true)).toBe('restore');
    expect(captionFace('minimize', true)).toBe('minimize');
    expect(captionFace('close', true)).toBe('close');
  });

  it('draw glyphs with the icon font on Windows and as SVG elsewhere', () => {
    expect(glyphSource(WINDOWS)).toBe('font');
    expect(glyphSource(MAC)).toBe('svg');
  });

  it('have a name in English and Arabic for every face', () => {
    for (const face of Object.keys(GLYPHS) as Face[]) {
      expect(DICTS.en[`chrome.${face}`]).toBeTruthy();
      expect(DICTS.ar[`chrome.${face}`]).toBeTruthy();
    }
  });
});

describe('the Windows title bar', () => {
  it('is drawn in the app on Windows but not on Linux, which keeps its frame', () => {
    expect(shouldDrawTitleBar(WINDOWS, true)).toBe(true);
    expect(shouldDrawTitleBar(LINUX, true)).toBe(false);
  });

  it('is drawn by the dev mock on any system', () => {
    expect(shouldDrawTitleBar(MAC, false)).toBe(true);
  });

  it('uses Mica on Windows 11 only', () => {
    expect(backdropFor('Windows', '15.0.0')).toBe('mica');
    expect(backdropFor('Windows', '13.0.0')).toBe('mica');
    expect(backdropFor('Windows', '10.0.0')).toBe('solid'); // Windows 10
    expect(backdropFor('macOS', '15.0.0')).toBe('solid');
    expect(backdropFor('', '')).toBe('solid');
  });

  it('opens the system menu on Alt+Space alone', () => {
    const key = { altKey: true, ctrlKey: false, shiftKey: false, metaKey: false, code: 'Space' };
    expect(isAltSpace(key)).toBe(true);
    expect(isAltSpace({ ...key, altKey: false })).toBe(false);
    expect(isAltSpace({ ...key, ctrlKey: true })).toBe(false);
    expect(isAltSpace({ ...key, code: 'KeyA' })).toBe(false);
  });

  it('reports the maximize button in physical pixels, edges rounded so neighbours meet', () => {
    expect(physicalArea({ left: 100.5, top: 0, width: 46, height: 48 }, 1.5)).toEqual({ x: 151, y: 0, width: 69, height: 72 });
    expect(physicalArea({ left: 888, top: 0, width: 46, height: 48 }, 1)).toEqual({ x: 888, y: 0, width: 46, height: 48 });
  });
});
