import { beforeAll, describe, expect, it } from 'vitest';
import type { Conn, DaemonStatus } from '../types';
import { appAlert, showFirstRun } from '../appState';
import { devOverrides, installMock, setMockState } from './mock';

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;

beforeAll(() => {
  (globalThis as Record<string, unknown>).window = globalThis;
  installMock();
  invoke = (globalThis as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});

const alertNow = async (online = true) =>
  appAlert({ conn: (await invoke('daemon_state')) as Conn, status: (await invoke('status')) as DaemonStatus, online });

describe('the mock’s app states (?state=)', () => {
  it('reads only the states it knows from the address', () => {
    expect(devOverrides('?state=quota').state).toBe('quota');
    expect(devOverrides('?state=nonsense').state).toBeUndefined();
  });

  it('reaches Google Drive full for two pairs', async () => {
    setMockState('quota');
    expect(await alertNow()).toMatchObject({ kind: 'quota', count: 2 });
  });

  it('reaches the stopped service, and Restart service brings it back', async () => {
    setMockState('stopped');
    expect(await alertNow()).toMatchObject({ kind: 'stopped', notFound: false });
    await invoke('start_daemon');
    expect(await invoke('daemon_state')).toEqual({ state: 'starting' });
    await new Promise((r) => setTimeout(r, 1300));
    expect(await invoke('daemon_state')).toMatchObject({ state: 'connected' });
  });

  it('reaches the first run: no account and no pair', async () => {
    setMockState('first');
    const s = (await invoke('status')) as DaemonStatus;
    expect(showFirstRun(s, false)).toBe(true);
    await invoke('add_account');
    expect(((await invoke('status')) as DaemonStatus).accounts).toHaveLength(1);
  });
});
