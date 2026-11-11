import { describe, expect, it } from 'vitest';
import { FOLDER_EDIT, MAIN_METHODS, MORE_METHODS, driveStep, editDeletes, editOf, encryptsByDefault, movesFolders, refusalFor, refusalOf } from './pairSheetModel';
import type { Pair } from '../types';

describe('a refusal from the service', () => {
  it('shows an overlap next to the local folder, naming the other pair', () => {
    const r = refusalOf({
      kind: 'invalid',
      message: '/Users/a/Code is already synced by the pair kahf-site, or is in or around its folder. Two pairs on the same files can undo each other. Choose another folder.',
    });
    expect(r).toEqual({ field: 'local', key: 'pairsheet.refuse_overlap', vars: { pair: 'kahf-site' } });
  });

  it("shows a view's folder and Wasl's own data folder next to the local folder", () => {
    expect(refusalOf({ kind: 'invalid', message: '/x is in or around the view Team, which is read-only. Choose another folder.' })).toEqual({
      field: 'local',
      key: 'pairsheet.refuse_view',
      vars: { view: 'Team' },
    });
    expect(refusalOf({ kind: 'invalid', message: '/x is in or around /y, where Wasl keeps its own data (its database and private key files). Choose another folder.' }).key).toBe(
      'pairsheet.refuse_own',
    );
  });

  it('shows a wrong vault passphrase next to the vault', () => {
    expect(refusalOf({ kind: 'invalid', message: 'Wrong passphrase for this vault.' })).toEqual({ field: 'vault', key: 'vault.wrong' });
  });

  it("words the vault's other refusals in the person's language", () => {
    expect(refusalOf({ kind: 'invalid', message: "A vault's passphrase needs at least 8 characters" })).toEqual({
      field: 'vault',
      key: 'vault.too_short',
      vars: { min: '8' },
    });
    expect(refusalOf({ kind: 'invalid', message: "Type the vault's passphrase" })).toEqual({ field: 'vault', key: 'pairsheet.refuse_vault_empty' });
    expect(refusalOf({ kind: 'invalid', message: 'Backup/Photos isn\'t empty. A new vault needs a new or empty Drive folder.' })).toEqual({
      field: 'drive',
      key: 'pairsheet.refuse_vault_not_empty',
    });
    expect(refusalOf({ kind: 'invalid', message: 'Backup/Photos already holds a vault. Unlock it instead.' }).key).toBe('pairsheet.refuse_vault_exists');
    expect(refusalOf({ kind: 'invalid', message: 'The Drive folder belongs to a different vault/passphrase' }).key).toBe('pairsheet.refuse_vault_other');
  });

  it("keeps the service's words for a vault refusal it doesn't know", () => {
    expect(refusalOf({ kind: 'invalid', message: 'The vault is damaged' })).toEqual({ field: 'vault', key: 'common.error', message: 'The vault is damaged' });
  });

  it('shows a missing Drive folder next to the Drive folder', () => {
    expect(refusalOf({ kind: 'invalid', message: 'Choose a Google account and a Drive folder' }).field).toBe('drive');
  });

  it("keeps the service's words for anything else, at the foot of the form", () => {
    expect(refusalOf({ kind: 'network', message: 'offline' })).toEqual({ field: 'form', key: 'common.error', message: 'offline' });
  });
});

describe('the directions', () => {
  it('offers all seven methods, three first', () => {
    expect(MAIN_METHODS).toHaveLength(3);
    expect(new Set([...MAIN_METHODS, ...MORE_METHODS]).size).toBe(7);
  });
});

describe('an edit', () => {
  const pair = { id: 'p', name: 'Docs', method: 'TWO_WAY' } as Pair;

  it('is nothing when the name and the method stay', () => {
    expect(editOf(pair, { name: ' Docs ', method: 'TWO_WAY' })).toBeNull();
  });

  it('carries a rename and a new method, trimmed', () => {
    expect(editOf(pair, { name: ' Documents ', method: 'UPLOAD_ONLY' })).toEqual({ name: 'Documents', method: 'UPLOAD_ONLY' });
  });
});

describe('changing a live pair to a deleting method', () => {
  it('asks first for every mirror and then-delete method', () => {
    for (const m of MORE_METHODS) expect(editDeletes('TWO_WAY', m)).toBe(true);
  });

  it("doesn't ask for the three main methods or when the method stays the same", () => {
    for (const m of MAIN_METHODS) expect(editDeletes('UPLOAD_MIRROR', m)).toBe(false);
    expect(editDeletes('UPLOAD_MIRROR', 'UPLOAD_MIRROR')).toBe(false);
  });
});

