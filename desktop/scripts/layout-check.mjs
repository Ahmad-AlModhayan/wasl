// Layout regression check (#203): opens the window's UI (the dev mock, src/dev) in headless Chrome or Edge at real
// window sizes and fails when parts of the shell draw over each other, controls are covered or cut off, or the
// window scrolls sideways. scripts/layout-probe.js holds the checks; this drives the browser over the DevTools
// protocol with Node's own WebSocket, so it needs no extra packages.
//
//   npm run test:layout                 every screen, size, theme and language
//   npm run test:layout -- --quick      All pairs only
//   npm run test:layout -- --shots=DIR  also save a PNG of each case
//   CHROME_PATH=/path/to/chrome         choose the browser (else Chrome, Edge or Chromium where they usually live)
//
// Sizes are the window's CSS pixels: 1280×800, the default 980×700, 800×600 and the minimum 640×480 (tauri.conf.json);
// "150%" and "200%" are Windows' text size or display scale, which give the page 2/3 and half the CSS pixels.

import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { inflateSync } from 'node:zlib';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const probe = readFileSync(join(here, 'layout-probe.js'), 'utf8');
const args = process.argv.slice(2);
const quick = args.includes('--quick');
const shotsDir = args.find((a) => a.startsWith('--shots='))?.slice('--shots='.length);
const only = args.find((a) => a.startsWith('--only='))?.slice('--only='.length);

const SIZES = [
  { name: '1280x800', w: 1280, h: 800, scale: 1 },
  { name: '980x700', w: 980, h: 700, scale: 1 },
  { name: '980x700@150', w: 653, h: 467, scale: 1.5 },
  { name: '800x600', w: 800, h: 600, scale: 1 },
  { name: '640x480', w: 640, h: 480, scale: 1 },
  { name: '980x700@200', w: 490, h: 350, scale: 2 },
];
const SCREENS = quick ? ['pairs'] : ['pairs', 'pair', 'views', 'devices', 'problems', 'agents', 'history', 'bin', 'settings'];
const THEMES = quick ? ['dark'] : ['light', 'dark'];
const LANGS = quick ? ['en'] : ['en', 'ar'];
const PLATFORMS = quick ? ['windows'] : ['windows', 'mac'];
// The sidebar toggled from the toolbar: hidden in a wide window, opened over the pane in a narrow one (the report).
const OPEN_SIDEBAR = SIZES.map((s) => s.name);

function findBrowser() {
  if (process.env.CHROME_PATH) return process.env.CHROME_PATH;
  const candidates = {
    win32: [
      'C:/Program Files/Google/Chrome/Application/chrome.exe',
      'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
      'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
    ],
    darwin: [
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
      '/Applications/Chromium.app/Contents/MacOS/Chromium',
    ],
    linux: ['/usr/bin/google-chrome', '/usr/bin/google-chrome-stable', '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/microsoft-edge'],
  }[process.platform] ?? [];
  return candidates.find((p) => existsSync(p));
}

async function waitFor(fn, ms, what) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await fn().catch(() => undefined);
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

/** A DevTools protocol session on one page. */
class Page {
  constructor(ws, sessionId) {
    this.ws = ws;
    this.sessionId = sessionId;
  }
  send(method, params = {}) {
    return this.ws.call(method, params, this.sessionId);
  }
  async evaluate(expression) {
    const r = await this.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
}

async function connect(url) {
  const ws = new WebSocket(url);
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = rej;
  });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.id && pending.has(msg.id)) {
      const { res, rej } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) rej(new Error(msg.error.message));
      else res(msg.result);
    }
  };
  ws.call = (method, params, sessionId) =>
    new Promise((res, rej) => {
      const n = ++id;
      pending.set(n, { res, rej });
      ws.send(JSON.stringify({ id: n, method, params, sessionId }));
    });
  return ws;
}

