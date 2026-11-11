// Builds wasld and the wasl CLI from ../engine in release mode and puts them where Tauri's bundler
// takes sidecars from (src-tauri/binaries/<name>-<target triple>[.exe]), so the installer ships the
// daemon next to the tray app (daemon.rs looks for it there first) and the CLI agents call.
// On macOS it also builds the File Provider extension and its helper (macos-provider/build.sh),
// which tauri.bundle.conf.json puts into Wasl.app.
//
//   node scripts/stage-engine.mjs [--target <triple>]

import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const engine = join(here, '..', '..', 'engine');
const out = join(here, '..', 'src-tauri', 'binaries');

const i = process.argv.indexOf('--target');
const target =
  i > 0
    ? process.argv[i + 1]
    : execFileSync('rustc', ['-vV'], { encoding: 'utf8' })
        .split('\n')
        .find((l) => l.startsWith('host:'))
        .slice(5)
        .trim();
const exe = target.includes('windows') ? '.exe' : '';

const args = ['build', '--release', '-p', 'wasld', '-p', 'wasl-cli', '--manifest-path', join(engine, 'Cargo.toml')];
if (i > 0) args.push('--target', target);
execFileSync('cargo', args, { stdio: 'inherit' });

const built = i > 0 ? join(engine, 'target', target, 'release') : join(engine, 'target', 'release');
mkdirSync(out, { recursive: true });
for (const name of ['wasld', 'wasl']) {
  copyFileSync(join(built, name + exe), join(out, `${name}-${target}${exe}`));
  console.log(`staged ${name}-${target}${exe}`);
}

if (process.platform === 'darwin') {
  execFileSync(join(here, '..', 'macos-provider', 'build.sh'), [], { stdio: 'inherit' });
}
