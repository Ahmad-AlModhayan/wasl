// Preview (FR-F1): what a sync would do, before anything happens. New pairs wait here for "Start sync".

import { api } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { formatBytes } from '../format';
import { button, errorBanner, errorText, h, line, toast } from '../ui';
import type { PlanSummary } from '../types';
import { guardReason } from './problemsModel';

export async function previewScreen(app: App, id: string): Promise<HTMLElement> {
  const name = app.status?.pairs.find((p) => p.pair.id === id)?.pair.name ?? '';
  const back = button(t('preview.back'), () => app.go({ name: 'pairs' }), { kind: 'plain', icon: 'back', mirror: true, key: 'preview-back' });
  const head = h('header', { class: 'screen-head' }, h('div', null, back, h('h1', { tabindex: '-1' }, t('preview.title', { name }))));
  const screen = h('div', { class: 'screen' }, head);

  let preview;
  try {
    preview = await api.preview(id);
  } catch (e) {
    screen.append(errorBanner(e, () => app.rerender()));
    return screen;
  }
  const s = preview.plan.summary;
  const rows = summaryRows(s);
  screen.append(
    h('p', { class: 'kf-muted' }, t('preview.intro')),
    rows.length > 0
      ? h(
          'dl',
          { class: 'kf-group summary' },
          rows.map(([label, files, bytes]) =>
            h(
              'div',
              { class: 'kf-row summary-row' },
              h('dt', { class: 'kf-row-title' }, label),
              h('dd', { class: 'kf-row-value kf-tnum' }, bytes > 0 ? `${t('preview.files', { count: files })} · ${formatBytes(bytes, lang())}` : t('preview.files', { count: files })),
            ),
          ),
        )
      : h('p', { class: 'kf-card' }, t('preview.nothing')),
  );
  // The engine's notes already carry the plan's warnings: each is shown once.
  const warnings = [...new Set([...s.warnings, ...preview.notes])];
  if (preview.plan.blocked && s.blockedReason) {
    screen.append(
      h('div', { class: 'kf-banner kf-tone-attention', role: 'note' },
        h('div', { class: 'kf-banner-icon', 'aria-hidden': 'true' }, line('warning', { size: 20 })),
        h('p', { class: 'kf-banner-title' }, t('preview.blocked', { reason: guardReason(lang(), s.blockedReason) }))),
    );
  }
  if (warnings.length > 0) {
    screen.append(h('section', { class: 'kf-card' }, h('h2', { class: 'kf-card-title' }, t('preview.warnings')), h('ul', { class: 'plain-list', dir: 'auto' }, warnings.map((w) => h('li', null, w)))));
  }
  if (s.sampleDeletes.length > 0) {
    screen.append(
      h('section', { class: 'kf-card' }, h('h2', { class: 'kf-card-title' }, t('preview.sample_deletes')), h('ul', { class: 'plain-list path' }, s.sampleDeletes.map((p) => h('li', { dir: 'auto' }, p)))),
    );
  }
  const start = button(
    t('preview.start'),
    async () => {
      start.disabled = true;
      try {
        toast(t('preview.started', { name }));
        app.go({ name: 'pairs' });
        await api.sync(id, 'preview');
      } catch (e) {
        toast(errorText(e));
      }
    },
    { kind: 'filled', icon: 'sync', key: 'preview-start', small: false },
  );
  screen.append(h('div', { class: 'screen-foot' }, start));
  return screen;
}

/** [label, files, bytes] for each kind of change the plan makes. */
function summaryRows(s: PlanSummary): Array<[string, number, number]> {
  const rows: Array<[string, number, number]> = [
    [t('preview.uploads'), s.uploads, s.uploadBytes],
    [t('preview.downloads'), s.downloads, s.downloadBytes],
    [t('preview.exports'), s.exports, 0],
    [t('preview.delete_remote'), s.deleteRemote, 0],
    [t('preview.delete_local'), s.deleteLocal, 0],
    [t('preview.conflicts'), s.conflicts, 0],
  ];
  const changes = rows.filter(([, n]) => n > 0);
  if (changes.length > 0 && s.unchanged > 0) changes.push([t('preview.unchanged'), s.unchanged, 0]);
  return changes;
}
