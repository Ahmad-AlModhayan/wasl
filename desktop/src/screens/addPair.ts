// New pair and Edit pair (#65): one sheet over the window, on both platforms, never a pushed page
// (D4). New: a folder here, a Google account, a folder on Drive, a name, a direction, a vault and what to skip;
// then the first sync's preview. Edit: the same form prefilled; the name, the direction and the folders change
// (another folder starts the pair over, held for its preview).

import { open } from '@tauri-apps/plugin-dialog';
import { api } from '../api';
import type { App } from '../app';
import { t } from '../i18n';
import { baseName, isMirror, methodKey } from '../format';
import { confirmDialog, describeControls, fieldProblem, h, line, sheet, sheetRow, toast } from '../ui';
import type { PairDrive, PairEdit, PairState, SyncMode } from '../types';
import { drivePicker, type DrivePicker } from './drivePicker';
import { FOLDER_EDIT, MAIN_METHODS, MORE_METHODS, driveStep, editDeletes, editOf, encryptsByDefault, movesFolders, refusalFor, refusalOf, type SheetField } from './pairSheetModel';
import { createVaultIn, encryptChoice } from './vault';

/** Presets offered in the sheet (wasl-ignore ids), most common first. */
const PRESETS = ['node', 'python', 'rust', 'android-gradle', 'flutter', 'docker', 'editors', 'os-junk'];

export interface PairSheetOptions {
  /** A folder already chosen (a folder dropped on the window, #71). */
  local?: string;
  /** The pair to edit; without it the sheet makes a new one. */
  edit?: PairState;
}

let open_ = false;

/** Opens the New pair sheet, or Edit pair with `edit`. One at a time, and never over another dialog: ⌘N while
 *  one is open does nothing. */
export async function openPairSheet(app: App, opts: PairSheetOptions = {}): Promise<void> {
  if (open_ || document.querySelector('dialog[open]')) return;
  open_ = true;
  try {
    if (opts.edit) await editSheet(app, opts.edit);
    else await newSheet(app, opts.local ?? '');
  } finally {
    open_ = false;
  }
}

/** "Folder on this Mac": a sentence, so "this" stays lower case (#147 L10). */
function localTitle(): string {
  return t(document.documentElement.dataset.kfPlatform === 'mac' ? 'pairsheet.local_mac' : 'pairsheet.local_pc');
}

/** The direction radios: three first, the other four behind one line (open when one of them is chosen). */
function directions(initial: SyncMode, onChange: (m: SyncMode) => void): HTMLElement {
  const warning = h('p', { class: 'sheet-hint', 'aria-live': 'polite' });
  const warn = (m: SyncMode) => warning.replaceChildren(...(isMirror(m) ? [line('warning'), t('method.mirror_warning')] : []));
  const radio = (m: SyncMode) =>
    h(
      'label',
      { class: 'sheet-radio' },
      h('input', {
        type: 'radio',
        name: 'pair-method',
        value: m,
        checked: m === initial,
        onchange: () => {
          onChange(m);
          warn(m);
        },
      }),
      h('span', null, t(methodKey(m)), h('small', null, t(`${methodKey(m)}_desc`))),
    );
  const more = h('div', { class: 'sheet-radios', id: 'pair-more-methods', hidden: !MORE_METHODS.includes(initial) }, MORE_METHODS.map(radio));
  const toggle = h(
    'button',
    {
      type: 'button',
      class: 'sheet-more',
      'aria-expanded': String(!more.hidden),
      'aria-controls': 'pair-more-methods',
      onclick: () => {
        more.hidden = !more.hidden;
        toggle.setAttribute('aria-expanded', String(!more.hidden));
        toggle.textContent = t(more.hidden ? 'pairsheet.more_methods' : 'pairsheet.fewer_methods');
      },
    },
    t(more.hidden ? 'pairsheet.more_methods' : 'pairsheet.fewer_methods'),
  );
  warn(initial);
  return h(
    'fieldset',
    { class: 'sheet-radios' },
    h('legend', { class: 'kf-sr-only' }, t('pairsheet.direction')),
    MAIN_METHODS.map(radio),
    toggle,
    more,
    warning,
  );
}

