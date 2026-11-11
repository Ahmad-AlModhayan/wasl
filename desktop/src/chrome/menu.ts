// The macOS menu bar's words and actions (#61, design D7). Rust builds the menu (src-tauri/src/menu.rs) and sends
// a click on one of Wasl's own items as `menu://action` with its id; the shell routes it. The shell also says which
// ids it handles now, and only those are enabled.

import { t } from '../i18n';

export const MENU_EVENT = 'menu://action';

/** Wasl's own menu items, in menu order. Same ids as `ACTIONS` in menu.rs. */
export const MENU_ACTIONS = [
  'settings',
  'new_pair',
  'toggle_sidebar',
  'toggle_inspector',
  'go_pairs',
  'go_views',
  'go_devices',
  'go_problems',
  'go_agents',
  'go_history',
  'go_bin',
  'sync_now',
  'toggle_pause',
  'show_in_finder',
] as const;

export type MenuAction = (typeof MENU_ACTIONS)[number];

/** Menu titles and the system items macOS draws, whose words Wasl passes in. Same keys as `WORDS` in menu.rs. */
export const MENU_WORDS = [
  'file',
  'edit',
  'view',
  'pair',
  'window',
  'help',
  'about',
  'services',
  'hide',
  'hide_others',
  'show_all',
  'quit',
  'close_window',
  'undo',
  'redo',
  'cut',
  'copy',
  'paste',
  'select_all',
  'full_screen',
  'minimize',
  'zoom',
  'bring_all_to_front',
] as const;

/** View → ⌘1–⌘7, in the design's order (Keyboard table). */
export const GO_ROUTES = {
  go_pairs: 'pairs',
  go_views: 'views',
  go_devices: 'devices',
  go_problems: 'problems',
  go_agents: 'agents',
  go_history: 'history',
  go_bin: 'bin',
} as const;

export interface MenuText {
  words: Record<string, string>;
  enabled: MenuAction[];
}

export function isMenuAction(id: unknown): id is MenuAction {
  return typeof id === 'string' && (MENU_ACTIONS as readonly string[]).includes(id);
}

/** Every word in the current language, and the handled ids in menu order. `words` replaces items' words that
 *  follow the window's state ("Show Sidebar", "Resume"). */
export function menuText(handled: Iterable<MenuAction>, words: Partial<Record<MenuAction, string>> = {}): MenuText {
  const all: Record<string, string> = {};
  for (const key of [...MENU_WORDS, ...MENU_ACTIONS]) all[key] = t(`menu.${key}`);
  Object.assign(all, words);
  const on = new Set(handled);
  return { words: all, enabled: MENU_ACTIONS.filter((a) => on.has(a)) };
}
