// Which window may call which of the app's commands (#69, #71). build.rs declares every command to Tauri's access
// control; capabilities/default.json lets the main window call all of them, and capabilities/tray.json and
// settings.json let the tray's popover and the macOS Settings window call only what their pages use. A command
// missing from any of them fails only when the app runs, so these tests read the files themselves.
import { describe, expect, it } from 'vitest';
import mainRs from '../src-tauri/src/main.rs?raw';
import buildRs from '../src-tauri/build.rs?raw';
import defaultCapability from '../src-tauri/capabilities/default.json';
import settingsCapability from '../src-tauri/capabilities/settings.json';
import trayCapability from '../src-tauri/capabilities/tray.json';
import apiTs from './api.ts?raw';

/** Every module of the front end, as text. */
const sources = import.meta.glob('./**/*.ts', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

/** `rel` imported from `file`, as a key of `sources`, or undefined for JSON and packages. */
function resolve(file: string, rel: string): string | undefined {
  const parts = file.split('/').slice(0, -1);
  for (const part of rel.split('/')) {
    if (part === '..') parts.pop();
    else if (part !== '.') parts.push(part);
  }
  const base = parts.join('/');
  return [base, `${base}.ts`, `${base}/index.ts`].find((p) => p in sources);
}

/** The commands a page can call: `api.…` in its entry module and in every module it imports (type-only imports
 *  bring no code, so they don't count). */
function pageCommands(entry: string): Set<string> {
  const commands = apiCommands();
  const seen = new Set<string>();
  const used = new Set<string>();
  const visit = (file: string) => {
    if (seen.has(file)) return;
    seen.add(file);
    const text = sources[file]!;
    for (const m of text.matchAll(/^import(?!\s+type\b)[^;]*?from\s+'(\.[^']+)'|import\(\s*'(\.[^']+)'\s*\)/gm)) {
      const next = resolve(file, (m[1] ?? m[2])!);
      if (next) visit(next);
    }
    for (const m of text.matchAll(/\bapi\.(\w+)\(/g)) used.add(commands.get(m[1]!) ?? `unknown api.${m[1]}`);
  };
  visit(entry);
  return used;
}

/** The commands main.rs hands to `generate_handler!`. */
function registered(): string[] {
  const list = /generate_handler!\[([\s\S]*?)\]\)/.exec(mainRs)?.[1] ?? '';
  return [...list.replace(/\/\/.*$/gm, '').matchAll(/\w+::(\w+)/g)].map((m) => m[1]!);
}

/** The commands build.rs declares in `AppManifest::commands`. */
function declared(): string[] {
  const list = /AppManifest::new\(\)\.commands\(&\[([\s\S]*?)\]\)/.exec(buildRs)?.[1] ?? '';
  return [...list.matchAll(/"(\w+)"/g)].map((m) => m[1]!);
}

/** api.ts: each method and the command it invokes. */
function apiCommands(): Map<string, string> {
  const methods = apiTs.matchAll(/(\w+):\s*\([^)]*\)\s*=>\s*invoke(?:<[^(]*>)?\(\s*'(\w+)'/g);
  return new Map([...methods].map((m) => [m[1]!, m[2]!]));
}

/** The permission Tauri makes for a command. */
const allow = (command: string) => `allow-${command.replaceAll('_', '-')}`;

const sorted = (list: Iterable<string>) => [...list].sort();

describe('the app commands and the windows that may call them', () => {
  it('build.rs declares exactly the commands main.rs registers', () => {
    expect(registered().length).toBeGreaterThan(50);
    expect(sorted(declared())).toEqual(sorted(registered()));
  });

  it('every invoke in api.ts is read, and each names a registered command', () => {
    const invokes = [...apiTs.matchAll(/\binvoke(?:<[^(]*>)?\(\s*'/g)].length;
    const commands = apiCommands();
    expect(commands.size).toBe(invokes);
    const known = new Set(registered());
    expect([...commands.values()].filter((c) => !known.has(c))).toEqual([]);
  });

  it('the main window may call every command', () => {
    const granted = new Set(defaultCapability.permissions);
    expect(registered().filter((c) => !granted.has(allow(c)))).toEqual([]);
  });

  it("the tray's popover may call exactly the commands its page uses", () => {
    const granted = trayCapability.permissions.filter((p) => p.startsWith('allow-'));
    expect(sorted(granted)).toEqual(sorted([...pageCommands('./tray/popover.ts')].map(allow)));
  });

  it('the macOS Settings window may call exactly the commands its page uses', () => {
    const used = pageCommands('./settingsWindow.ts');
    expect(used.has('set_tray_visible')).toBe(true);
    const granted = settingsCapability.permissions.filter((p) => p.startsWith('allow-'));
    expect(sorted(granted)).toEqual(sorted([...used].map(allow)));
  });

  it('the Settings window gets no folder picker and no window powers beyond titling, sizing and dragging itself', () => {
    expect(settingsCapability.windows).toEqual(['settings']);
    expect(settingsCapability.permissions.filter((p) => !p.startsWith('allow-'))).toEqual([
      'core:default',
      'core:window:allow-start-dragging',
      'core:window:allow-set-title',
      'core:window:allow-set-size',
    ]);
  });

  it("the tray's popover only listens to events: it can't emit them or touch windows", () => {
    expect(trayCapability.permissions.filter((p) => !p.startsWith('allow-'))).toEqual(['core:event:allow-listen', 'core:event:allow-unlisten']);
  });
});
