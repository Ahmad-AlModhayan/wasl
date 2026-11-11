// Encrypted vaults in the window: unlocking one from a pair's or a view's card, and "Encrypt this folder
// (vault)" when adding a pair or a view. A passphrase lives only in its password field until the one call
// that needs it, and the field is emptied right after; nothing here keeps or logs it.

import { api, isUiError } from '../api';
import { t } from '../i18n';
import { button, errorText, h, line, toast } from '../ui';
import type { VaultInfo } from '../types';

/** The shortest passphrase a new vault takes (as on the phone and in the service). */
export const MIN_PASSPHRASE = 8;

/** Why a new vault's passphrase can't be used (a string key), or null. */
export function newPassphraseProblem(passphrase: string, repeat: string): string | null {
  if ([...passphrase].length < MIN_PASSPHRASE) return 'vault.too_short';
  if (passphrase !== repeat) return 'vault.mismatch';
  return null;
}

/** The service's answer to a wrong passphrase. */
export function isWrongPassphrase(e: unknown): boolean {
  return isUiError(e) && e.kind === 'invalid' && e.message.startsWith('Wrong passphrase');
}

/** Words for an error from a vault command: a wrong passphrase in the window's language, the service's own
 *  words for what it refused (a folder that isn't empty) or couldn't check (Drive didn't answer in time: the
 *  usual "check the internet connection" would send people looking for a fault that isn't there), the usual
 *  words otherwise. */
export function vaultErrorText(e: unknown): string {
  if (isWrongPassphrase(e)) return t('vault.wrong');
  if (isUiError(e) && (e.kind === 'invalid' || e.kind === 'network') && e.message) {
    return t('common.error', { message: e.message });
  }
  return errorText(e);
}

/** The vaults by id, or none when the service can't say. */
export async function vaultsById(): Promise<Map<string, VaultInfo>> {
  const list = await api.vaults().catch(() => [] as VaultInfo[]);
  return new Map(list.map((v) => [v.id, v]));
}

/** Asks for the vault's passphrase and unlocks it on this computer. Resolves true once it is unlocked. */
export function unlockDialog(vault: { id: string; name: string }): Promise<boolean> {
  const before = document.activeElement as HTMLElement | null;
  return new Promise((resolve) => {
    const id = `dlg-${Math.random().toString(36).slice(2)}`;
    const dlg = h('dialog', { class: 'dialog', 'aria-labelledby': `${id}-title`, 'aria-describedby': `${id}-body` });
    const input = h('input', {
      id: `${id}-pass`,
      type: 'password',
      autocomplete: 'off',
      spellcheck: 'false',
      dir: 'auto',
      'aria-describedby': `${id}-status`,
    });
    const status = h('p', { id: `${id}-status`, class: 'kf-field-help', 'aria-live': 'polite' });
    const field = h(
      'div',
      { class: 'kf-field' },
      h('label', { class: 'kf-field-label', for: `${id}-pass` }, t('vault.passphrase')),
      h('div', { class: 'kf-field-box' }, input),
      status,
    );
    const submit = h('button', { type: 'submit', class: 'kf-btn kf-btn-sm kf-btn-filled' }, t('vault.unlock'));
    let busy = false;
    const done = (ok: boolean) => {
      input.value = '';
      dlg.close();
      dlg.remove();
      before?.focus();
      resolve(ok);
    };
    dlg.addEventListener('cancel', (e) => {
      e.preventDefault();
      if (!busy) done(false);
    });
    input.addEventListener('input', () => {
      field.classList.remove('kf-field-error');
      status.replaceChildren();
    });
    const form = h(
      'form',
      {
        class: 'dialog-form',
        onsubmit: (e: Event) => {
          e.preventDefault();
          if (busy) return;
          if (!input.value) {
            input.focus();
            return;
          }
          busy = true;
          submit.disabled = true;
          dlg.setAttribute('aria-busy', 'true');
          status.replaceChildren(h('span', { class: 'spinner', 'aria-hidden': 'true' }), t('vault.checking'));
          // The value goes to the one call and the field is emptied, whatever the answer.
          const attempt = api.unlockVault(vault.id, input.value);
          input.value = '';
          void attempt
            .then(() => {
              toast(t('vault.unlocked', { name: vault.name }));
              done(true);
            })
            .catch((err: unknown) => {
              busy = false;
              submit.disabled = false;
              dlg.removeAttribute('aria-busy');
              field.classList.add('kf-field-error');
              status.replaceChildren(line('failed'), vaultErrorText(err));
              input.focus();
            });
        },
      },
      h('h2', { id: `${id}-title`, class: 'dialog-title' }, t('vault.unlock_title', { name: vault.name })),
      h('p', { id: `${id}-body`, class: 'dialog-body' }, t('vault.unlock_body')),
      field,
      h('div', { class: 'dialog-actions' }, button(t('common.cancel'), () => !busy && done(false), { kind: 'gray' }), submit),
    );
    dlg.appendChild(form);
    document.body.appendChild(dlg);
    dlg.showModal();
    input.focus();
  });
}

