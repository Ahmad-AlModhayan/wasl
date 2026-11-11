// Context menus (#63, design D7): native menus through Tauri (`Menu.popup()`, allowed by `core:default`'s
// `core:menu:default`), opened by a right click, a Ctrl-click on macOS (the webview reports it as a right click),
// Shift+F10 and the Menu key. In a plain browser (the dev mock) the shell draws the menu itself with kahf-design's
// `.kf-desk-menu`. Later screens add their own menus through `bindContextMenu`.

import { LogicalPosition } from '@tauri-apps/api/dpi';
import { Menu } from '@tauri-apps/api/menu';
import { h, line } from '../ui';
import type { LineName } from '../icons';
import type { Platform } from '../platform';
import { accelerator, shortcutLabel, type ShellAction } from './keys';

export interface MenuEntry {
  text: string;
  run: () => void;
  icon?: LineName;
  /** Shows the action's shortcut beside it. */
  shortcut?: ShellAction;
  disabled?: boolean;
  /** Removes or deletes something: red text, never filled (design). */
  danger?: boolean;
}

export type MenuLine = MenuEntry | 'separator';

function platform(): Platform {
  return document.documentElement.dataset.kfPlatform === 'mac' ? 'mac' : 'windows';
}

/** A real Tauri window, not the dev mock standing in for one. */
function nativeMenus(): boolean {
  return '__TAURI_INTERNALS__' in window && !('__WASL_MOCK__' in window);
}

/** The last native menu shown. Its resource is closed when the next one opens: its items' actions may still be on
 *  their way when `popup` returns, so it isn't closed right after. */
let shown: Menu | null = null;

/** Opens a menu at a point in the window (CSS pixels); `opener` gets focus back when it closes. */
export async function popupMenu(lines: MenuLine[], at: { x: number; y: number }, opener: HTMLElement): Promise<void> {
  if (nativeMenus()) {
    const p = platform();
    const previous = shown;
    shown = null;
    await previous?.close().catch(() => undefined);
    const menu = await Menu.new({
      items: lines.map((l) =>
        l === 'separator'
          ? { item: 'Separator' as const }
          : { text: l.text, enabled: !l.disabled, accelerator: l.shortcut ? accelerator(l.shortcut, p) : undefined, action: () => l.run() },
      ),
    });
    shown = menu;
    await menu.popup(new LogicalPosition(at.x, at.y));
    return;
  }
  htmlMenu(lines, at, opener);
}

let lastKeyboardOpen = 0;

/**
 * Gives `el` a context menu. `lines` is read when the menu opens, so it reflects the state at that moment.
 * Keyboard openers place the menu at the element's start corner.
 */
export function bindContextMenu(el: HTMLElement, lines: () => MenuLine[]): void {
  const atElement = (): { x: number; y: number } => {
    const r = el.getBoundingClientRect();
    const rtl = document.documentElement.dir === 'rtl';
    return { x: rtl ? r.right - 8 : r.left + 8, y: r.bottom - 4 };
  };
  el.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    // Chromium also turns Shift+F10 and the Menu key into this event; the keydown below already opened it.
    if (Date.now() - lastKeyboardOpen < 400) return;
    const fromKeyboard = e.clientX === 0 && e.clientY === 0;
    void popupMenu(lines(), fromKeyboard ? atElement() : { x: e.clientX, y: e.clientY }, el);
  });
  el.addEventListener('keydown', (e) => {
    if ((e.key === 'F10' && e.shiftKey) || e.key === 'ContextMenu') {
      e.preventDefault();
      e.stopPropagation();
      lastKeyboardOpen = Date.now();
      void popupMenu(lines(), atElement(), el);
    }
  });
}

// ---------------------------------------------------------------- the browser's stand-in

let open: { close: (refocus: boolean) => void } | null = null;

function htmlMenu(lines: MenuLine[], at: { x: number; y: number }, opener: HTMLElement): void {
  open?.close(false);
  const p = platform();
  const items: HTMLButtonElement[] = [];
  const menu = h(
    'div',
    { class: 'kf-desk-menu shell-menu', role: 'menu', 'aria-orientation': 'vertical' },
    lines.map((l) => {
      if (l === 'separator') return h('div', { role: 'separator' });
      const key = l.shortcut ? shortcutLabel(l.shortcut, p) : null;
      const b = h(
        'button',
        {
          type: 'button',
          role: 'menuitem',
          class: l.danger ? 'shell-menu-danger' : undefined,
          disabled: l.disabled,
          tabindex: '-1',
          onclick: () => {
            close(true);
            l.run();
          },
        },
        l.icon ? line(l.icon) : h('span', { class: 'shell-menu-gap', 'aria-hidden': 'true' }),
        h('span', { class: 'shell-menu-text' }, l.text),
        key ? h('kbd', { class: 'shell-menu-key' }, key) : null,
      );
      items.push(b);
      return b;
    }),
  );
  const enabled = () => items.filter((b) => !b.disabled);
  const move = (by: number | 'first' | 'last') => {
    const list = enabled();
    if (!list.length) return;
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    const next = by === 'first' ? 0 : by === 'last' ? list.length - 1 : (i + by + list.length) % list.length;
    list[next]!.focus();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') close(true);
    else if (e.key === 'ArrowDown') move(1);
    else if (e.key === 'ArrowUp') move(-1);
    else if (e.key === 'Home') move('first');
    else if (e.key === 'End') move('last');
    else if (e.key === 'Tab') close(true);
    else return;
    e.preventDefault();
    e.stopPropagation();
  };
  const onDown = (e: MouseEvent) => {
    if (!menu.contains(e.target as Node)) close(false);
  };
  function close(refocus: boolean): void {
    if (!menu.isConnected) return;
    menu.remove();
    document.removeEventListener('keydown', onKey, true);
    document.removeEventListener('mousedown', onDown, true);
    window.removeEventListener('blur', onBlur);
    open = null;
    if (refocus) opener.focus();
  }
  const onBlur = () => close(false);
  document.body.append(menu);
  // Inside the window, flipped back from the edges it would cross.
  const r = menu.getBoundingClientRect();
  // Right to left, the menu hangs from its right edge, as native menus do.
  const startX = document.documentElement.dir === 'rtl' ? at.x - r.width : at.x;
  const x = Math.min(startX, window.innerWidth - r.width - 4);
  const y = at.y + r.height > window.innerHeight - 4 ? Math.max(4, at.y - r.height) : at.y;
  menu.style.left = `${Math.max(4, x)}px`;
  menu.style.top = `${y}px`;
  document.addEventListener('keydown', onKey, true);
  document.addEventListener('mousedown', onDown, true);
  window.addEventListener('blur', onBlur);
  open = { close };
  move('first');
}
