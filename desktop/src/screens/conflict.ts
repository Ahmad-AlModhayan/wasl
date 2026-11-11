// A conflict in Problems' detail pane (#66, design "Conflict"): both versions side by side, then the three
// answers the engine has: Keep both (the default, nothing lost), keep this device's, keep Drive's. Undo comes in
// the toast and on ⌘Z / Ctrl+Z (Problems keeps the last answer for it).
//
// The daemon tells the size and the time of each version, not its contents, so the compare shows those; a line
// diff needs the files' text from the engine first.

import { api } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { formatBytes, formatDate, isolatePath } from '../format';
import { errorText, h, line, statusGlyph, toast } from '../ui';
import type { ConflictChoice, ConflictDetails, ConflictRecord, ConflictSide } from '../types';

function isMac(): boolean {
  return document.documentElement.dataset.kfPlatform === 'mac';
}

/** The last answer, for Undo: what to call, what to say, and the toast that offers it. */
export interface UndoStep {
  label: string;
  run: () => Promise<void>;
  toast: number;
}

/** What Problems knows of a conflict's two versions: read when it is selected. */
export type ConflictLoad =
  | { state: 'loading' }
  | { state: 'ok'; details: ConflictDetails | null }
  | { state: 'error'; message: string };

/** Conflicts with an answer on its way: their answers are off everywhere (buttons and context menu) until it lands. */
const answering = new Set<string>();

export function isAnswering(id: string): boolean {
  return answering.has(id);
}

function setAnswering(id: string, on: boolean): void {
  if (on) answering.add(id);
  else answering.delete(id);
  for (const b of document.querySelectorAll<HTMLButtonElement>(`[data-conflict-answers="${CSS.escape(id)}"] button`)) b.disabled = on;
}

/** Answers a conflict; returns the Undo step, or null when the answer failed (the toast says why) or one is on its way. */
export async function resolveConflict(c: ConflictRecord, choice: ConflictChoice, label: string, onUndo?: () => void): Promise<UndoStep | null> {
  if (answering.has(c.id)) return null;
  setAnswering(c.id, true);
  try {
    const undo = await api.resolveConflict(c.id, choice);
    const step: UndoStep = { label, run: () => api.undoConflict(undo), toast: 0 };
    step.toast = toast(t('problems.resolved', { choice: label }), { label: t('problems.undo'), run: () => (onUndo ? onUndo() : void runUndo(step)) });
    return step;
  } catch (e) {
    toast(errorText(e));
    return null;
  } finally {
    setAnswering(c.id, false);
  }
}

export async function runUndo(step: UndoStep): Promise<void> {
  try {
    await step.run();
    toast(t('problems.undone'));
  } catch (e) {
    toast(errorText(e));
  }
}

/** The words for each answer, the default last, on both platforms (#124). */
export function conflictChoices(): Array<{ choice: ConflictChoice; label: string; primary: boolean }> {
  const both = { choice: 'KeepBoth' as const, label: t('problems.keep_both'), primary: true };
  const device = { choice: 'KeepDevice' as const, label: t(isMac() ? 'conflict.keep_mac' : 'conflict.keep_pc'), primary: false };
  const drive = { choice: 'KeepDrive' as const, label: t('conflict.keep_drive'), primary: false };
  return [device, drive, both];
}

function side(app: App, which: 'device' | 'drive', load: ConflictLoad, s: ConflictSide | null, newer: boolean): HTMLElement {
  const name = which === 'drive' ? t('conflict.drive') : t(isMac() ? 'detail.this_mac' : 'detail.this_pc');
  return h(
    'section',
    { class: 'review-side', 'aria-label': name },
    h(
      'header',
      { class: 'review-side-head' },
      line(which === 'drive' ? 'cloud' : 'laptop'),
      h('span', { class: 'review-side-name' }, name),
      newer ? h('span', { class: 'review-newer' }, t('conflict.newer')) : null,
    ),
    load.state === 'loading'
      ? h('p', { class: 'review-muted', role: 'status' }, t('conflict.reading'))
      : load.state === 'error'
        ? h('p', { class: 'review-muted', dir: 'auto' }, t('conflict.unreadable', { message: load.message }))
        : s
          ? h(
              'dl',
              { class: 'review-facts' },
              h('dt', null, t('deletions.col_size')),
              h('dd', { class: 'kf-tnum' }, formatBytes(s.size, lang())),
              h('dt', null, t('conflict.changed')),
              h('dd', { class: 'kf-tnum', title: formatDate(s.mtime_ms, lang()) }, app.ago(s.mtime_ms)),
            )
          : h('p', { class: 'review-muted' }, t('conflict.gone')),
  );
}

/** The detail pane for one conflict. `onAnswer` runs after the engine took the answer. */
export function conflictDetail(
  app: App,
  c: ConflictRecord,
  load: ConflictLoad,
  pairName: string,
  onAnswer: (step: UndoStep) => void,
  onUndo?: () => void,
): HTMLElement {
  const d = load.state === 'ok' ? load.details : null;
  const name = d?.name ?? c.key.split('/').pop() ?? c.key;
  const where = d ? [d.folder, d.name].filter(Boolean).join('/') : c.key;
  const newer = d?.device && d.drive ? (d.device.mtime_ms > d.drive.mtime_ms ? 'device' : d.drive.mtime_ms > d.device.mtime_ms ? 'drive' : null) : null;
  const titleId = `conflict-${c.id}`;
  // All three are off while any answer is on its way (from here or the context menu).
  const buttons = conflictChoices().map((o) =>
    h(
      'button',
      {
        type: 'button',
        class: `kf-desk-btn${o.primary ? ' kf-desk-btn-default' : ''}`,
        'data-key': `conflict-${o.choice}`,
        disabled: answering.has(c.id),
        onclick: async () => {
          const step = await resolveConflict(c, o.choice, o.label, onUndo);
          if (step) onAnswer(step);
        },
      },
      o.label,
    ),
  );
  return h(
    'section',
    { class: 'review-detail', 'aria-labelledby': titleId },
    h(
      'div',
      { class: 'review-body' },
      h('div', { class: 'review-title-row' }, h('h2', { id: titleId, class: 'review-title', dir: 'auto', tabindex: '-1' }, name), statusGlyph({ state: 'conflict', paused: false, percent: null, count: 1 }, { label: true })),
      h(
        'p',
        { class: 'review-muted' },
        pairName ? [h('span', { dir: 'auto' }, pairName), ' · '] : null,
        h('bdi', { class: 'review-path', dir: 'ltr' }, isolatePath(where)),
      ),
      h('p', null, c.message && !d ? h('span', { dir: 'auto' }, c.message) : t('conflict.body')),
      h(
        'div',
        { class: 'review-compare', role: 'group', 'aria-label': t('conflict.compare_label') },
        side(app, 'device', load, d?.device ?? null, newer === 'device'),
        side(app, 'drive', load, d?.drive ?? null, newer === 'drive'),
      ),
    ),
    h('footer', { class: 'review-foot' }, h('p', { class: 'review-foot-note' }, t('conflict.footnote')), h('div', { class: 'review-foot-actions', 'data-conflict-answers': c.id }, buttons)),
  );
}
