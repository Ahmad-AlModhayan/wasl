// Choosing a Google account and a folder on Drive, by browsing or by typing its path. Shared by Add folder
// pair and Add view; each section re-draws on its own.

import { api, isUiError } from '../api';
import type { App } from '../app';
import { t } from '../i18n';
import { button, errorText, fill, h, line } from '../ui';
import type { Account, DriveFolder } from '../types';

export interface ChosenFolder {
  id: string;
  /** "My Drive/Photos/2026". */
  path: string;
  driveId: string | null;
}

export interface DrivePicker {
  accountBox: HTMLElement;
  driveBox: HTMLElement;
  /** The account chosen ('' when none is signed in). */
  accountId(): string;
  chosen(): ChosenFolder | null;
}

/**
 * The account and Drive folder sections. `allowCreate` offers "create it when missing" for a typed path
 * (a pair may start from an empty Drive folder; a view shows one that exists). `onChosen` runs whenever the
 * folder changes.
 */
export async function drivePicker(
  app: App,
  opts: { allowCreate: boolean; onChosen?: (f: ChosenFolder | null) => void },
): Promise<DrivePicker> {
  let accounts: Account[] = await api.accounts().catch(() => app.status?.accounts ?? []);
  const st = {
    accountId: accounts[0]?.id ?? '',
    chosen: null as ChosenFolder | null,
    driveMode: 'browse' as 'browse' | 'path',
    crumbs: [{ id: 'root', name: t('add.my_drive'), driveId: null as string | null }],
    waiting: false,
    signinUrl: '',
    accountError: '',
  };
  const accountBox = h('div', { class: 'section-body' });
  const driveBox = h('div', { class: 'section-body' });

  const choose = (f: ChosenFolder | null) => {
    st.chosen = f;
    opts.onChosen?.(f);
  };

  const drawAccount = () => {
    const select =
      accounts.length > 0
        ? h(
            'div',
            { class: 'kf-field' },
            // The sheet's row already says Google account: the label stays for screen readers only (#147 L10).
            h('label', { class: 'kf-field-label kf-sr-only', for: 'pair-account' }, t('add.account_label')),
            h(
              'div',
              { class: 'kf-field-box' },
              h(
                'select',
                {
                  id: 'pair-account',
                  class: 'select',
                  onchange: (e: Event) => {
                    st.accountId = (e.target as HTMLSelectElement).value;
                    choose(null);
                    st.crumbs = st.crumbs.slice(0, 1);
                    void drawDrive();
                  },
                },
                accounts.map((a) => h('option', { value: a.id, selected: a.id === st.accountId }, a.email)),
              ),
            ),
          )
        : null;
    fill(accountBox,
      select ?? h('p', { class: 'kf-muted' }, t('settings.no_accounts')),
      button(t('add.add_account'), () => void addAccount(), { icon: 'user', key: 'add-account', disabled: st.waiting }),
      st.waiting
        ? h(
            'div',
            { class: 'waiting', role: 'status' },
            h('p', null, h('span', { class: 'spinner', 'aria-hidden': 'true' }), t('add.waiting_google')),
            st.signinUrl
              ? h('label', { class: 'kf-field' }, h('span', { class: 'kf-field-help' }, t('add.signin_link')),
                  h('span', { class: 'kf-field-box' }, h('input', { type: 'text', readonly: true, value: st.signinUrl, dir: 'ltr' })))
              : null,
          )
        : null,
      st.accountError ? h('p', { class: 'field-error', role: 'alert' }, st.accountError) : null,
    );
  };

  const addAccount = async () => {
    st.waiting = true;
    st.accountError = '';
    drawAccount();
    try {
      st.signinUrl = await api.addAccount();
    } catch (e) {
      st.waiting = false;
      st.accountError = errorText(e);
    }
    drawAccount();
  };

  app.onEvent((e) => {
    if (e.event === 'account_added') {
      void api.accounts().then((list) => {
        accounts = list;
        // Another account: a folder chosen in the last one no longer applies.
        if (st.accountId !== e.account.id) {
          choose(null);
          st.crumbs = st.crumbs.slice(0, 1);
        }
        st.accountId = e.account.id;
        st.waiting = false;
        st.signinUrl = '';
        drawAccount();
        void drawDrive();
      });
    } else if (e.event === 'account_failed' && st.waiting) {
      st.waiting = false;
      st.accountError = t('add.account_failed', { message: e.message });
      drawAccount();
    }
  });

  const drawDrive = async () => {
    const seg = h(
      'div',
      { class: 'kf-seg drive-mode', role: 'radiogroup', 'aria-label': t('add.drive_title') },
      (['browse', 'path'] as const).map((m) =>
        h(
          'button',
          {
            type: 'button',
            role: 'radio',
            class: 'kf-seg-item',
            'aria-checked': String(st.driveMode === m),
            'data-key': `drive-mode-${m}`,
            onclick: () => {
              st.driveMode = m;
              void drawDrive();
            },
          },
          t(m === 'browse' ? 'add.browse' : 'add.type_path'),
        ),
      ),
    );
    const chosen = st.chosen ? h('p', { class: 'chosen', role: 'status' }, line('check'), t('add.chosen', { path: st.chosen.path })) : null;
    if (!st.accountId) {
      driveBox.replaceChildren(h('p', { class: 'kf-muted' }, t('add.missing_account')));
      return;
    }
    if (st.driveMode === 'path') {
      const input = h('input', { id: 'drive-path', type: 'text', dir: 'ltr', placeholder: t('add.path_hint'), value: st.chosen?.path ?? '' });
      const create = opts.allowCreate ? h('input', { id: 'drive-create', type: 'checkbox' }) : null;
      const status = h('p', { 'aria-live': 'polite' });
      fill(driveBox,
        seg,
        h('div', { class: 'kf-field' },
          h('label', { class: 'kf-field-label', for: 'drive-path' }, t('add.path_label')),
          h('div', { class: 'kf-field-box' }, input),
          h('p', { class: 'kf-field-help' }, t('add.path_hint'))),
        create ? h('label', { class: 'check-row' }, create, t('add.create_missing')) : null,
        button(t('add.find'), async () => {
          status.replaceChildren();
          try {
            const f = await api.resolveDrivePath(st.accountId, input.value.trim(), create?.checked ?? false);
            choose({ id: f.id, path: input.value.trim(), driveId: f.drive_id });
            void drawDrive();
          } catch (e) {
            if (st.chosen) {
              choose(null);
              chosen?.remove();
            }
            status.className = 'field-error';
            status.textContent = isUiError(e) && e.kind === 'not_found' ? t('add.path_not_found') : errorText(e);
          }
        }, { icon: 'folder', key: 'drive-find' }),
        status,
        chosen,
      );
      return;
    }
    const here = st.crumbs[st.crumbs.length - 1];
    const list = h('ul', { class: 'kf-group folder-list', 'aria-busy': 'true' }, h('li', { class: 'kf-row kf-muted' }, t('common.loading')));
    fill(driveBox,
      seg,
      h(
        'nav',
        { class: 'crumbs', 'aria-label': t('add.drive_title') },
        h('ol', null, st.crumbs.map((c, i) =>
          h('li', null,
            i < st.crumbs.length - 1
              ? h('button', { type: 'button', class: 'crumb', 'data-key': `crumb-${i}`, onclick: () => { st.crumbs = st.crumbs.slice(0, i + 1); void drawDrive(); } }, c.name)
              : h('span', { 'aria-current': 'location' }, c.name),
            i < st.crumbs.length - 1 ? line('forward', { mirror: true }) : null),
        )),
      ),
      list,
      h('div', { class: 'row-actions' },
        st.crumbs.length > 1 ? button(t('add.up'), () => { st.crumbs = st.crumbs.slice(0, -1); void drawDrive(); }, { icon: 'back', mirror: true, key: 'drive-up' }) : null,
        button(t('add.use_this'), () => {
          choose({ id: here.id, path: st.crumbs.map((c) => c.name).join('/'), driveId: here.driveId });
          void drawDrive();
        }, { kind: 'tinted', icon: 'check', key: 'drive-use' })),
      chosen,
    );
    let folders: DriveFolder[];
    try {
      folders = await api.driveFolders(st.accountId, here.id);
    } catch (e) {
      list.replaceChildren(h('li', { class: 'kf-row field-error', role: 'alert' }, errorText(e)));
      list.removeAttribute('aria-busy');
      return;
    }
    list.removeAttribute('aria-busy');
    list.replaceChildren(
      ...(folders.length === 0
        ? [h('li', { class: 'kf-row kf-muted' }, t('add.no_subfolders'))]
        : folders.map((f) =>
            h('li', null, h('button', {
              type: 'button',
              class: 'kf-row folder-row',
              'data-key': `folder-${f.id}`,
              'aria-label': t('add.open_folder', { name: f.name }),
              onclick: () => {
                st.crumbs = [...st.crumbs, { id: f.id, name: f.name, driveId: f.drive_id }];
                void drawDrive().then(() => driveBox.querySelector<HTMLElement>('.folder-row, [data-key="drive-use"]')?.focus());
              },
            }, line('folder'), h('span', { class: 'kf-row-title', dir: 'auto' }, f.name), line('forward', { mirror: true })))
          )),
    );
  };

  drawAccount();
  void drawDrive();
  return { accountBox, driveBox, accountId: () => st.accountId, chosen: () => st.chosen };
}
