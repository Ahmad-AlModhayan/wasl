// The Windows 11 title bar (D1, #62). The window has no native frame (src-tauri/tauri.windows.conf.json), so Wasl
// draws a 48px bar on Mica: the app mark and name at the leading edge, a centred search box (the shell's search, wired in app.ts) and the
// caption buttons at the trailing edge, which is the left in Arabic. Every empty part of the bar drags the window,
// double-click maximizes and right-click opens the system menu. Rust (src-tauri/src/chrome_windows.rs) lays a native
// window over the maximize button so hovering it opens Snap Layouts.
//
// The bar sits before #app in <body>, outside the shell the app rebuilds, and relabels itself when <html lang>
// changes.

import '../styles/chrome-windows.css';
import { emit, listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { t } from '../i18n';
import { h, mark } from '../ui';
import { CaptionButtons, glyphSource } from './captionButtons';

/** Events shared with src-tauri/src/chrome_windows.rs. */
const SNAP_AREA = 'chrome://snap-area';
const SYSTEM_MENU = 'chrome://system-menu';
const SNAP = 'chrome://snap';

/** In the app, only Windows draws its own bar (Linux keeps its native frame); the dev mock draws it anywhere. */
export function shouldDrawTitleBar(userAgent: string, inApp: boolean): boolean {
  return !inApp || /Windows NT/.test(userAgent);
}

/** Mica needs Windows 11 (platform version 13 and up in User-Agent Client Hints); elsewhere the window is solid. */
export function backdropFor(platform: string, platformVersion: string): 'mica' | 'solid' {
  return platform === 'Windows' && Number.parseInt(platformVersion, 10) >= 13 ? 'mica' : 'solid';
}

export interface Area {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A CSS-pixel box in the page as physical pixels in the window's client area, which the page fills. */
export function physicalArea(box: { left: number; top: number; width: number; height: number }, ratio: number): Area {
  const x = Math.round(box.left * ratio);
  const y = Math.round(box.top * ratio);
  return { x, y, width: Math.round((box.left + box.width) * ratio) - x, height: Math.round((box.top + box.height) * ratio) - y };
}

function searchIcon(): SVGSVGElement {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  for (const [k, v] of Object.entries({ class: 'wtb-search-icon', viewBox: '0 0 16 16', width: '16', height: '16', 'aria-hidden': 'true', focusable: 'false' })) svg.setAttribute(k, v);
  svg.innerHTML = '<circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor"/><path d="m10.5 10.5 3.5 3.5" stroke="currentColor"/>'; // our own constant markup
  return svg;
}

/** Alt+Space alone opens the system menu. */
export function isAltSpace(e: Pick<KeyboardEvent, 'altKey' | 'ctrlKey' | 'shiftKey' | 'metaKey' | 'code'>): boolean {
  return e.altKey && !e.ctrlKey && !e.shiftKey && !e.metaKey && e.code === 'Space';
}

/** Right-click and drag belong to the bar itself, not to its search box or buttons. */
function onControl(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('button, input') !== null;
}

export function mountWindowsChrome(): void {
  const inApp = !(window as { __WASL_MOCK__?: boolean }).__WASL_MOCK__;
  if (!shouldDrawTitleBar(navigator.userAgent, inApp)) return;
  const root = document.documentElement;
  const win = getCurrentWindow();

  const caps = new CaptionButtons(
    {
      minimize: () => void win.minimize(),
      maximize: () => void win.toggleMaximize(),
      // Hides the window: main.rs keeps the app and syncing running, as Alt+F4 does.
      close: () => void win.close(),
    },
    glyphSource(navigator.userAgent),
  );
  const name = h('span', { class: 'wtb-title' });
  const search = h('input', {
    type: 'search',
    class: 'wtb-search-field',
    // Out of the tab order until the shell wires it to its search (app.ts, bindChromeSearch).
    tabindex: '-1',
    'data-chrome-search': '',
  });
  const bar = h(
    'header',
    { class: 'wtb', 'data-tauri-drag-region': 'deep' },
    h('span', { class: 'wtb-mark' }, mark(16)),
    name,
    h('div', { class: 'wtb-search' }, h('div', { class: 'wtb-search-box' }, searchIcon(), search)),
    caps.el,
  );

  const label = () => {
    name.textContent = t('app.name');
    bar.setAttribute('aria-label', t('chrome.title_bar'));
    search.placeholder = t('chrome.search');
    search.setAttribute('aria-label', t('chrome.search'));
    caps.render();
  };
  label();
  document.body.prepend(bar);
  root.dataset.kfChrome = 'windows';

  // The system menu: right-click on the bar opens it at the pointer; Alt+Space, which WebView2 doesn't pass on to
  // the window, opens it under the bar's leading corner (the right one in Arabic), as on a native frame.
  const systemMenu = (left: number, top: number) => {
    const at = physicalArea({ left, top, width: 0, height: 0 }, devicePixelRatio);
    void emit(SYSTEM_MENU, { x: at.x, y: at.y, rtl: root.dir === 'rtl' });
  };
  bar.addEventListener('contextmenu', (e) => {
    if (onControl(e.target)) return;
    e.preventDefault();
    systemMenu(e.clientX, e.clientY);
  });
  addEventListener(
    'keydown',
    (e) => {
      if (!isAltSpace(e)) return;
      e.preventDefault();
      const box = bar.getBoundingClientRect();
      systemMenu(root.dir === 'rtl' ? box.right : box.left, box.bottom);
    },
    { capture: true },
  );

  // The bar and the caption glyphs dim while another window is active: the window's focus in the app, the page's in
  // the dev mock.
  const setActive = (active: boolean) => root.classList.toggle('kf-desk-inactive', !active);
  if (inApp) {
    void win.onFocusChanged((e) => setActive(e.payload));
    void win.isFocused().then(setActive, () => undefined);
  } else {
    const activity = () => setActive(document.hasFocus());
    addEventListener('focus', activity);
    addEventListener('blur', activity);
    activity();
  }

  // Tell Rust where the maximize button is, whenever it moves: resizing, a new language (the side changes), a new
  // display scale.
  let sent = '';
  const sendSnapArea = () => {
    const area = physicalArea(caps.maximize.getBoundingClientRect(), devicePixelRatio);
    const payload = JSON.stringify({ ...area, label: caps.maximize.getAttribute('aria-label') ?? '', maximized: caps.isMaximized });
    if (payload === sent) return;
    sent = payload;
    void emit(SNAP_AREA, JSON.parse(payload));
  };

  // Maximize turns into restore, and back.
  const maximized = async () => {
    caps.setMaximized(Boolean(await win.isMaximized().catch(() => false)));
    sendSnapArea();
  };
  void win.onResized(() => void maximized());
  void maximized();

  new ResizeObserver(sendSnapArea).observe(bar);
  const watchScale = () => {
    matchMedia(`(resolution: ${devicePixelRatio}dppx)`).addEventListener(
      'change',
      () => {
        sendSnapArea();
        watchScale();
      },
      { once: true },
    );
  };
  watchScale();
  new MutationObserver(() => {
    label();
    sendSnapArea();
  }).observe(root, { attributes: true, attributeFilter: ['lang', 'dir'] });

  // Hover and press on maximize come from the native window over it.
  void listen<string>(SNAP, (e) => {
    const state = e.payload === 'enter' || e.payload === 'release' ? 'hover' : e.payload === 'press' ? 'pressed' : null;
    caps.setMaximizeState(state);
  });

  void detectBackdrop();
}

/** Mica shows through a transparent page on Windows 11; earlier Windows and the dev mock paint a solid window. */
async function detectBackdrop(): Promise<void> {
  const root = document.documentElement;
  root.dataset.kfBackdrop = 'solid';
  const ua = (navigator as Navigator & { userAgentData?: NavigatorUAData }).userAgentData;
  if (!ua) return;
  const { platform, platformVersion } = await ua.getHighEntropyValues(['platformVersion']).catch(() => ({ platform: '', platformVersion: '' }));
  root.dataset.kfBackdrop = backdropFor(platform ?? '', platformVersion ?? '');
}

interface NavigatorUAData {
  getHighEntropyValues(hints: string[]): Promise<{ platform?: string; platformVersion?: string }>;
}