/** The problem slots, one per field, plus one at the foot of the form. */
function problems(): Record<SheetField, ReturnType<typeof fieldProblem>> {
  return {
    local: fieldProblem('pair-local-problem'),
    drive: fieldProblem('pair-drive-problem'),
    name: fieldProblem('pair-name-problem'),
    vault: fieldProblem('pair-vault-problem'),
    form: fieldProblem('pair-form-problem'),
  };
}

function nameField(value: string, onInput: (v: string) => void): HTMLInputElement {
  return h('input', {
    id: 'pair-name',
    type: 'text',
    autocomplete: 'off',
    spellcheck: 'false',
    dir: 'auto',
    value,
    'aria-describedby': 'pair-name-problem',
    oninput: (e: Event) => onInput((e.target as HTMLInputElement).value),
  });
}

function pathBox(text: string, iconName: 'folder' | 'cloud', placeholder: string): HTMLElement {
  return h(
    'div',
    { class: `sheet-path${text ? ' is-set' : ''}` },
    line(iconName, { size: 16 }),
    h('span', { dir: text ? 'ltr' : 'auto' }, text || placeholder),
  );
}

/** Shows a refusal next to its field and moves focus there. */
function showRefusal(p: ReturnType<typeof problems>, e: unknown, focus: Partial<Record<SheetField, HTMLElement | null>>): void {
  const r = refusalOf(e);
  p[r.field].show(r.message !== undefined ? t('common.error', { message: r.message }) : t(r.key, r.vars));
  (focus[r.field] ?? null)?.focus();
}

