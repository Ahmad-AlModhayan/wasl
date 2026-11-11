// macOS window chrome (#61, design D1): an overlay title bar, so the page draws the toolbar strip itself and keeps
// the traffic lights' corner clear. The strip is a drag region (`data-tauri-drag-region`, in app.ts); a
// double-click on it does what System Settings says rather than always zooming.

import { api } from '../api';
import { applyChrome } from './index';

export function mountMacChrome(): void {
  const root = document.documentElement;
  // One question per burst of resize events. A timer, not requestAnimationFrame: the window starts hidden.
  let timer: ReturnType<typeof setTimeout> | undefined;
  const refresh = (): void => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      void api
        .windowChrome()
        .then((c) => c && applyChrome(root, c))
        // Without an answer, play safe: an opaque window rather than one the desktop shows through.
        .catch(() => root.toggleAttribute('data-chrome-opaque', true));
    }, 30);
  };
  refresh();
  // Full screen, a narrower window, or Wasl's language flipping the page's direction.
  window.addEventListener('resize', refresh);
  // Reduce Transparency or Increase Contrast may have changed while Wasl was in the background.
  window.addEventListener('focus', refresh);
  new MutationObserver(refresh).observe(root, { attributes: true, attributeFilter: ['dir'] });
  followDoubleClickSetting();
}

/**
 * Tauri's drag script zooms on any double-click in a drag region. macOS lets people choose zoom, minimise or
 * nothing, so the second click's mouse-up is taken here first (window, capture phase) and Rust reads the setting.
 */
function followDoubleClickSetting(): void {
  let downX = 0;
  let downY = 0;
  const onStrip = (e: MouseEvent): boolean => {
    const el = e.target;
    return el instanceof HTMLElement && el.hasAttribute('data-tauri-drag-region') && el.dataset.tauriDragRegion !== 'false';
  };
  window.addEventListener(
    'mousedown',
    (e) => {
      if (e.button === 0 && e.detail === 2 && onStrip(e)) [downX, downY] = [e.clientX, e.clientY];
    },
    true,
  );
  window.addEventListener(
    'mouseup',
    (e) => {
      if (e.button !== 0 || e.detail !== 2 || !onStrip(e)) return;
      e.stopImmediatePropagation();
      // A double-click that turned into a drag does nothing, as in a native title bar.
      if (e.clientX === downX && e.clientY === downY) void api.titleBarDoubleClick().catch(() => undefined);
    },
    true,
  );
}