describe('moving a pair to other folders', () => {
  const pair = { id: 'p', name: 'Docs', method: 'TWO_WAY', local_root: '/u/Docs', account_id: 'acc', drive_folder_id: 'f1', remote_path: 'My Drive/Docs', vault_id: null } as Pair;
  const drive = { accountId: 'acc', driveFolderId: 'f2', remotePath: 'My Drive/Other', driveId: null, vaultId: null };
  const syncing = { kind: 'invalid', message: 'Docs is syncing. Stop it or let it finish, then change its folders.' };

  it('is on: Edit pair can change the folders', () => {
    expect(FOLDER_EDIT).toBe(true);
  });

  it('carries another local folder or Drive side, and nothing for the same ones', () => {
    expect(editOf(pair, { name: 'Docs', method: 'TWO_WAY', local: '/u/Docs', drive: null })).toBeNull();
    expect(editOf(pair, { name: 'Docs', method: 'TWO_WAY', drive: { ...drive, driveFolderId: 'f1' } })).toBeNull();
    expect(editOf(pair, { name: 'Docs', method: 'TWO_WAY', local: '/u/Papers' })).toEqual({ name: 'Docs', method: 'TWO_WAY', localRoot: '/u/Papers' });
    expect(editOf(pair, { name: 'Docs', method: 'TWO_WAY', drive })).toEqual({ name: 'Docs', method: 'TWO_WAY', drive });
  });

  it('counts another account as a move', () => {
    expect(editOf(pair, { name: 'Docs', method: 'TWO_WAY', drive: { ...drive, driveFolderId: 'f1', accountId: 'other' } })?.drive).toBeDefined();
  });

  it('re-picking the same folder is not a move', () => {
    const vaultPair = { ...pair, vault_id: 'v1' } as Pair;
    const same = { accountId: 'acc', folderId: 'f1', encrypt: false, made: false, dropConfirmed: false };
    expect(driveStep(vaultPair, same)).toEqual({ kind: 'stay' });
    expect(driveStep(pair, { ...same, encrypt: true })).toEqual({ kind: 'stay' });
    expect(driveStep(pair, null)).toEqual({ kind: 'stay' });
    expect(editOf(vaultPair, { name: 'Docs', method: 'TWO_WAY', drive: { ...drive, driveFolderId: 'f1', vaultId: null } })).toBeNull();
    expect(editOf(pair, { name: 'Docs', method: 'TWO_WAY', drive: { ...drive, driveFolderId: 'f1', vaultId: 'v' } })).toBeNull();
  });

  it('a vault pair moved keeps a vault unless explicitly dropped', () => {
    const vaultPair = { ...pair, vault_id: 'v1' } as Pair;
    const other = { accountId: 'acc', folderId: 'f2', encrypt: encryptsByDefault(vaultPair), made: false, dropConfirmed: false };
    expect(encryptsByDefault(vaultPair)).toBe(true);
    expect(encryptsByDefault(pair)).toBe(false);
    expect(driveStep(vaultPair, other)).toEqual({ kind: 'move', vault: 'new' });
    expect(driveStep(vaultPair, { ...other, made: true })).toEqual({ kind: 'move', vault: 'made' });
    expect(driveStep(vaultPair, { ...other, encrypt: false })).toEqual({ kind: 'confirm_drop' });
    expect(driveStep(vaultPair, { ...other, encrypt: false, made: true })).toEqual({ kind: 'confirm_drop' });
    expect(driveStep(vaultPair, { ...other, encrypt: false, dropConfirmed: true })).toEqual({ kind: 'move', vault: 'none' });
    expect(driveStep(pair, { ...other, encrypt: false })).toEqual({ kind: 'move', vault: 'none' });
  });

  it('holds a moved pair for its preview, and only a moved one', () => {
    expect(movesFolders(editOf(pair, { name: 'Docs', method: 'TWO_WAY', local: '/u/Papers' }))).toBe(true);
    expect(movesFolders(editOf(pair, { name: 'Papers', method: 'UPLOAD_ONLY' }))).toBe(false);
    expect(movesFolders(null)).toBe(false);
  });

  it('shows "is syncing" next to the local folder, in the person\'s language', () => {
    expect(refusalOf(syncing)).toEqual({ field: 'local', key: 'pairsheet.refuse_syncing', vars: { name: 'Docs' } });
  });

  it('shows "is syncing" next to the Drive folder when only that changed, and on the form when both did', () => {
    expect(refusalFor(syncing, { name: 'Docs', method: 'TWO_WAY', localRoot: '/u/x', drive }).field).toBe('form');
    expect(refusalFor(syncing, { name: 'Docs', method: 'TWO_WAY', drive }).field).toBe('drive');
    expect(refusalFor(syncing, { name: 'Docs', method: 'TWO_WAY', localRoot: '/u/x' }).field).toBe('local');
    expect(refusalFor({ kind: 'invalid', message: 'A pair needs a name' }, { name: '', method: 'TWO_WAY', drive }).field).toBe('name');
  });
});