async function newSheet(app: App, initialLocal: string): Promise<void> {
  const st = {
    local: initialLocal,
    name: initialLocal ? baseName(initialLocal) : '',
    nameTouched: false,
    method: 'TWO_WAY' as SyncMode,
    gitignore: false,
    hidden: false,
    presets: new Set<string>(['os-junk']),
  };
  const drive = await drivePicker(app, { allowCreate: true });
  const encrypt = encryptChoice('pair-vault');
  /** The vault made in this sheet, by Drive folder id: a retry after a refusal doesn't make it twice. */
  const made = new Map<string, string>();
  const p = problems();

  const localLine = h('div', { class: 'sheet-line' });
  const chooseBtn = h('button', { type: 'button', class: 'kf-desk-btn', 'aria-describedby': 'pair-local-problem', onclick: () => void chooseLocal() });
  const drawLocal = () => {
    chooseBtn.textContent = t(st.local ? 'add.change_folder' : 'add.choose_folder');
    localLine.replaceChildren(pathBox(st.local, 'folder', t('add.no_folder')), chooseBtn);
  };
  const nameInput = nameField(st.name, (v) => {
    st.name = v;
    st.nameTouched = true;
    p.name.show(null);
  });
  const chooseLocal = async () => {
    const picked = await open({ directory: true, multiple: false }).catch(() => null);
    if (typeof picked === 'string' && picked) {
      st.local = picked;
      p.local.show(null);
      if (!st.nameTouched) {
        st.name = baseName(picked);
        nameInput.value = st.name;
      }
      drawLocal();
      chooseBtn.focus();
    }
  };
  drawLocal();

  const check = (label: string, checked: boolean, onchange: (on: boolean) => void) => {
    const box = h('input', { type: 'checkbox', checked, onchange: () => onchange(box.checked) });
    return h('label', { class: 'sheet-radio' }, box, h('span', null, label));
  };
  const skip = h(
    'details',
    null,
    h('summary', null, t('add.rules_title')),
    h(
      'div',
      { class: 'sheet-radios' },
      h('p', { class: 'sheet-hint' }, t('add.rules_hint')),
      check(t('add.rules_gitignore'), st.gitignore, (on) => (st.gitignore = on)),
      check(t('add.rules_hidden'), st.hidden, (on) => (st.hidden = on)),
      PRESETS.map((id) =>
        check(t(`add.preset_${id.replace('-', '_')}`), st.presets.has(id), (on) => {
          if (on) st.presets.add(id);
          else st.presets.delete(id);
        }),
      ),
    ),
  );

  const refusalFocus = () => ({
    local: chooseBtn,
    drive: drive.driveBox.querySelector<HTMLElement>('button, input'),
    name: nameInput,
    vault: encrypt.box.querySelector<HTMLElement>('input[type="password"]'),
  });
  // A missing account or Drive folder and a passphrase refusal are announced from these controls.
  describeControls(drive.accountBox, p.drive.el);
  describeControls(drive.driveBox, p.drive.el);
  describeControls(encrypt.box, p.vault.el);
  const content = [
    sheetRow(localTitle(), localLine, { problem: p.local.el }),
    sheetRow(t('add.account_title'), drive.accountBox),
    sheetRow(t('pairsheet.drive'), drive.driveBox, { problem: p.drive.el }),
    sheetRow(t('add.name_label'), h('div', { class: 'kf-field' }, h('div', { class: 'kf-field-box' }, nameInput)), { for: 'pair-name', problem: p.name.el }),
    sheetRow(t('pairsheet.direction'), directions(st.method, (m) => (st.method = m))),
    sheetRow(t('pairsheet.encrypt'), encrypt.box, { problem: p.vault.el }),
    sheetRow(t('pairsheet.skip'), skip),
    p.form.el,
  ];

  const id = await sheet<string | null>({
    title: t('pairsheet.new_title'),
    body: t('pairsheet.new_body'),
    confirm: t('pairsheet.preview_first'),
    wide: true,
    content,
    cancelled: null,
    focus: initialLocal ? nameInput : chooseBtn,
    submit: async () => {
      for (const k of Object.keys(p) as SheetField[]) p[k].show(null);
      const chosen = drive.chosen();
      const missing: Array<[SheetField, string, HTMLElement | null]> = [];
      if (!st.local) missing.push(['local', 'add.missing_folder', chooseBtn]);
      if (!drive.accountId()) missing.push(['drive', 'add.missing_account', drive.accountBox.querySelector('button, select, input')]);
      else if (!chosen) missing.push(['drive', 'add.missing_drive', drive.driveBox.querySelector('button, input')]);
      if (!st.name.trim()) missing.push(['name', 'add.missing_name', nameInput]);
      const passProblem = !chosen || !made.has(chosen.id) ? encrypt.problem() : null;
      for (const [field, key] of missing) p[field].show(t(key));
      if (passProblem) p.vault.show(passProblem);
      if (missing.length > 0 || passProblem || !chosen) {
        (missing[0]?.[2] ?? encrypt.box.querySelector<HTMLElement>('input[type="password"]'))?.focus();
        return undefined;
      }
      let vaultId: string | null = made.get(chosen.id) ?? null;
      if (encrypt.wanted() && !vaultId) {
        try {
          const vault = await createVaultIn({ accountId: drive.accountId(), ...chosen }, encrypt.take());
          made.set(chosen.id, vault.id);
          vaultId = vault.id;
          toast(t('vault.created', { name: vault.name }));
        } catch (e) {
          showRefusal(p, e, refusalFocus());
          return undefined;
        }
      }
      try {
        return await api.createPair({
          name: st.name.trim(),
          localRoot: st.local,
          accountId: drive.accountId(),
          driveFolderId: chosen.id,
          remotePath: chosen.path,
          driveId: chosen.driveId,
          method: st.method,
          vaultId,
          gitignore: st.gitignore,
          presets: PRESETS.filter((x) => st.presets.has(x)),
          hidden: st.hidden,
        });
      } catch (e) {
        showRefusal(p, e, refusalFocus());
        return undefined;
      }
    },
  });
  if (id) app.go({ name: 'preview', id });
}

