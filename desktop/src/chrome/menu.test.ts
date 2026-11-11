import rust from '../../src-tauri/src/menu.rs?raw';
import { describe, expect, it } from 'vitest';
import { DICTS, setLang } from '../i18n';
import { GO_ROUTES, MENU_ACTIONS, MENU_WORDS, isMenuAction, menuText } from './menu';

/** The first string of each tuple in a Rust `const NAME: &[(...)] = &[ ... ];` table. */
function rustIds(name: string): string[] {
  const table = rust.slice(rust.indexOf(`const ${name}:`), rust.indexOf('];', rust.indexOf(`const ${name}:`)));
  return [...table.matchAll(/^\s*\("([a-z_]+)",/gm)].map((m) => m[1]);
}

describe('the menu bar', () => {
  it('has the same action ids as menu.rs, in menu order', () => {
    expect(rustIds('ACTIONS')).toEqual([...MENU_ACTIONS]);
  });

  it('has the same word keys as menu.rs', () => {
    expect(rustIds('WORDS')).toEqual([...MENU_WORDS]);
  });

  it('has every word in English and Arabic', () => {
    for (const key of [...MENU_WORDS, ...MENU_ACTIONS]) {
      expect(DICTS.en[`menu.${key}`], key).toBeTruthy();
      expect(DICTS.ar[`menu.${key}`], key).toBeTruthy();
    }
  });

  it('sends the words in the current language', () => {
    setLang('ar');
    expect(menuText([]).words.quit).toBe('إنهاء وصل');
    setLang('en');
    expect(menuText([]).words.go_pairs).toBe('All Pairs');
  });

  it('enables only the handled actions, in menu order', () => {
    expect(menuText(['sync_now', 'settings', 'go_bin']).enabled).toEqual(['settings', 'go_bin', 'sync_now']);
  });

  it('takes the words that follow the window’s state', () => {
    setLang('en');
    const text = menuText(['toggle_pause'], { toggle_pause: 'Resume', toggle_sidebar: 'Show Sidebar' });
    expect(text.words.toggle_pause).toBe('Resume');
    expect(text.words.toggle_sidebar).toBe('Show Sidebar');
    expect(text.words.sync_now).toBe('Sync Now');
  });

  it('routes ⌘1–⌘7 to the seven sections in the design order', () => {
    expect(Object.values(GO_ROUTES)).toEqual(['pairs', 'views', 'devices', 'problems', 'agents', 'history', 'bin']);
  });

  it('knows its own ids and nothing else', () => {
    expect(isMenuAction('new_pair')).toBe(true);
    expect(isMenuAction('quit')).toBe(false);
    expect(isMenuAction(7)).toBe(false);
  });
});
