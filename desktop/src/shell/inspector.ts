// The inspector (#63, design D4): a trailing pane, 260–320px, that shows the selection. ⌥⌘I / Ctrl+I opens and
// closes it, and the window remembers which. With one pair selected it shows that pair; with none, how the pairs
// stand. Later screens put their own content here through `app.setInspector`.

import { t, lang } from '../i18n';
import { failureText, formatRelative, pairFacts, isolatePath, pairStatus, progressFraction, progressText, STATUS_STATES, statusLabel } from '../format';
import { h, line, statusGlyph } from '../ui';
import type { App } from '../app';
import type { PairState } from '../types';

export function renderInspector(app: App, el: HTMLElement): void {
  const custom = app.inspectorContent();
  if (custom) {
    el.replaceChildren(custom);
    return;
  }
  const selected = app.selectedPairs();
  el.replaceChildren(selected.length === 1 ? pairInspector(app, selected[0]!) : summary(app, selected));
}

function heading(text: string): HTMLElement {
  // A data-key, so a live update that rebuilds the inspector keeps the focus here (⌥⌘I, Ctrl+I and F6 put it here).
  return h('h2', { id: 'shell-inspector-title', class: 'shell-insp-title', tabindex: '-1', 'data-key': 'insp-title' }, text);
}

function pairInspector(app: App, s: PairState): HTMLElement {
  const v = pairStatus(s);
  const p = s.pair;
  const route = p.route ?? 'drive';
  const f = s.progress ? progressFraction(s.progress) : null;
  const when = p.status.last_success_at;
  return h(
    'div',
    { class: 'shell-insp' },
    h('div', { class: 'shell-insp-group' }, heading(p.name), h('p', { class: 'path shell-insp-path' }, isolatePath(p.local_root))),
    h(
      'div',
      { class: 'shell-insp-group' },
      statusGlyph(v, { label: true, pairId: p.id }),
      s.running && s.progress
        ? h(
            'div',
            { class: 'progress-box', 'data-progress-for': p.id },
            h(
              'div',
              {
                class: `kf-progress${f === null ? ' indeterminate' : ''}`,
                role: 'progressbar',
                'aria-label': p.name,
                'aria-valuemin': '0',
                'aria-valuemax': '100',
                'aria-valuenow': f === null ? undefined : String(Math.round(f * 100)),
              },
              h('span', { style: `inline-size:${f === null ? 30 : Math.round(f * 100)}%` }),
            ),
            h('p', { class: 'progress-text shell-insp-caption' }, progressText(s.progress, lang())),
          )
        : h(
            'p',
            { class: 'shell-insp-caption' },
            when ? t('pairs.last_synced', { when: formatRelative(when, Date.now(), lang()) }) : t('pairs.never_synced'),
          ),
      // A failed pair says why, in words (the codes stay in Diagnostics).
      v.state === 'failed'
        ? h('p', { class: 'shell-insp-caption', dir: 'auto' }, failureText(p.status.last_error_kind, p.status.last_error, lang()) ?? t('banner.failed_body'))
        : null,
    ),
    // Size and skipped (#103), only when the daemon reports them: an older one leaves them out.
    ...pairFacts(s, lang()).map((f) => h('div', { class: 'shell-insp-group' }, h('h3', { class: 'shell-insp-label' }, f.label), h('p', { class: 'shell-insp-num' }, f.value), f.hint ? h('p', { class: 'shell-insp-caption' }, f.hint) : null)),
    h(
      'div',
      { class: 'shell-insp-group' },
      h('h3', { class: 'shell-insp-label' }, t('inspector.goes_to')),
      h(
        'ul',
        { class: 'shell-insp-box', role: 'list' },
        route !== 'devices'
          ? h('li', null, line('cloud'), h('div', null, h('p', null, 'Google Drive'), h('p', { class: 'path shell-insp-caption' }, isolatePath(p.remote_path))))
          : null,
        route !== 'drive' ? h('li', null, line('bolt'), h('div', null, h('p', null, t('inspector.devices')))) : null,
      ),
    ),
    h(
      'div',
      { class: 'shell-insp-actions' },
      h(
        'button',
        { type: 'button', class: 'kf-desk-btn', 'data-key': 'insp-pause', onclick: () => app.setPaused([s], !p.paused) },
        line(p.paused ? 'play' : 'pause'),
        t(p.paused ? 'pairs.resume' : 'pairs.pause'),
      ),
      h(
        'button',
        { type: 'button', class: 'kf-desk-btn', 'data-key': 'insp-reveal', onclick: () => app.showInFolder(p.id) },
        line('reveal'),
        app.revealLabel(),
      ),
    ),
  );
}

/** No single pair selected: how many pairs (all, or the several selected) are in each state. */
function summary(app: App, selected: PairState[]): HTMLElement {
  const several = selected.length > 1;
  const pairs = several ? selected : app.conn.state === 'connected' ? app.status?.pairs ?? [] : [];
  const views = pairs.map(pairStatus);
  const rows = STATUS_STATES.map((state) => ({ state, n: views.filter((v) => v.state === state).length })).filter((r) => r.n > 0);
  return h(
    'div',
    { class: 'shell-insp' },
    h('div', { class: 'shell-insp-group' }, heading(several ? t('pairs.selected', { count: selected.length }) : t('inspector.summary'))),
    rows.length
      ? h(
          'ul',
          { class: 'shell-insp-box', role: 'list' },
          rows.map((r) =>
            h(
              'li',
              null,
              statusGlyph({ state: r.state, paused: false, percent: null, count: 0 }),
              h('p', { class: 'shell-insp-grow' }, statusLabel({ state: r.state, paused: false, percent: null, count: 1 }, lang())),
              h('span', { class: 'shell-insp-count' }, String(r.n)),
            ),
          ),
        )
      : null,
    several ? null : h('p', { class: 'shell-insp-caption' }, t('inspector.empty')),
  );
}
