import { describe, expect, it } from 'vitest';
import { GO_ROUTES, isMenuAction } from '../chrome/menu';
import { SHORTCUTS, accelerator, comboOf, reloadsWebview, shortcutFor, shortcutLabel, type KeyLike } from './keys';

function key(code: string, mods: Partial<KeyLike> = {}): KeyLike {
  return { code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods };
}

const inBrowser = { nativeMenu: false, typing: false };

describe('the shortcut map', () => {
  it('goes to the seven sections with ⌘1–⌘7 and Ctrl+1–7, in the design order', () => {
    const order = Object.keys(GO_ROUTES);
    for (let i = 1; i <= 7; i++) {
      expect(shortcutFor(key(`Digit${i}`, { metaKey: true }), 'mac', inBrowser)?.action).toBe(order[i - 1]);
      expect(shortcutFor(key(`Digit${i}`, { ctrlKey: true }), 'windows', inBrowser)?.action).toBe(order[i - 1]);
    }
  });

  it('syncs with ⌘R on macOS, and Ctrl+R or F5 on Windows', () => {
    expect(shortcutFor(key('KeyR', { metaKey: true }), 'mac', inBrowser)?.action).toBe('sync_now');
    expect(shortcutFor(key('KeyR', { ctrlKey: true }), 'windows', inBrowser)?.action).toBe('sync_now');
    expect(shortcutFor(key('F5'), 'windows', inBrowser)?.action).toBe('sync_now');
    expect(shortcutFor(key('KeyR', { ctrlKey: true }), 'mac', inBrowser)).toBeNull();
  });

  it('finds with ⌘F, and Ctrl+F or Ctrl+E', () => {
    expect(shortcutFor(key('KeyF', { metaKey: true }), 'mac', inBrowser)?.action).toBe('find');
    expect(shortcutFor(key('KeyF', { ctrlKey: true }), 'windows', inBrowser)?.action).toBe('find');
    expect(shortcutFor(key('KeyE', { ctrlKey: true }), 'windows', inBrowser)?.action).toBe('find');
  });

  it('toggles the inspector with ⌥⌘I and Ctrl+I', () => {
    expect(shortcutFor(key('KeyI', { metaKey: true, altKey: true }), 'mac', inBrowser)?.action).toBe('toggle_inspector');
    expect(shortcutFor(key('KeyI', { ctrlKey: true }), 'windows', inBrowser)?.action).toBe('toggle_inspector');
  });

  it('cycles the panes with F6 and Shift+F6 on Windows only', () => {
    expect(shortcutFor(key('F6'), 'windows', inBrowser)?.action).toBe('next_pane');
    expect(shortcutFor(key('F6', { shiftKey: true }), 'windows', inBrowser)?.action).toBe('prev_pane');
    expect(shortcutFor(key('F6'), 'mac', inBrowser)).toBeNull();
  });

  it('leaves the keys the macOS menu bar sends to the menu, but handles them in a browser', () => {
    const syncKey = key('KeyR', { metaKey: true });
    expect(shortcutFor(syncKey, 'mac', { nativeMenu: true, typing: false })).toBeNull();
    expect(shortcutFor(syncKey, 'mac', inBrowser)?.action).toBe('sync_now');
    // Find isn't in the menu bar, so the window always handles it.
    expect(shortcutFor(key('KeyF', { metaKey: true }), 'mac', { nativeMenu: true, typing: false })?.action).toBe('find');
  });

  it('fires only find, the sections and F6 from a Windows text field', () => {
    const typing = { nativeMenu: false, typing: true };
    expect(shortcutFor(key('F5'), 'windows', typing)).toBeNull();
    expect(shortcutFor(key('KeyN', { ctrlKey: true }), 'windows', typing)).toBeNull();
    expect(shortcutFor(key('KeyP', { ctrlKey: true }), 'windows', typing)).toBeNull();
    expect(shortcutFor(key('KeyR', { ctrlKey: true }), 'windows', typing)).toBeNull();
    expect(shortcutFor(key('KeyF', { ctrlKey: true }), 'windows', typing)?.action).toBe('find');
    expect(shortcutFor(key('Digit3', { ctrlKey: true }), 'windows', typing)?.action).toBe('go_devices');
    expect(shortcutFor(key('F6'), 'windows', typing)?.action).toBe('next_pane');
    for (const s of SHORTCUTS.windows) {
      if (s.typing) expect(s.action).toMatch(/^(find|go_.*|next_pane|prev_pane)$/);
    }
  });

  it('keeps the macOS menu shortcuts working from a text field in a browser, as the menu bar does', () => {
    const typing = { nativeMenu: false, typing: true };
    expect(shortcutFor(key('KeyN', { metaKey: true }), 'mac', typing)?.action).toBe('new_pair');
    for (const s of SHORTCUTS.mac) expect(s.keys).toContain('Mod');
  });

  it('never lets F5 or Ctrl+R reload the Windows webview, typing or not', () => {
    expect(reloadsWebview(key('F5'), 'windows')).toBe(true);
    expect(reloadsWebview(key('F5', { ctrlKey: true }), 'windows')).toBe(true);
    expect(reloadsWebview(key('KeyR', { ctrlKey: true }), 'windows')).toBe(true);
    expect(reloadsWebview(key('KeyR', { ctrlKey: true, shiftKey: true }), 'windows')).toBe(true);
    expect(reloadsWebview(key('KeyR'), 'windows')).toBe(false);
    expect(reloadsWebview(key('KeyR', { metaKey: true }), 'mac')).toBe(false);
  });

  it('ignores the Windows key', () => {
    expect(comboOf(key('KeyR', { metaKey: true }), 'windows')).toBeNull();
  });

  it('uses only actions the menu bar or the shell knows, and no key twice', () => {
    for (const map of Object.values(SHORTCUTS)) {
      for (const s of map) expect(isMenuAction(s.action) || ['find', 'next_pane', 'prev_pane'].includes(s.action)).toBe(true);
      expect(new Set(map.map((s) => s.keys)).size).toBe(map.length);
    }
  });

  it('matches the macOS menu bar’s shortcuts (menu.rs)', () => {
    expect(accelerator('toggle_pause', 'mac')).toBe('Alt+CmdOrCtrl+P');
    expect(accelerator('show_in_finder', 'mac')).toBe('Shift+CmdOrCtrl+R');
    expect(accelerator('toggle_sidebar', 'mac')).toBe('Ctrl+CmdOrCtrl+S');
  });

  it('writes shortcuts the way each platform does', () => {
    expect(shortcutLabel('toggle_pause', 'mac')).toBe('⌥⌘P');
    expect(shortcutLabel('show_in_finder', 'windows')).toBe('Ctrl+Shift+E');
    expect(shortcutLabel('sync_now', 'windows')).toBe('Ctrl+R');
    expect(shortcutLabel('toggle_sidebar', 'windows')).toBeNull();
  });
});