async function editSheet(app: App, s: PairState): Promise<void> {
  const pair = s.pair;
  const st = { name: pair.name, method: pair.method, local: pair.local_root };
  const p = problems();
  const nameInput = nameField(st.name, (v) => {
    st.name = v;
    p.name.show(null);
  });
  const dirs = directions(st.method, (m) => (st.method = m));

  // The folders (#103): another one resets the pair and holds it for a new preview. A Drive-less pair (devices
  // only) has just its local folder.
  const localLine = h('div', { class: 'sheet-line' });
  const localBtn = h('button', { type: 'button', class: 'kf-desk-btn', 'aria-describedby': 'pair-local-problem', onclick: () => void chooseLocal() }, t('add.change_folder'));
  const drawLocal = () => localLine.replaceChildren(pathBox(st.local, 'folder', ''), ...(FOLDER_EDIT ? [localBtn] : []));
  const chooseLocal = async () => {
    const picked = await open({ directory: true, multiple: false, defaultPath: st.local }).catch(() => null);
    if (typeof picked === 'string' && picked) {
      st.local = picked;
      p.local.show(null);
      drawLocal();
      localBtn.focus();
    }
  };
  drawLocal();

  /** The Drive picker and the vault choice, made the first time the person asks for another Drive folder. `drive`
   *  is the picker while it shows; "Keep current folder" hides it again and the pair keeps its Drive side. An
   *  encrypted pair keeps a vault in the new folder unless the person turns "Encrypt" off (and confirms). */
  let picker: DrivePicker | null = null;
  let drive: DrivePicker | null = null;
  const encrypt = encryptChoice('pair-vault', { checked: encryptsByDefault(pair) });
  const made = new Map<string, string>();
  const driveBody = h('div', { class: 'sheet-radios' });
  const vaultRow = sheetRow(t('pairsheet.encrypt'), encrypt.box, { problem: p.vault.el });
  vaultRow.hidden = true;
  const vaultOnRow = pair.vault_id ? sheetRow(t('pairsheet.encrypt'), h('p', { class: 'sheet-hint' }, t('pairsheet.vault_on'))) : null;
  const currentLine = h('div', { class: 'sheet-line' });
  const keepBtn = h('button', { type: 'button', class: 'kf-desk-btn', onclick: () => keepCurrent() }, t('pairsheet.keep_drive'));
  const driveBtn = h(
    'button',
    {
      type: 'button',
      class: 'kf-desk-btn',
      'aria-describedby': 'pair-drive-problem',
      onclick: async () => {
        driveBtn.disabled = true;
        if (!picker) {
          picker = await drivePicker(app, { allowCreate: true });
          describeControls(picker.accountBox, p.drive.el);
          describeControls(picker.driveBox, p.drive.el);
          describeControls(encrypt.box, p.vault.el);
        }
        drive = picker;
        driveBtn.disabled = false;
        // The current folder stays in view above the picker until another one is chosen.
        currentLine.replaceChildren(pathBox(pair.remote_path, 'cloud', ''), keepBtn);
        driveBody.replaceChildren(currentLine, picker.accountBox, picker.driveBox);
        vaultRow.hidden = false;
        if (vaultOnRow) vaultOnRow.hidden = true;
        picker.accountBox.querySelector<HTMLElement>('button, select, input')?.focus();
      },
    },
    t('pairsheet.change_drive'),
  );
  const drawCurrent = () => {
    currentLine.replaceChildren(pathBox(pair.remote_path, 'cloud', ''), ...(FOLDER_EDIT ? [driveBtn] : []));
    driveBody.replaceChildren(currentLine);
  };
  const keepCurrent = () => {
    drive = null;
    vaultRow.hidden = true;
    if (vaultOnRow) vaultOnRow.hidden = false;
    p.drive.show(null);
    p.vault.show(null);
    drawCurrent();
    driveBtn.focus();
  };
  const hasDrive = !!pair.drive_folder_id;
  drawCurrent();

  const note = FOLDER_EDIT
    ? h(
        'p',
        { class: 'sheet-note' },
        line('info'),
        h('span', null, t('pairsheet.folders_move'), pair.route === 'devices' ? ` ${t('pairsheet.folders_move_shared')}` : ''),
      )
    : h('p', { class: 'sheet-note' }, line('info'), t('pairsheet.folders_fixed'));
  const content = [
    sheetRow(t('add.name_label'), h('div', { class: 'kf-field' }, h('div', { class: 'kf-field-box' }, nameInput)), { for: 'pair-name', problem: p.name.el }),
    sheetRow(localTitle(), localLine, { problem: p.local.el }),
    hasDrive ? sheetRow(t('pairsheet.drive'), driveBody, { problem: p.drive.el }) : null,
    vaultOnRow,
    hasDrive ? vaultRow : null,
    note,
    sheetRow(t('pairsheet.direction'), dirs),
    p.form.el,
  ];
  const moved = await sheet<PairEdit | false>({
    title: t('pairsheet.edit_title'),
    body: t(FOLDER_EDIT ? 'pairsheet.edit_body_folders' : 'pairsheet.edit_body', { name: pair.name }),
    confirm: t('pairsheet.save'),
    wide: true,
    content,
    cancelled: false,
    focus: nameInput,
    submit: async () => {
      for (const k of Object.keys(p) as SheetField[]) p[k].show(null);
      if (!st.name.trim()) {
        p.name.show(t('add.missing_name'));
        nameInput.focus();
        return undefined;
      }
      // Another Drive folder, once one is chosen (the pair's own again is no move). Every question is asked before
      // anything is made on Drive: the vault's passphrase, dropping encryption, a deleting method.
      const chosen = drive?.chosen() ?? null;
      const accountId = drive?.accountId() ?? '';
      const checkbox = encrypt.box.querySelector<HTMLElement>('input[type="checkbox"]');
      const passField = encrypt.box.querySelector<HTMLElement>('input[type="password"]');
      const choice = (dropConfirmed: boolean) =>
        chosen ? { accountId, folderId: chosen.id, encrypt: encrypt.wanted(), made: made.has(chosen.id), dropConfirmed } : null;
      let step = driveStep(pair, choice(false));
      if (step.kind === 'confirm_drop') {
        const ok = await confirmDialog({
          title: t('pairsheet.drop_vault_title', { name: pair.name }),
          body: t('pairsheet.drop_vault_body', { name: pair.name, path: chosen?.path ?? '' }),
          confirm: t('pairsheet.drop_vault_action'),
          danger: true,
        });
        if (!ok) {
          checkbox?.focus();
          return undefined;
        }
        step = driveStep(pair, choice(true));
      }
      if (step.kind === 'move' && step.vault === 'new') {
        const passProblem = encrypt.problem();
        if (passProblem) {
          p.vault.show(passProblem);
          passField?.focus();
          return undefined;
        }
      }
      const driveOf = (vaultId: string | null): PairDrive | null =>
        step.kind === 'move' && chosen ? { accountId, driveFolderId: chosen.id, remotePath: chosen.path, driveId: chosen.driveId, vaultId } : null;
      const planned = editOf(pair, { name: st.name, method: st.method, local: st.local, drive: driveOf(null) });
      if (!planned) return false;
      if (editDeletes(pair.method, planned.method)) {
        const method = t(methodKey(planned.method));
        const ok = await confirmDialog({
          title: t('pairsheet.confirm_delete_title', { name: pair.name, method }),
          body: t(`pairsheet.confirm_delete_${planned.method.toLowerCase()}`),
          confirm: t('pairsheet.confirm_delete_action', { method }),
          danger: true,
        });
        if (!ok) {
          dirs.querySelector<HTMLElement>('input[name="pair-method"]:checked')?.focus();
          return undefined;
        }
      }
      // The vault: none, the one an earlier Save made here, or a new one made now.
      let vaultId: string | null = null;
      if (step.kind === 'move' && step.vault !== 'none' && chosen) {
        vaultId = made.get(chosen.id) ?? null;
        if (!vaultId) {
          try {
            const vault = await createVaultIn({ accountId, ...chosen }, encrypt.take());
            made.set(chosen.id, vault.id);
            vaultId = vault.id;
            toast(t('vault.created', { name: vault.name }));
          } catch (e) {
            showRefusal(p, e, { drive: drive?.driveBox.querySelector<HTMLElement>('button, input') ?? null, vault: passField });
            return undefined;
          }
        }
      }
      const edit = editOf(pair, { name: st.name, method: st.method, local: st.local, drive: driveOf(vaultId) }) ?? planned;
      try {
        await api.updatePair(pair.id, edit);
        toast(t(movesFolders(edit) ? 'pairsheet.saved_moved' : 'pairsheet.saved', { name: edit.name }));
        app.rerender();
        return movesFolders(edit) ? edit : false;
      } catch (e) {
        const r = refusalFor(e, edit);
        p[r.field].show(r.message !== undefined ? t('common.error', { message: r.message }) : t(r.key, r.vars));
        // A vault made for this Save stays on Drive; the next Save uses it.
        if (vaultId && r.field !== 'vault' && chosen) p.vault.show(t('pairsheet.vault_kept', { path: chosen.path }));
        const focus: Partial<Record<SheetField, HTMLElement | null>> = {
          name: nameInput,
          local: FOLDER_EDIT ? localBtn : null,
          drive: drive?.driveBox.querySelector<HTMLElement>('button, input') ?? (driveBtn.isConnected ? driveBtn : null),
          vault: passField,
          form: FOLDER_EDIT ? localBtn : null,
        };
        (focus[r.field] ?? null)?.focus();
        return undefined;
      }
    },
  });
  // A moved pair starts over: its preview, waiting for approval.
  if (moved) app.go({ name: 'preview', id: pair.id });
}
