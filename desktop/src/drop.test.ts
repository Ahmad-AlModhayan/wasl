import { describe, expect, it } from 'vitest';
import en from './i18n/en.json';
import ar from './i18n/ar.json';
import { dropVerdict, folderOf } from './drop';
import { NOTIFY_KINDS } from './notifyText';
import { NOTIFY_CATEGORIES, settingOn } from './screens/settings';

const folders = new Set(['/Users/a/Projects', '/Users/a/Notes']);
const isFolder = async (p: string) => folders.has(p);

describe('a folder dropped on the window', () => {
  it('opens New pair with the first folder among the dropped paths', async () => {
    expect(await folderOf(['/Users/a/todo.txt', '/Users/a/Projects', '/Users/a/Notes'], isFolder)).toEqual({ folder: '/Users/a/Projects' });
  });

  it('gets a message when only files were dropped', async () => {
    expect(await folderOf(['/Users/a/todo.txt'], isFolder)).toEqual({ error: 'not_folder' });
  });

  it('treats a path it cannot check as a file', async () => {
    expect(await folderOf(['/gone'], () => Promise.reject(new Error('no')))).toEqual({ error: 'not_folder' });
  });

  it('does nothing when nothing was dropped', async () => {
    expect(await folderOf([], isFolder)).toBeNull();
  });
});

describe('what a dropped folder does', () => {
  const base = { ready: true, dialogOpen: false, firstRunStep: null };

  it('opens New pair when the service is ready and nothing is open', () => {
    expect(dropVerdict(base)).toBe('take');
  });

  it('waits for the service and the first-run check before taking a folder', () => {
    expect(dropVerdict({ ...base, ready: false })).toBe('not_ready');
  });

  it('is refused with a reason while a sheet or dialog is open', () => {
    expect(dropVerdict({ ...base, dialogOpen: true })).toBe('busy');
  });

  it('is kept for the folder step while the first run is still at sign-in', () => {
    expect(dropVerdict({ ...base, firstRunStep: 'sign_in' })).toBe('kept');
    expect(dropVerdict({ ...base, firstRunStep: 'folder' })).toBe('take');
    expect(dropVerdict({ ...base, firstRunStep: 'tray' })).toBe('take');
  });

  it('says every refusal in English and Arabic', () => {
    for (const v of ['not_ready', 'busy', 'kept']) {
      expect((en as Record<string, string>)[`drop.${v}`]).toBeTruthy();
      expect((ar as Record<string, string>)[`drop.${v}`]).toBeTruthy();
    }
  });
});

describe('the notifications and their settings', () => {
  const dicts = { en: en as Record<string, string>, ar: ar as Record<string, string> };

  it('have a title and a body for every kind in English and Arabic, with the same placeholders', () => {
    for (const kind of NOTIFY_KINDS) {
      for (const part of ['title', 'body']) {
        const key = `notify.${kind}_${part}`;
        const holes = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
        expect(dicts.en[key], key).toBeTruthy();
        expect(dicts.ar[key], key).toBeTruthy();
        expect(holes(dicts.ar[key]!), key).toEqual(holes(dicts.en[key]!));
      }
    }
  });

  it('name each category in Settings in both languages', () => {
    for (const c of NOTIFY_CATEGORIES) {
      for (const key of [`settings.notify_${c}`, `settings.notify_${c}_body`]) {
        expect(dicts.en[key], key).toBeTruthy();
        expect(dicts.ar[key], key).toBeTruthy();
      }
    }
  });

  it('are on until switched off, like the menu bar icon', () => {
    expect(settingOn(null)).toBe(true);
    expect(settingOn(undefined)).toBe(true);
    expect(settingOn('true')).toBe(true);
    expect(settingOn('false')).toBe(false);
  });
});