/** "Encrypt this folder (vault)" on the Add pair and Add view screens. */
export interface EncryptChoice {
  box: HTMLElement;
  /** Whether a new vault is wanted. */
  wanted(): boolean;
  /** Why the passphrase fields can't be used (in words), or null. */
  problem(): string | null;
  /** The passphrase, read once: both fields are emptied. */
  take(): string;
}

/** `checked` ticks "Encrypt" from the start (an encrypted pair moving to another Drive folder keeps a vault). */
export function encryptChoice(prefix: string, opts: { checked?: boolean } = {}): EncryptChoice {
  const check = h('input', { id: `${prefix}-encrypt`, type: 'checkbox', checked: !!opts.checked });
  const pass = h('input', { id: `${prefix}-pass`, type: 'password', autocomplete: 'new-password', spellcheck: 'false', dir: 'auto' });
  const again = h('input', { id: `${prefix}-pass2`, type: 'password', autocomplete: 'new-password', spellcheck: 'false', dir: 'auto' });
  const fields = h(
    'div',
    { class: 'vault-fields', hidden: !opts.checked },
    h(
      'div',
      { class: 'kf-field' },
      h('label', { class: 'kf-field-label', for: `${prefix}-pass` }, t('vault.passphrase')),
      h('div', { class: 'kf-field-box' }, pass),
      h('p', { class: 'kf-field-help' }, t('vault.passphrase_help', { min: MIN_PASSPHRASE })),
    ),
    h(
      'div',
      { class: 'kf-field' },
      h('label', { class: 'kf-field-label', for: `${prefix}-pass2` }, t('vault.repeat')),
      h('div', { class: 'kf-field-box' }, again),
    ),
    h('p', { class: 'method-warning', role: 'note' }, line('warning'), t('vault.lose_warning')),
  );
  check.addEventListener('change', () => {
    fields.hidden = !check.checked;
    if (!check.checked) {
      pass.value = '';
      again.value = '';
    }
  });
  const box = h(
    'div',
    { class: 'section-body' },
    h('label', { class: 'check-row' }, check, line('lock'), t('vault.encrypt')),
    h('p', { class: 'kf-muted' }, t('vault.encrypt_help')),
    fields,
  );
  return {
    box,
    wanted: () => check.checked,
    problem: () => {
      if (!check.checked) return null;
      const key = newPassphraseProblem(pass.value, again.value);
      return key ? t(key, { min: MIN_PASSPHRASE }) : null;
    },
    take: () => {
      const value = pass.value;
      pass.value = '';
      again.value = '';
      return value;
    },
  };
}

/** Makes the vault in the chosen (new or empty) Drive folder, saying that it takes a moment. */
export async function createVaultIn(folder: { accountId: string; id: string; driveId: string | null; path: string }, passphrase: string): Promise<VaultInfo> {
  toast(t('vault.making_keys'));
  const parts = folder.path.split('/').filter(Boolean);
  return api.createVault({
    accountId: folder.accountId,
    folderId: folder.id,
    driveId: folder.driveId,
    remotePath: folder.path,
    name: parts[parts.length - 1] ?? folder.path,
    passphrase,
  });
}
