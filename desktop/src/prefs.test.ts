import { describe, expect, it } from 'vitest';
import { PANES, accentOf, applyAccent, onColor, paneOf, paneSignature } from './prefs';
import type { DaemonStatus, PairState } from './types';

/** Just enough of an element for applyAccent: its dataset and inline style. */
function fakeRoot() {
  const props = new Map<string, string>();
  const root = {
    dataset: {} as Record<string, string>,
    style: {
      setProperty: (k: string, v: string) => void props.set(k, v),
      removeProperty: (k: string) => void props.delete(k),
    },
  };
  return { root: root as unknown as HTMLElement, dataset: root.dataset, props };
}

describe('settings shared between windows', () => {
  it('reads anything but "system" as the Wasl accent', () => {
    expect(accentOf('system')).toBe('system');
    expect(accentOf('wasl')).toBe('wasl');
    expect(accentOf(null)).toBe('wasl');
    expect(accentOf('blue')).toBe('wasl');
  });

  it('reopens on a known pane, else General', () => {
    expect(PANES).toEqual(['general', 'accounts', 'sync', 'notifications', 'advanced']);
    expect(paneOf('accounts')).toBe('accounts');
    expect(paneOf('versions')).toBe('general');
    expect(paneOf(null)).toBe('general');
  });

  it('marks the root for the system accent and takes it off again', () => {
    const { root, dataset, props } = fakeRoot();
    applyAccent(root, 'system');
    expect(dataset.kfAccent).toBe('system');
    expect(dataset.waslAccentColor).toBeUndefined();
    applyAccent(root, 'wasl');
    expect(dataset.kfAccent).toBeUndefined();
    expect(props.size).toBe(0);
  });

  it('uses the colour Rust read on Windows, with readable text on it', () => {
    const { root, dataset, props } = fakeRoot();
    applyAccent(root, 'system', '#005fb8');
    expect(dataset.waslAccentColor).toBe('');
    expect(props.get('--wasl-system-accent')).toBe('#005fb8');
    expect(props.get('--wasl-system-on-accent')).toBe('#ffffff');
    applyAccent(root, 'wasl', '#005fb8');
    expect(dataset.waslAccentColor).toBeUndefined();
    expect(props.has('--wasl-system-accent')).toBe(false);
  });

  it('ignores a colour that is not #rrggbb', () => {
    const { root, props } = fakeRoot();
    applyAccent(root, 'system', 'red; background: url(x)');
    expect(props.size).toBe(0);
  });

  it('stays Wasl navy where the engine has no CSS accent and no colour came back (Windows)', () => {
    const { root, props } = fakeRoot();
    expect(applyAccent(root, 'system', null, false)).toBe(false);
    expect(root.dataset.kfAccent).toBeUndefined();
    expect(props.size).toBe(0);
    expect(applyAccent(root, 'system', '#005fb8', false)).toBe(true);
    expect(root.dataset.kfAccent).toBe('system');
  });

  it('puts black text on light accents and white on dark ones', () => {
    expect(onColor('#ffb900')).toBe('#000000');
    expect(onColor('#1f3d6b')).toBe('#ffffff');
    expect(onColor('#ffffff')).toBe('#000000');
  });
});

describe('when a Settings pane redraws on a status', () => {
  const pair = (paused: boolean) => ({ pair: { paused } }) as unknown as PairState;
  const status = (over: Partial<DaemonStatus>): DaemonStatus =>
    ({ version: '0.3.3', protocol: 1, accounts: [], pairs: [], overall: 'up_to_date', ...over }) as DaemonStatus;

  it('ignores a status that only moves sync progress', () => {
    const a = status({ pairs: [pair(false), pair(true)] });
    const b = status({ pairs: [pair(false), pair(true)], overall: 'syncing' });
    for (const pane of PANES) expect(paneSignature(pane, a)).toBe(paneSignature(pane, b));
  });

  it('redraws Accounts when an account signs out or is added', () => {
    const one = status({ accounts: [{ id: 'a', email: 'me@example.com', auth_error: null }] });
    const expired = status({ accounts: [{ id: 'a', email: 'me@example.com', auth_error: 'expired' }] });
    expect(paneSignature('accounts', one)).not.toBe(paneSignature('accounts', expired));
    expect(paneSignature('accounts', one)).not.toBe(paneSignature('accounts', status({})));
  });

  it('redraws Sync when a pair is paused', () => {
    expect(paneSignature('sync', status({ pairs: [pair(false)] }))).not.toBe(paneSignature('sync', status({ pairs: [pair(true)] })));
  });
});
