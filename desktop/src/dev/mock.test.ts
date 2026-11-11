import { beforeAll, describe, expect, it, vi } from 'vitest';
import apiSource from '../api.ts?raw';
import type { DaemonStatus } from '../types';
import { devOverrides, installMock, isAnswered } from './mock';

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;

beforeAll(() => {
  // The mock installs itself on `window`, as Tauri does; here the global object stands in for it.
  (globalThis as Record<string, unknown>).window = globalThis;
  installMock();
  invoke = (globalThis as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});

describe('the development mock', () => {
  it('answers every command the window can send', () => {
    // A command the mock doesn't know returns nothing, which every caller takes as success: a removed pair
    // stayed listed under a "Removed" toast (found by TestSprite, 2026-10).
    const commands = [...apiSource.matchAll(/invoke(?:<[^']*?>)?\('([a-z_]+)'/g)].map((m) => m[1]);
    expect(commands.length).toBeGreaterThan(40);
    expect(commands.filter((c) => !isAnswered(c))).toEqual([]);
  });

  it('renames a pair and changes its direction', async () => {
    await invoke('update_pair', { id: 'life', edit: { name: 'Papers', method: 'UPLOAD_ONLY' } });
    const life = ((await invoke('status')) as DaemonStatus).pairs.find((p) => p.pair.id === 'life')!;
    expect(life.pair.name).toBe('Papers');
    expect(life.pair.method).toBe('UPLOAD_ONLY');
  });

  it('refuses an edit the way the service does, with words the sheet places by its field', async () => {
    await expect(invoke('update_pair', { id: 'life', edit: { name: ' ', method: 'TWO_WAY' } })).rejects.toMatchObject({ kind: 'invalid' });
    await expect(invoke('update_pair', { id: 'life', edit: { name: 'kahf-site', method: 'TWO_WAY' } })).rejects.toMatchObject({
      message: expect.stringContaining('already synced by the pair kahf-site'),
    });
  });

  it('removes a pair', async () => {
    const before = (await invoke('status')) as DaemonStatus;
    expect(before.pairs.some((p) => p.pair.id === 'music')).toBe(true);
    await invoke('delete_pair', { id: 'music' });
    const after = (await invoke('status')) as DaemonStatus;
    expect(after.pairs.some((p) => p.pair.id === 'music')).toBe(false);
    expect(after.pairs).toHaveLength(before.pairs.length - 1);
  });

  it('stops a running sync', async () => {
    await invoke('cancel', { id: 'site' });
    const photos = ((await invoke('status')) as DaemonStatus).pairs.find((p) => p.pair.id === 'site');
    expect(photos?.running).toBe(false);
    expect(photos?.progress).toBeNull();
  });

  it('shows a sync run, then its end', async () => {
    // "Sync now", "Sync all" and "Try again" changed nothing on screen (TestSprite TC003/TC004/TC005/TC021).
    vi.useFakeTimers({ toFake: ['setTimeout'] });
    try {
      /** Calls the mock past its pretend IPC latency. */
      const call = async (cmd: string, args?: Record<string, unknown>) => {
        const answer = invoke(cmd, args);
        await vi.advanceTimersByTimeAsync(200);
        return answer;
      };
      const pair = async (id: string) => ((await call('status')) as DaemonStatus).pairs.find((p) => p.pair.id === id);

      await call('sync', { id: 'life', trigger: 'manual' });
      expect((await pair('life'))?.running).toBe(true);
      expect((await pair('life'))?.progress).not.toBeNull();

      await vi.advanceTimersByTimeAsync(5000);
      const done = await pair('life');
      expect(done?.running).toBe(false);
      expect(done?.pair.status.last_status).toBe('SUCCESS');
      expect(done?.pair.status.last_error).toBeNull();

      // A locked vault's pair doesn't run.
      await call('sync', { id: 'private', trigger: 'manual' });
      expect((await pair('private'))?.running).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('answers the system folder and file pickers', async () => {
    // "Keep a file here…" and "Choose folder" did nothing in a browser (TestSprite TC025).
    expect(await invoke('plugin:dialog|open', { options: { directory: true, multiple: false } })).toMatch(/Documents/);
    const file = await invoke('plugin:dialog|open', { options: { defaultPath: 'C:\\Users\\me\\Archive (Drive)', directory: false } });
    expect(String(file).startsWith('C:\\Users\\me\\Archive (Drive)\\')).toBe(true);
  });

  it('keeps a setting it was given', async () => {
    expect(await invoke('setting', { key: 'start_at_login' })).toBeNull();
    await invoke('set_setting', { key: 'start_at_login', value: 'true' });
    expect(await invoke('setting', { key: 'start_at_login' })).toBe('true');
  });

  it('takes the platform and theme to show from the query', () => {
    expect(devOverrides('?platform=mac&theme=dark')).toEqual({ platform: 'mac', theme: 'dark' });
    expect(devOverrides('?platform=windows')).toEqual({ platform: 'windows', theme: undefined });
    expect(devOverrides('?platform=linux&theme=sepia')).toEqual({ platform: undefined, theme: undefined });
  });
});
