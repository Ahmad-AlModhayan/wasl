// Window chrome shared by the platforms (#61, #62): where the window's own controls sit, as CSS properties the
// shell lays out around. macOS reports its traffic lights from Rust (`window_chrome`); see ./mac.ts. Windows draws
// its own title bar; see ./windows.ts.

import type { Platform } from '../platform';

/** Mounts the platform's chrome. Each platform's module (and its CSS) loads only on that platform; on Linux, which
 *  takes the Windows path, ./windows.ts leaves the native frame alone. */
export async function mountChrome(platform: Platform): Promise<void> {
  if (platform === 'mac') (await import('./mac')).mountMacChrome();
  else (await import('./windows')).mountWindowsChrome();
}

/** A rectangle in CSS pixels from the window's top-left corner. */
export interface Frame {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** What `window_chrome` answers. */
export interface Chrome {
  /** The traffic lights; null in full screen and on platforms without them. */
  lights: Frame | null;
  windowWidth: number;
  /** Reduce Transparency or Increase Contrast is on: the sidebar gets an opaque fill. */
  opaque: boolean;
}

/** The design's toolbar strip (macOS 26 unified toolbar). */
export const TOOLBAR_HEIGHT = 52;

/** Space between the traffic lights and the first thing beside them. */
const GAP = 12;

/**
 * How much of each inline edge the traffic lights take, for the page's direction. macOS puts them on the right
 * only when it runs Wasl in Arabic; Wasl's own Arabic only flips the page, so the lights can sit at either end.
 */
export function chromeInsets(c: Chrome, dir: 'ltr' | 'rtl'): { start: number; end: number } {
  const l = c.lights;
  if (!l || c.windowWidth <= 0) return { start: 0, end: 0 };
  const onLeft = l.x + l.width / 2 < c.windowWidth / 2;
  const left = onLeft ? Math.ceil(l.x + l.width + GAP) : 0;
  const right = onLeft ? 0 : Math.ceil(c.windowWidth - l.x + GAP);
  return dir === 'ltr' ? { start: left, end: right } : { start: right, end: left };
}

/** The strip is the design's 52px, taller only if the lights would not fit centred in it. */
export function toolbarHeight(c: Chrome): number {
  const l = c.lights;
  return l ? Math.max(TOOLBAR_HEIGHT, Math.ceil(2 * l.y + l.height)) : TOOLBAR_HEIGHT;
}

/** Writes the chrome onto the root: `--chrome-inset-inline-start/end`, `--chrome-toolbar-height`, `data-chrome-opaque`. */
export function applyChrome(root: HTMLElement, c: Chrome): void {
  const dir = root.dir === 'rtl' ? 'rtl' : 'ltr';
  const { start, end } = chromeInsets(c, dir);
  root.style.setProperty('--chrome-inset-inline-start', `${start}px`);
  root.style.setProperty('--chrome-inset-inline-end', `${end}px`);
  root.style.setProperty('--chrome-toolbar-height', `${toolbarHeight(c)}px`);
  root.toggleAttribute('data-chrome-opaque', c.opaque);
}
