// The commands above the detail pane (#63, design D1/D7): a 52px toolbar with the sidebar toggle and the title
// leading, the commands as icon buttons in a capsule, then search, then the inspector toggle last. The same on both
// platforms (#124); only the frame differs: on macOS it is the window's top strip under the traffic lights, on
// Windows it sits under the title bar, which holds the search field (#62). Every command here is also in the macOS
// menu bar (#61).

import { t } from '../i18n';
import type { LineName } from '../icons';
import { fill, h, line } from '../ui';
import type { App } from '../app';
import { shortcutLabel, type ShellAction } from './keys';

/** A toolbar command. Later screens pass their own through `app.setCommands`. */
export interface Command {
  id: string;
  label: string;
  icon: LineName;
  run: () => void;
  /** The one filled command. */
  primary?: boolean;
  disabled?: boolean;
  /** Shows the shortcut in the tooltip. */
  shortcut?: ShellAction;
}

function isMac(): boolean {
  return document.documentElement.dataset.kfPlatform === 'mac';
}

function tip(c: { label: string; shortcut?: ShellAction }): string {
  const key = c.shortcut ? shortcutLabel(c.shortcut, isMac() ? 'mac' : 'windows') : null;
  return key ? `${c.label} (${key})` : c.label;
}

/** A command shows its icon only; its name is the label, and the tooltip adds the shortcut. */
function commandButton(c: Command): HTMLButtonElement {
  return h(
    'button',
    {
      type: 'button',
      class: `kf-desk-btn kf-desk-icon-btn shell-cmd${c.primary ? ' kf-desk-btn-default' : ''}`,
      'data-key': `cmd-${c.id}`,
      'aria-label': c.label,
      title: tip(c),
      disabled: c.disabled,
      onclick: () => c.run(),
    },
    line(c.icon),
  );
}

/** A toggle keeps its name ("Inspector") and says whether it is on with `aria-pressed`, so its name never says the
 *  opposite of its state. */
function toggleButton(opts: { key: string; icon: LineName; label: string; pressed: boolean; shortcut?: ShellAction; mirror?: boolean; run: () => void }): HTMLButtonElement {
  return h(
    'button',
    {
      type: 'button',
      class: 'kf-desk-btn shell-cmd kf-desk-icon-btn',
      'data-key': opts.key,
      'aria-label': opts.label,
      'aria-pressed': String(opts.pressed),
      title: tip(opts),
      onclick: opts.run,
    },
    line(opts.icon, { mirror: opts.mirror }),
  );
}

/** The search field: filters the pairs in the sidebar; later screens filter their own lists through `app.query`. */
export function searchField(app: App): HTMLElement {
  const input = h('input', {
    type: 'search',
    id: 'shell-search',
    class: 'shell-search-input',
    placeholder: t('shell.search'),
    'aria-label': t('shell.search_label'),
    autocomplete: 'off',
    spellcheck: 'false',
    value: app.query,
    oninput: (e: Event) => app.setQuery((e.target as HTMLInputElement).value),
    onkeydown: (e: KeyboardEvent) => {
      if (e.key === 'Escape' && (e.target as HTMLInputElement).value) {
        e.stopPropagation();
        (e.target as HTMLInputElement).value = '';
        app.setQuery('');
      }
    },
  });
  return h('div', { class: 'shell-search', role: 'search' }, line('search'), input);
}

/** Each toolbar's search field, made once: live updates rebuild the toolbar around it, so its caret and an IME
 *  composition (Arabic) aren't cut. A new language builds a new toolbar, and a new field with its words. */
const searchFields = new WeakMap<HTMLElement, HTMLElement>();

function keptSearch(app: App, toolbar: HTMLElement): HTMLElement {
  let box = searchFields.get(toolbar);
  if (!box) {
    box = searchField(app);
    searchFields.set(toolbar, box);
  }
  const input = box.querySelector('input')!;
  if (document.activeElement !== input && input.value !== app.query) input.value = app.query;
  return box;
}

/** Puts `before`, `kept` and `after` in `el`, leaving `kept` in place (not moved, so not blurred) when it is there. */
function fillAround(el: HTMLElement, before: Array<Node | null>, kept: HTMLElement, after: Array<Node | null>): void {
  if (kept.parentElement !== el) {
    fill(el, before, kept, after);
    return;
  }
  for (const n of [...el.childNodes]) if (n !== kept) n.remove();
  kept.before(...before.filter((n): n is Node => n !== null));
  kept.after(...after.filter((n): n is Node => n !== null));
}

export function renderToolbar(app: App, el: HTMLElement): void {
  const { title, subtitle } = app.titles();
  const commands = app.commands();
  const mac = isMac();
  el.className = 'shell-toolbar';
  // macOS: the toolbar is the window's top strip, so it moves the window. Windows: the title bar above it does.
  if (mac) el.setAttribute('data-tauri-drag-region', '');
  else el.removeAttribute('data-tauri-drag-region');
  const drag = mac ? '' : undefined;
  const before = [
    toggleButton({
      key: 'cmd-sidebar',
      icon: 'sidebar',
      label: t('shell.sidebar'),
      pressed: app.sidebarVisible(),
      shortcut: 'toggle_sidebar',
      mirror: true,
      run: () => app.toggleSidebar(),
    }),
    h(
      'div',
      { class: 'shell-titles', 'data-tauri-drag-region': drag },
      h('p', { class: 'shell-title', 'data-tauri-drag-region': drag }, title),
      subtitle ? h('p', { class: 'shell-subtitle', 'data-tauri-drag-region': drag }, subtitle) : null,
    ),
    commands.length ? h('div', { class: 'shell-capsule', role: 'toolbar', 'aria-label': t('shell.commands') }, commands.map(commandButton)) : null,
  ];
  // Settings is a page of its own (D5) and never shows the inspector, so it has no toggle for it.
  const inspector = app.route.name === 'settings'
    ? null
    : h(
        'div',
        { class: 'shell-capsule' },
        toggleButton({
          key: 'cmd-inspector',
          icon: 'inspector',
          label: t('shell.inspector'),
          pressed: app.inspectorOpen,
          shortcut: 'toggle_inspector',
          mirror: true,
          run: () => app.toggleInspector(),
        }),
      );
  // Windows: the search field is in the title bar (#62), part of the frame.
  if (mac) fillAround(el, before, keptSearch(app, el), [inspector]);
  else fill(el, before, inspector);
}
