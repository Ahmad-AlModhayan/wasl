// Settings shared by every Wasl window (#68): the main window and, on macOS, the separate Settings window (D5).
// A change is saved in this origin's localStorage, which both windows share, and announced to the other windows
// with a Tauri event, so each applies it at once without a reload. The browser dev mock has no Tauri events
// across windows; there localStorage's own `storage` event carries it.

import { emit, listen } from '@tauri-apps/api/event';
import type { DaemonStatus } from './types';

export type Accent = 'wasl' | 'system';
export type SettingsPane = 'general' | 'accounts' | 'sync' | 'notifications' | 'advanced';
export type SettingKey = 'language' | 'accent' | 'pane';

/** The panes, in the order the macOS toolbar and the Windows page show them. */
export const PANES: readonly SettingsPane[] = ['general', 'accounts', 'sync', 'notifications', 'advanced'];

export const STORE_KEYS: Record<SettingKey, string> = {
  language: 'wasl.language',
  accent: 'wasl.accent',
  pane: 'wasl.settings.pane',
};

/** Tauri event every window listens to; the payload says which setting changed and which window changed it. */
export const SETTINGS_EVENT = 'settings://changed';

export interface SettingChange {
  key: SettingKey;
  value: string;
  from: string;
}

/** Each window's own id, so it ignores its own announcements. */
const WINDOW_ID = Math.random().toString(36).slice(2);

export function readStored(key: SettingKey): string | null {
  try {
    return localStorage.getItem(STORE_KEYS[key]);
  } catch {
    return null;
  }
}

function writeStored(key: SettingKey, value: string): void {
  try {
    localStorage.setItem(STORE_KEYS[key], value);
  } catch {
    // Private mode or storage off: the change still applies in this window.
  }
}

export function accentOf(value: string | null | undefined): Accent {
  return value === 'system' ? 'system' : 'wasl';
}

/** What a Settings pane shows from the service's status. The window redraws a pane on a status only when this
 *  changes, so a status every second while syncing doesn't rebuild the controls under the keyboard. */
export function paneSignature(pane: SettingsPane, status: DaemonStatus | null): string {
  if (!status) return '';
  switch (pane) {
    case 'accounts':
      return JSON.stringify(status.accounts.map((a) => [a.id, a.email, Boolean(a.auth_error)]));
    case 'sync':
      return JSON.stringify([status.pairs.length, status.pairs.filter((p) => p.pair.paused).length]);
    case 'advanced':
      return status.version;
    default:
      return '';
  }
}

export function paneOf(value: string | null | undefined): SettingsPane {
  return PANES.includes(value as SettingsPane) ? (value as SettingsPane) : 'general';
}

/**
 * Puts the accent on the page root. `system` sets `data-kf-accent="system"`, which kahf-design's desktop layer turns
 * into CSS `AccentColor` where the engine has it (WebKit on macOS). Windows' WebView2 doesn't (`cssAccent` false), so
 * Rust reads the person's accent and it arrives here as `systemColor`; without one the page stays Wasl navy rather
 * than map the primary colour to a value WebView2 can't resolve. Forced colours ignore both (the system colours win
 * there). Returns whether the system accent is showing.
 */
export function applyAccent(root: HTMLElement, accent: Accent, systemColor: string | null = null, cssAccent = true): boolean {
  const color = systemColor && /^#[0-9a-f]{6}$/i.test(systemColor) ? systemColor : null;
  const shown = accent === 'system' && (cssAccent || color !== null);
  if (shown) root.dataset.kfAccent = 'system';
  else delete root.dataset.kfAccent;
  if (shown && color) {
    root.dataset.waslAccentColor = '';
    root.style.setProperty('--wasl-system-accent', color);
    root.style.setProperty('--wasl-system-on-accent', onColor(color));
  } else {
    delete root.dataset.waslAccentColor;
    root.style.removeProperty('--wasl-system-accent');
    root.style.removeProperty('--wasl-system-on-accent');
  }
  return shown;
}

/** Black or white text on a colour, whichever reads better (WCAG relative luminance). */
export function onColor(hex: string): '#000000' | '#ffffff' {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  const l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  // Contrast with white is 1.05 / (l + 0.05); with black (l + 0.05) / 0.05. They are equal at l ≈ 0.179.
  return l > 0.179 ? '#000000' : '#ffffff';
}

/** Saves a change and tells the other windows. */
export function shareSetting(key: SettingKey, value: string): void {
  writeStored(key, value);
  void emit(SETTINGS_EVENT, { key, value, from: WINDOW_ID } satisfies SettingChange).catch(() => undefined);
}

/** Calls `fn` when another window changes a setting. */
export function onSettingChange(fn: (key: SettingKey, value: string) => void): void {
  void listen<SettingChange>(SETTINGS_EVENT, (e) => {
    if (e.payload.from !== WINDOW_ID) fn(e.payload.key, e.payload.value);
  }).catch(() => undefined);
  window.addEventListener('storage', (e) => {
    const key = (Object.keys(STORE_KEYS) as SettingKey[]).find((k) => STORE_KEYS[k] === e.key);
    if (key && e.newValue !== null) fn(key, e.newValue);
  });
}