let langSet;
/** Stores the language as Settings does (localStorage, read at start), once per language. */
async function setLanguage(page, base, lang) {
  if (langSet === lang) return;
  await page.send('Page.navigate', { url: base });
  await waitFor(() => page.evaluate(`document.readyState === 'complete'`), 15000, 'the page');
  await page.evaluate(`localStorage.setItem('wasl.language', '${lang}'); localStorage.removeItem('wasl.sidebar'); true`);
  langSet = lang;
}

/** The colour of one pixel of a 1×1 PNG from Page.captureScreenshot. A one-pixel first row reads the same under
 *  every PNG filter, so the bytes after the filter byte are the pixel. */
function pixel(png) {
  let at = 8;
  const idat = [];
  while (at < png.length) {
    const len = png.readUInt32BE(at);
    const type = png.toString('ascii', at + 4, at + 8);
    if (type === 'IDAT') idat.push(png.subarray(at + 8, at + 8 + len));
    at += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  return [raw[1], raw[2], raw[3]];
}

/**
 * Hover and press keep a filled button on its own colour (#203: blue buttons turned black on hover). Forces the
 * states through DevTools, so no pointer is needed, and compares the colour each button is drawn with, read from
 * the screen beside its label: the fill and the state layer over it (a background-image) together. The state
 * layer is a light wash of the label colour (the hover and pressed tokens); a change of more than about a fifth
 * of the range in any channel means the fill itself changed.
 */
async function buttonStates(page) {
  const problems = [];
  const sel = '.kf-desk-btn-default, .kf-btn-filled, .kf-btn-tinted, .kf-desk-btn:not(.kf-desk-btn-default, .kf-desk-btn-plain, .kf-desk-icon-btn)';
  const spots = await page.evaluate(`[...document.querySelectorAll('${sel}')].map((b) => {
    const r = b.getBoundingClientRect();
    const x = r.left + 3, y = r.top + r.height / 2;
    const seen = r.width > 8 && x >= 0 && y >= 0 && x < innerWidth && y < innerHeight && document.elementFromPoint(x, y)?.closest('button') === b;
    return seen ? { x, y } : null;
  })`);
  const read = async () => {
    const out = [];
    for (const s of spots) {
      if (!s) { out.push(null); continue; }
      const { data } = await page.send('Page.captureScreenshot', { format: 'png', clip: { x: s.x, y: s.y, width: 1, height: 1, scale: 1 } });
      out.push(pixel(Buffer.from(data, 'base64')));
    }
    return out;
  };
  const rest = await read();
  const { root } = await page.send('DOM.getDocument', { depth: -1 });
  const { nodeIds } = await page.send('DOM.querySelectorAll', { nodeId: root.nodeId, selector: sel });
  for (const state of ['hover', 'active']) {
    for (const nodeId of nodeIds) await page.send('CSS.forcePseudoState', { nodeId, forcedPseudoClasses: [state] });
    await new Promise((r) => setTimeout(r, 250)); // past the state transition
    const now = await read();
    now.forEach((c, i) => {
      const was = rest[i];
      if (!c || !was) return;
      if (Math.max(...c.map((v, k) => Math.abs(v - was[k]))) > 52) problems.push(`button ${i} colour changes on ${state}: rgb(${was}) -> rgb(${c})`);
    });
    for (const nodeId of nodeIds) await page.send('CSS.forcePseudoState', { nodeId, forcedPseudoClasses: [] });
  }
  if (!rest.some(Boolean)) problems.push('no filled buttons found on screen to check');
  return problems;
}

async function main() {
  const browserPath = findBrowser();
  if (!browserPath) {
    console.error('layout-check: no Chrome, Edge or Chromium found; set CHROME_PATH');
    process.exit(2);
  }
  const server = await createServer({ root, configFile: join(root, 'vite.config.ts'), server: { port: 0, strictPort: false }, logLevel: 'error' });
  await server.listen();
  const base = server.resolvedUrls.local[0];

  const profile = mkdtempSync(join(tmpdir(), 'wasl-layout-'));
  const browser = spawn(browserPath, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--hide-scrollbars=false', 'about:blank'], { stdio: 'ignore' });
  const portFile = join(profile, 'DevToolsActivePort');
  const [port, path] = (await waitFor(async () => existsSync(portFile) && readFileSync(portFile, 'utf8').trim().split('\n'), 15000, 'the browser')).map((s) => s.trim());
  const ws = await connect(`ws://127.0.0.1:${port}${path}`);
  const { targetId } = await ws.call('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await ws.call('Target.attachToTarget', { targetId, flatten: true });
  const page = new Page(ws, sessionId);
  await page.send('Page.enable');
  await page.send('Runtime.enable');
  await page.send('DOM.enable');
  await page.send('CSS.enable');
  if (shotsDir) mkdirSync(shotsDir, { recursive: true });

  const failures = [];
  let cases = 0;
  try {
    for (const platform of PLATFORMS)
      for (const lang of LANGS)
        for (const theme of THEMES)
          for (const size of SIZES) {
            await page.send('Emulation.setDeviceMetricsOverride', { width: size.w, height: size.h, deviceScaleFactor: size.scale, mobile: false });
            await page.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
            for (const screen of SCREENS) {
              for (const sidebarOpen of OPEN_SIDEBAR.includes(size.name) && screen === 'pairs' ? [false, true] : [false]) {
                const label = `${platform} ${lang} ${theme} ${size.name} ${screen}${sidebarOpen ? ' +sidebar' : ''}`;
                if (only && !label.includes(only)) continue;
                cases++;
                // A fresh page for each case, in the language set the way Settings stores it.
                await setLanguage(page, base, lang);
                await page.send('Page.navigate', { url: `${base}?platform=${platform}&theme=${theme}` });
                await waitFor(() => page.evaluate(`!!document.querySelector('.kf-desk-table tbody tr, .shell-sidebar .shell-pair')`), 15000, 'the pairs');
                if (screen === 'pair') await page.evaluate(`document.querySelector('.shell-sidebar .shell-pair')?.click(); true`);
                else if (screen !== 'pairs') await page.evaluate(`document.querySelector('[data-key="nav-${screen}"]')?.click(); true`);
                if (sidebarOpen) await page.evaluate(`document.querySelector('[data-key="cmd-sidebar"], .shell-hamburger')?.click(); true`);
                // Dismiss the update toast so it isn't counted (it floats on purpose).
                await page.evaluate(`document.querySelectorAll('.toast-region').forEach((t) => t.remove()); true`);
                // Let the screen load (the mock answers after a short delay), then settle.
                await waitFor(() => page.evaluate(`!document.querySelector('.main .loading, .main [aria-busy="true"]')`), 5000, 'the screen').catch(() => {});
                await new Promise((r) => setTimeout(r, 200));
                const problems = await page.evaluate(probe);
                if (screen === 'pairs' && !sidebarOpen && size.name === '980x700') problems.push(...(await buttonStates(page)));
                if (problems.length) failures.push({ label, problems });
                if (shotsDir) {
                  // Problems' shot shows the guard's reason, which a small window puts below the fold.
                  if (screen === 'problems') await page.evaluate(`document.querySelector('.review-detail .review-muted')?.scrollIntoView({ block: 'center' }); true`);
                  const { data } = await page.send('Page.captureScreenshot', { format: 'png' });
                  writeFileSync(join(shotsDir, `${label.replace(/[ +]+/g, '-')}.png`), Buffer.from(data, 'base64'));
                }
              }
            }
          }
  } finally {
    ws.close();
    browser.kill();
    await server.close();
    try {
      rmSync(profile, { recursive: true, force: true });
    } catch {
      // The browser may still hold its profile for a moment on Windows.
    }
  }

  for (const f of failures) {
    console.log(`FAIL ${f.label}`);
    for (const p of f.problems) console.log(`  - ${p}`);
  }
  console.log(`layout-check: ${cases - failures.length}/${cases} cases clean`);
  process.exit(failures.length ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
