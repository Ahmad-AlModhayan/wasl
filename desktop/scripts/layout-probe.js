// Runs inside the page (scripts/layout-check.mjs injects it; paste it in a devtools console to try it by hand).
// Returns the layout problems a person would see: parts of the shell drawn over each other, controls covered or
// cut off, sideways scrolling. An empty list means the window looks right at this size (#203).
(() => {
  const problems = [];
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  const rect = (el) => el.getBoundingClientRect();
  const shown = (el) => {
    if (!el) return false;
    const r = rect(el);
    const cs = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none';
  };
  const name = (el) => {
    const label = el.getAttribute('aria-label') || el.textContent.trim().replace(/\s+/g, ' ').slice(0, 30);
    return `${el.tagName.toLowerCase()}.${[...el.classList].join('.')}${label ? ` "${label}"` : ''}`;
  };
  const overlaps = (a, b) => a.left < b.right - 1 && b.left < a.right - 1 && a.top < b.bottom - 1 && b.top < a.bottom - 1;
  const alpha = (color) => {
    const m = color.match(/rgba?\(([^)]+)\)/);
    if (!m) return color === 'transparent' ? 0 : 1;
    const parts = m[1].split(/[ ,/]+/).filter(Boolean);
    return parts.length > 3 ? Number(parts[3]) : 1;
  };

  // 1. No sideways scrolling of the window itself.
  if (document.documentElement.scrollWidth > vw + 1) problems.push(`page scrolls sideways: ${document.documentElement.scrollWidth}px in ${vw}px`);

  const shell = document.querySelector('.shell');
  const sidebar = document.querySelector('.shell-sidebar');
  const pane = document.querySelector('.pane');
  const view = shell?.dataset.sidebarView;

  if (sidebar && shown(sidebar)) {
    // 2. The sidebar never scrolls sideways (the double scrollbars in the report).
    if (sidebar.scrollWidth > sidebar.clientWidth + 1) problems.push(`sidebar scrolls sideways: ${sidebar.scrollWidth}px in ${sidebar.clientWidth}px`);
    if (view === 'overlay') {
      // 3. Over the pane, the sidebar is opaque and a scrim covers what is behind it.
      const bg = getComputedStyle(sidebar).backgroundColor;
      if (alpha(bg) < 1) problems.push(`sidebar over the pane is see-through (${bg})`);
      const scrim = document.querySelector('.shell-scrim');
      if (!shown(scrim)) problems.push('sidebar over the pane has no scrim');
    } else if (pane && overlaps(rect(sidebar), rect(pane))) {
      // 4. Beside the pane, the sidebar and the pane never share pixels.
      problems.push(`sidebar (${view}) overlaps the pane: ${JSON.stringify(rect(sidebar))} / ${JSON.stringify(rect(pane))}`);
    }
  }

  // 5. Every control and header the person needs is on screen, not cut off, and on top (nothing drawn over it).
  //    Skipped behind an open overlay (sidebar or sheet), where covering the pane is the point.
  const covered = view === 'overlay' || document.querySelector('.sheet-backdrop:not([hidden]), dialog[open]');
  const targets = document.querySelectorAll(
    '.shell-toolbar button, .shell-toolbar .shell-title, .kf-desk-table th, .kf-desk-table tbody tr:first-child td, .shell-sidebar .kf-desk-sidebar-item, .wtb button, .wtb input',
  );
  for (const el of targets) {
    if (!shown(el)) continue;
    if (el.closest('[hidden], [inert]')) continue;
    const r = rect(el);
    if (r.left < -1 || r.right > vw + 1 || r.top < -1 || r.bottom > vh + 1) {
      // Rows below the fold are fine (they scroll); sideways is not.
      if (r.left < -1 || r.right > vw + 1) problems.push(`off screen sideways: ${name(el)}`);
      continue;
    }
    // Cut off by a scrolling or clipping ancestor (a column under the inspector, say).
    for (let a = el.parentElement; a && a !== document.body; a = a.parentElement) {
      const cs = getComputedStyle(a);
      if (cs.overflowX === 'visible' && cs.overflowY === 'visible') continue;
      const ar = rect(a);
      if (r.left < ar.left - 1 || r.right > ar.right + 1) {
        problems.push(`cut off sideways by ${name(a).slice(0, 40)}: ${name(el)}`);
        break;
      }
    }
    if (covered && !el.closest('.shell-sidebar')) continue;
    const cx = Math.min(Math.max(r.left + r.width / 2, 0), vw - 1);
    const cy = Math.min(Math.max(r.top + r.height / 2, 0), vh - 1);
    const top = document.elementFromPoint(cx, cy);
    // A sticky bar (the pair's button row) over rows that scroll under it is the fold, not an overlap.
    const sticky = (n) => { for (let a = n; a && a !== document.body; a = a.parentElement) if (getComputedStyle(a).position === 'sticky') return a; return null; };
    if (top && top !== el && !el.contains(top) && !top.contains(el) && !top.closest('.toast-region, [role="tooltip"], .shell-menu') && !(sticky(top) && !sticky(el))) {
      problems.push(`${name(el)} is under ${name(top)}`);
    }
  }

  // 6. Button labels fit their buttons (text cut off by a fixed width).
  for (const b of document.querySelectorAll('button')) {
    if (!shown(b) || b.closest('[hidden], [inert]')) continue;
    const cs = getComputedStyle(b);
    if (cs.overflow === 'visible' || cs.textOverflow === 'ellipsis') continue;
    if (b.scrollWidth > b.clientWidth + 2) problems.push(`label cut off: ${name(b)}`);
  }
  // 7. The page heading shows once: the toolbar names the screen, so an in-page heading with the same words is a
  //    repeat (a screen reader still gets it from a .sr-title heading, which is not drawn).
  const title = document.querySelector(".shell-toolbar .shell-title")?.textContent.trim();
  if (title) {
    for (const hd of document.querySelectorAll(".main h1, .main h2")) {
      if (!shown(hd) || hd.closest("[hidden], [inert]") || hd.classList.contains("sr-title")) continue;
      if (rect(hd).width <= 1 || rect(hd).height <= 1) continue;
      if (hd.textContent.trim() === title) problems.push(`heading shown twice: ${name(hd)}`);
    }
  }

  // 8. A short label (a fact's name, a column header, a field label) stays on one line.
  for (const el of document.querySelectorAll("dt, th, .history-filter-label, .form-label")) {
    if (!shown(el) || el.closest("[hidden], [inert]")) continue;
    const text = el.textContent.trim();
    if (!text || text.length > 24) continue;
    // The lines its text sits on (text only: an icon beside it is not a line), a few pixels apart counting as one.
    const tops = [];
    const walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walk.nextNode(); n; n = walk.nextNode()) {
      if (!n.textContent.trim()) continue;
      const range = document.createRange();
      range.selectNodeContents(n);
      for (const r of range.getClientRects()) if (r.width > 0 && !tops.some((t) => Math.abs(t - r.top) < r.height / 2)) tops.push(r.top);
    }
    if (tops.length > 1) problems.push(`label wraps onto ${tops.length} lines: ${name(el)}`);
  }
  return problems;
})();
