// The New pair and Edit pair sheet's rules (#65), apart from the DOM so they can be tested.

import { isUiError } from '../api';
import { isWrongPassphrase } from './vault';
import type { Pair, PairDrive, PairEdit, SyncMode } from '../types';

/** The three directions most people want, shown first; the other four sit behind "more". */
export const MAIN_METHODS: SyncMode[] = ['TWO_WAY', 'UPLOAD_ONLY', 'DOWNLOAD_ONLY'];
export const MORE_METHODS: SyncMode[] = ['UPLOAD_THEN_DELETE', 'UPLOAD_MIRROR', 'DOWNLOAD_THEN_DELETE', 'DOWNLOAD_MIRROR'];

/** Edit pair can change a pair's folders: the service resets a moved pair and holds it for a new preview (#94). The
 *  sheet shows them read-only when this is false. */
export const FOLDER_EDIT = true;

/** The field a refusal is about; `form` when it is about none in particular. */
export type SheetField = 'local' | 'drive' | 'name' | 'vault' | 'form';

/** A refusal as words the sheet shows next to its field: an i18n key and its values, or the service's own words. */
export interface Refusal {
  field: SheetField;
  key: string;
  vars?: Record<string, string>;
  /** The service's words, when no key fits. */
  message?: string;
}

/** Maps a refusal from the service (the pair rules in wasl-pairs, the daemon's checks, a vault) to the field it
 *  concerns and words in the person's language. */
export function refusalOf(e: unknown): Refusal {
  if (isWrongPassphrase(e)) return { field: 'vault', key: 'vault.wrong' };
  const msg = isUiError(e) ? e.message : String(e);
  let m = /^(.*) is already synced by the pair (.*?), or is in or around its folder/s.exec(msg);
  if (m) return { field: 'local', key: 'pairsheet.refuse_overlap', vars: { pair: m[2]! } };
  m = /^(.*) is in or around the view (.*?), which is read-only/s.exec(msg);
  if (m) return { field: 'local', key: 'pairsheet.refuse_view', vars: { view: m[2]! } };
  if (/where Wasl keeps its own data/.test(msg)) return { field: 'local', key: 'pairsheet.refuse_own' };
  if (/by its full path/.test(msg)) return { field: 'local', key: 'add.missing_folder' };
  if (/Choose a Google account and a Drive folder/.test(msg)) return { field: 'drive', key: 'add.missing_drive' };
  // A vault's refusals (wasld's vaults.rs), in the person's language; any other keeps the service's words.
  m = /passphrase needs at least (\d+) characters/.exec(msg);
  if (m) return { field: 'vault', key: 'vault.too_short', vars: { min: m[1]! } };
  if (/^Type the vault's passphrase/.test(msg)) return { field: 'vault', key: 'pairsheet.refuse_vault_empty' };
  if (/isn't empty\. A new vault needs a new or empty Drive folder/.test(msg)) return { field: 'drive', key: 'pairsheet.refuse_vault_not_empty' };
  if (/already holds a vault/.test(msg)) return { field: 'drive', key: 'pairsheet.refuse_vault_exists' };
  if (/belongs to a different vault/.test(msg)) return { field: 'drive', key: 'pairsheet.refuse_vault_other' };
  if (/vault|passphrase|isn't empty/i.test(msg)) return { field: 'vault', key: 'common.error', message: msg };
  if (/needs a name/.test(msg)) return { field: 'name', key: 'add.missing_name' };
  // A folder change while the pair runs (wasld's UpdatePair, #94). Edit pair moves it to the Drive folder when only
  // that changed (refusalFor).
  m = /^(.*) is syncing\. Stop it or let it finish, then change its folders/s.exec(msg);
  if (m) return { field: 'local', key: 'pairsheet.refuse_syncing', vars: { name: m[1]! } };
  return { field: 'form', key: 'common.error', message: msg };
}

/** Whether an edit turns a pair into one that deletes files (a mirror or a then-delete method). Edit asks first: a
 *  live pair takes the new method on its next sync, without a preview. */
export function editDeletes(from: SyncMode, to: SyncMode): boolean {
  return to !== from && MORE_METHODS.includes(to);
}

/** What Edit pair's form holds. `local` and `drive` are left out (or the same as the pair's) when they stay. */
export interface EditForm {
  name: string;
  method: SyncMode;
  /** The folder on this computer, when the person chose one. */
  local?: string;
  /** The Drive side, when the person chose a Drive folder. */
  drive?: PairDrive | null;
}

/** Whether the form's Drive side is another one than the pair's: another account or Drive folder. Picking the pair's
 *  own folder again is no move, whatever the vault choice says. */
function driveMoved(pair: Pair, d: PairDrive): boolean {
  return d.accountId !== pair.account_id || d.driveFolderId !== pair.drive_folder_id;
}

/** A Drive folder chosen in Edit pair, with the vault choice beside it. */
export interface DriveChoice {
  accountId: string;
  folderId: string;
  /** "Encrypt" is ticked. */
  encrypt: boolean;
  /** A vault was already made in this folder (an earlier Save that the service refused). */
  made: boolean;
  /** The person confirmed that the new folder won't be encrypted. */
  dropConfirmed: boolean;
}

/** What a chosen Drive folder does: nothing (none chosen, or the pair's own again), a move with no vault, a new one
 *  or the one already made, or first a confirmation, when a vault pair would move without encryption. */
export type DriveStep = { kind: 'stay' } | { kind: 'move'; vault: 'none' | 'new' | 'made' } | { kind: 'confirm_drop' };

export function driveStep(pair: Pair, c: DriveChoice | null): DriveStep {
  if (!c || (c.accountId === pair.account_id && c.folderId === pair.drive_folder_id)) return { kind: 'stay' };
  if (c.encrypt) return { kind: 'move', vault: c.made ? 'made' : 'new' };
  if (pair.vault_id && !c.dropConfirmed) return { kind: 'confirm_drop' };
  return { kind: 'move', vault: 'none' };
}

/** Whether "Encrypt" starts ticked when an encrypted pair moves to another Drive folder: it keeps a vault unless the
 *  person turns it off. */
export function encryptsByDefault(pair: Pair): boolean {
  return !!pair.vault_id;
}

/** What an edit changes, or null when the person changed nothing. Folders are carried only when they change. */
export function editOf(pair: Pair, form: EditForm): PairEdit | null {
  const name = form.name.trim();
  const local = form.local && form.local !== pair.local_root ? form.local : undefined;
  const drive = form.drive && driveMoved(pair, form.drive) ? form.drive : undefined;
  if (name === pair.name && form.method === pair.method && !local && !drive) return null;
  return { name, method: form.method, ...(local ? { localRoot: local } : {}), ...(drive ? { drive } : {}) };
}

/** Whether an edit moves the pair's folders: it then starts over and waits for its preview (Needs approval). */
export function movesFolders(edit: PairEdit | null): boolean {
  return !!edit && (edit.localRoot !== undefined || edit.drive !== undefined);
}

/** A refusal of an edit, next to the field the edit changed: "is syncing" goes to the Drive folder when the local
 *  folder stayed, and to the form when both changed. */
export function refusalFor(e: unknown, edit: PairEdit): Refusal {
  const r = refusalOf(e);
  if (r.key === 'pairsheet.refuse_syncing' && edit.drive !== undefined) return { ...r, field: edit.localRoot === undefined ? 'drive' : 'form' };
  return r;
}
