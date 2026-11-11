// New view: a Google account and a folder on Drive, where it appears on this computer (a new folder inside a
// chosen place), and how much space unopened files may take. A sheet over Views, never a page (D4, #65).

import { open } from '@tauri-apps/plugin-dialog';
import { api, isUiError } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { VIEW_LIMITS_GB, formatSpace, isolatePath, viewFolder } from '../format';
import { button, errorText, h, sheet, sheetRow, toast } from '../ui';
import { drivePicker } from './drivePicker';
import { createVaultIn, encryptChoice, vaultErrorText } from './vault';

const DEFAULT_LIMIT_GB = 20;

let open_ = false;

/** Opens the New view sheet; one at a time. */
export async function openViewSheet(app: App): Promise<void> {
  if (open_ || document.querySelector('dialog.sheet[open]')) return;
  open_ = true;
  try {
    await viewSheet(app);
  } finally {
    open_ = false;
  }
}

async function viewSheet(app: App): Promise<void> {
  const st = {
    parent: '',
    name: '',
    nameTouched: false,
    limit: DEFAULT_LIMIT_GB * 2 ** 30,
  };
  const nameInput = h('input', {
    id: 'view-name',
    type: 'text',
    autocomplete: 'off',
    dir: 'auto',
    oninput: () => {
      st.name = nameInput.value;
      st.nameTouched = true;
      drawPlace();
    },
  });
  const drive = await drivePicker(app, {
    allowCreate: false,
    onChosen: (f) => {
      if (f && !st.nameTouched) {
        const parts = f.path.split('/').filter(Boolean);
        st.name = parts[parts.length - 1] ?? '';
        nameInput.value = st.name;
        drawPlace();
      }
    },
  });

  const encrypt = encryptChoice('view-vault');
  /** The vault made on this screen, by Drive folder id: a retry after a failure doesn't make it twice. */
  const made = new Set<string>();
  const errors = h('div', { 'aria-live': 'polite' });
  // Re-drawn in parts, so the name field keeps its focus while typing.
  const placeLine = h('div', null);
  const preview = h('p', { class: 'kf-muted', 'aria-live': 'polite' });
  const placeBox = h(
    'div',
    { class: 'section-body' },
    placeLine,
    h('div', { class: 'kf-field' }, h('label', { class: 'kf-field-label', for: 'view-name' }, t('addview.name_label')), h('div', { class: 'kf-field-box' }, nameInput)),
    preview,
  );

  const drawPlace = () => {
    const target = viewFolder(st.parent, st.name);
    placeLine.replaceChildren(
      h('p', { class: st.parent ? 'path' : 'kf-muted', dir: 'auto' }, st.parent || t('addview.no_place')),
      button(st.parent ? t('addview.change_place') : t('addview.choose_place'), () => void choosePlace(), { icon: 'folder', key: 'choose-place' }),
    );
    preview.textContent = target ? t('addview.path_preview', { path: isolatePath(target) }) : '';
  };

  const choosePlace = async () => {
    const picked = await open({ directory: true, multiple: false }).catch(() => null);
    if (typeof picked === 'string' && picked) {
      st.parent = picked;
      drawPlace();
      document.querySelector<HTMLElement>('[data-key="choose-place"]')?.focus();
    }
  };

  const l = lang();
  const limit = h(
    'select',
    {
      id: 'view-limit',
      class: 'select',
      onchange: (e: Event) => {
        st.limit = Number((e.target as HTMLSelectElement).value);
      },
    },
    VIEW_LIMITS_GB.map((g) => h('option', { value: String(g * 2 ** 30), selected: g === DEFAULT_LIMIT_GB }, formatSpace(g * 2 ** 30, l))),
  );

  const submit = async (): Promise<boolean | undefined> => {
    const chosen = drive.chosen();
    const target = viewFolder(st.parent, st.name);
    const missing: string[] = [];
    if (!drive.accountId()) missing.push(t('add.missing_account'));
    if (!chosen) missing.push(t('add.missing_drive'));
    if (!st.parent) missing.push(t('addview.missing_place'));
    else if (!target) missing.push(t('addview.missing_name'));
    const passProblem = !chosen || !made.has(chosen.id) ? encrypt.problem() : null;
    if (passProblem) missing.push(passProblem);
    if (missing.length > 0 || !chosen || !target) {
      errors.replaceChildren(h('ul', { class: 'field-error plain-list', role: 'alert' }, missing.map((m) => h('li', null, m))));
      return undefined;
    }
    errors.replaceChildren();
    if (encrypt.wanted() && !made.has(chosen.id)) {
      try {
        const vault = await createVaultIn({ accountId: drive.accountId(), ...chosen }, encrypt.take());
        made.add(chosen.id);
        toast(t('vault.created', { name: vault.name }));
      } catch (e) {
        errors.replaceChildren(h('p', { class: 'field-error', role: 'alert' }, vaultErrorText(e)));
        return undefined;
      }
    }
    try {
      const view = await api.addView({
        name: st.name.trim(),
        localPath: target,
        accountId: drive.accountId(),
        driveFolderId: chosen.id,
        remotePath: chosen.path,
        driveId: chosen.driveId,
        cacheLimit: st.limit,
      });
      toast(
        view.last_error
          ? t('addview.added_not_yet', { name: view.name, message: view.last_error })
          : t('addview.added', { name: view.name }),
      );
      if (app.route.name === 'views') app.rerender();
      else app.go({ name: 'views' });
      return true;
    } catch (e) {
      // The daemon says why (a vault, a folder that isn't empty, one inside a pair): its words.
      const text = isUiError(e) && !['not_running', 'disconnected', 'untrusted', 'version'].includes(e.kind)
        ? t('common.error', { message: e.message })
        : errorText(e);
      errors.replaceChildren(h('p', { class: 'field-error', role: 'alert' }, text));
      return undefined;
    }
  };

  drawPlace();

  await sheet<boolean>({
    title: t('addview.title'),
    body: t('views.intro'),
    confirm: t('addview.create'),
    wide: true,
    cancelled: false,
    submit,
    content: [
      sheetRow(t('add.account_title'), drive.accountBox),
      sheetRow(t('pairsheet.drive'), drive.driveBox),
      sheetRow(t('pairsheet.encrypt'), encrypt.box),
      sheetRow(t('addview.where_title'), placeBox),
      sheetRow(t('views.limit_label'), h('div', { class: 'kf-field' }, h('div', { class: 'kf-field-box' }, limit)), { for: 'view-limit', hint: t('addview.space_help') }),
      errors,
    ],
  });
}
