// The window's shortcuts, one map per platform, from the design's Keyboard table (#63, design D7). No global
// hotkeys: these work only while Wasl's window has focus. On macOS the menu bar owns most of them (#61): in the app
// the menu sends `menu://action`, so the window leaves those keys alone; in a plain browser (the dev mock) it
// handles them itself.

import type { MenuAction } from '../chrome/menu';
import type { Platform } from '../platform';

/** What a shortcut does: a menu bar action, or one only the window has. */
export type ShellAction = MenuAction | 'find' | 'next_pane' | 'prev_pane';

export interface Shortcut {
  /** Modifiers in the order Ctrl, Alt, Shift, Mod, then the key's `code` (layout-independent): "Alt+Mod+KeyP".
   *  Mod is ⌘ on macOS and Ctrl on Windows; Ctrl on macOS is ⌃. */
  keys: string;
  action: ShellAction;
  /** macOS: the menu bar has this shortcut, so in the app the menu sends it. */
  menu?: boolean;
  /** Fires while a text field has focus: find, the sections and F6 (and macOS menu shortcuts, as the menu bar
   *  does). Others wait, so typing isn't lost. */
  typing?: boolean;
}

const GO: MenuAction[] = ['go_pairs', 'go_views', 'go_devices', 'go_problems', 'go_agents', 'go_history', 'go_bin'];

export const SHORTCUTS: Record<Platform, Shortcut[]> = {
  mac: [
    { keys: 'Mod+KeyN', action: 'new_pair', menu: true, typing: true },
    { keys: 'Mod+KeyR', action: 'sync_now', menu: true, typing: true },
    { keys: 'Alt+Mod+KeyP', action: 'toggle_pause', menu: true, typing: true },
    { keys: 'Mod+KeyF', action: 'find', typing: true },
    { keys: 'Alt+Mod+KeyI', action: 'toggle_inspector', menu: true, typing: true },
    { keys: 'Ctrl+Mod+KeyS', action: 'toggle_sidebar', menu: true, typing: true },
    { keys: 'Shift+Mod+KeyR', action: 'show_in_finder', menu: true, typing: true },
    ...GO.map((action, i) => ({ keys: `Mod+Digit${i + 1}`, action, menu: true, typing: true })),
    { keys: 'Mod+Comma', action: 'settings', menu: true, typing: true },
  ],
  windows: [
    // Only finding, the sections and F6 fire from a text field: Ctrl+N or Ctrl+P there would leave or change what
    // is being typed. Ctrl+R and F5 never reach the webview, which would reload (see `reloadsWebview`).
    { keys: 'Mod+KeyN', action: 'new_pair' },
    { keys: 'Mod+KeyR', action: 'sync_now' },
    { keys: 'F5', action: 'sync_now' },
    // Ctrl+Shift+P is taken by editors; Ctrl+P works in the window only (design).
    { keys: 'Mod+KeyP', action: 'toggle_pause' },
    { keys: 'Mod+KeyF', action: 'find', typing: true },
    { keys: 'Mod+KeyE', action: 'find', typing: true },
    { keys: 'Mod+KeyI', action: 'toggle_inspector' },
    { keys: 'Shift+Mod+KeyE', action: 'show_in_finder' },
    ...GO.map((action, i) => ({ keys: `Mod+Digit${i + 1}`, action, typing: true })),
    { keys: 'Mod+Comma', action: 'settings' },
    // F6 moves between the panes from anywhere, a text field too: it is how a keyboard leaves one.
    { keys: 'F6', action: 'next_pane', typing: true },
    { keys: 'Shift+F6', action: 'prev_pane', typing: true },
  ],
};

/** The parts of a KeyboardEvent a shortcut reads. */
export interface KeyLike {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** A key event as a shortcut string for the platform, or null when it can't be one (⌘ held on Windows). */
export function comboOf(e: KeyLike, platform: Platform): string | null {
  const mods: string[] = [];
  if (platform === 'mac') {
    if (e.ctrlKey) mods.push('Ctrl');
  } else if (e.metaKey) {
    return null; // the Windows key belongs to the system
  }
  if (e.altKey) mods.push('Alt');
  if (e.shiftKey) mods.push('Shift');
  if (platform === 'mac' ? e.metaKey : e.ctrlKey) mods.push('Mod');
  return [...mods, e.code].join('+');
}

/**
 * The shortcut a key press means here, or null. `nativeMenu`: the macOS menu bar is there to send its own
 * shortcuts. `typing`: a text field has focus.
 */
export function shortcutFor(e: KeyLike, platform: Platform, opts: { nativeMenu: boolean; typing: boolean }): Shortcut | null {
  const combo = comboOf(e, platform);
  if (!combo) return null;
  const s = SHORTCUTS[platform].find((x) => x.keys === combo);
  if (!s) return null;
  if (s.menu && opts.nativeMenu) return null;
  if (opts.typing && !s.typing) return null;
  return s;
}

/**
 * Windows: F5 (with any modifier) and Ctrl+R or Ctrl+Shift+R reload WebView2, losing a half-typed form. The window
 * always takes them, also from a text field and when nothing runs.
 */
export function reloadsWebview(e: KeyLike, platform: Platform): boolean {
  if (platform !== 'windows' || e.metaKey) return false;
  if (e.code === 'F5') return true;
  return e.code === 'KeyR' && e.ctrlKey && !e.altKey;
}

const MAC_MODS: Record<string, string> = { Ctrl: '⌃', Alt: '⌥', Shift: '⇧', Mod: '⌘' };

function keyName(code: string): string {
  if (code.startsWith('Key')) return code.slice(3);
  if (code.startsWith('Digit')) return code.slice(5);
  if (code === 'Comma') return ',';
  return code;
}

/** The first shortcut for an action, as people read it: "⌥⌘P" on macOS, "Ctrl+P" on Windows; null for none. */
export function shortcutLabel(action: ShellAction, platform: Platform): string | null {
  const s = SHORTCUTS[platform].find((x) => x.action === action);
  if (!s) return null;
  const parts = s.keys.split('+');
  const key = keyName(parts.pop()!);
  if (platform === 'mac') return parts.map((m) => MAC_MODS[m]).join('') + key;
  // Windows writes Ctrl first: "Ctrl+Shift+E".
  const order = ['Mod', 'Alt', 'Shift'];
  parts.sort((a, b) => order.indexOf(a) - order.indexOf(b));
  return [...parts.map((m) => (m === 'Mod' ? 'Ctrl' : m)), key].join('+');
}

/** The first shortcut for an action as a Tauri accelerator ("Alt+CmdOrCtrl+P"), for native menus to show. */
export function accelerator(action: ShellAction, platform: Platform): string | undefined {
  const s = SHORTCUTS[platform].find((x) => x.action === action);
  if (!s) return undefined;
  const parts = s.keys.split('+');
  const key = keyName(parts.pop()!);
  return [...parts.map((m) => (m === 'Mod' ? 'CmdOrCtrl' : m)), key].join('+');
}
