// Agent changes (docs/AGENT_CHANGES.md): what Claude Code, Gemini, Raqeeb and other agents changed in synced
// folders. Each changeset waits here, unsynced, until Accept (it syncs now) or Revert (every file back; the
// agent's versions go to the recycle bin).

import { api } from '../api';
import type { App } from '../app';
import { lang, t } from '../i18n';
import { formatBytes } from '../format';
import { button, chip, confirmDialog, copyText, empty, errorBanner, errorText, h, line, toast } from '../ui';
import type { AgentEntry, Changeset, ChangesetDetails, ChangesetStatus } from '../types';

export async function agentsScreen(app: App): Promise<HTMLElement> {
  const screen = h('div', { class: 'screen' }, h('header', { class: 'screen-head' }, h('h1', { class: 'sr-title', tabindex: '-1' }, t('agents.title'))));
  let all: Changeset[];
  try {
    all = await api.agentChangesets(true);
  } catch (e) {
    screen.append(errorBanner(e, () => app.rerender()));
    return screen;
  }
  const waiting = all.filter((c) => c.status === 'held' || c.status === 'open');
  const past = all.filter((c) => !waiting.includes(c)).slice(0, 20);
  if (all.length === 0) {
    screen.append(empty('spark', t('agents.empty_title'), t('agents.empty_body')));
    screen.append(await setupSection());
    return screen;
  }
  screen.append(h('p', { class: 'kf-muted' }, t('agents.intro')));
  if (waiting.length > 0) {
    const details = await Promise.all(waiting.map((c) => api.agentChangeset(c.id).catch(() => null)));
    screen.append(
      h('section', { class: 'problem-section', 'aria-labelledby': 'sec-agents-waiting' },
        h('h2', { id: 'sec-agents-waiting', class: 'section-title' }, t('agents.waiting')),
        h('p', { class: 'kf-muted' }, t('agents.accept_note')),
        h('ul', { class: 'problem-list', role: 'list' }, waiting.map((c, i) => card(app, c, details[i])))),
    );
  } else {
    screen.append(empty('synced', t('agents.none_waiting_title'), t('agents.none_waiting_body')));
  }
  screen.append(await setupSection());
  if (past.length > 0) {
    screen.append(
      h('section', { class: 'problem-section', 'aria-labelledby': 'sec-agents-past' },
        h('h2', { id: 'sec-agents-past', class: 'section-title' }, t('agents.past')),
        h('ul', { class: 'kf-group', role: 'list' }, past.map((c) => pastRow(app, c)))),
    );
  }
  return screen;
}

function agentName(c: Changeset): string {
  return c.actor.replace(/^agent:/, '');
}

function where(c: Changeset, pairName: string): string {
  return [pairName, c.scope || '/'].filter(Boolean).join(' · ');
}

function statusChip(c: Changeset): HTMLElement {
  const tone: Record<Changeset['status'], string> = {
    open: 'kf-tone-info',
    held: 'kf-tone-attention',
    accepted: 'kf-tone-positive',
    reverted: 'kf-tone-neutral',
    partly_reverted: 'kf-tone-attention',
  };
  return chip(t(`agents.status.${c.status}`), tone[c.status]);
}

function card(app: App, c: Changeset, d: ChangesetDetails | null): HTMLElement {
  const entries = d?.entries ?? [];
  const titleId = `agent-${c.id}`;
  const diffs = h('div', { class: 'agent-diffs' });
  let shown = false;
  const toggle = button(t('agents.show_changes'), () => {
    shown = !shown;
    toggle.textContent = t(shown ? 'agents.hide_changes' : 'agents.show_changes');
    if (shown) void loadDiffs(c, entries, diffs);
    else diffs.replaceChildren();
  }, { kind: 'plain', key: `diff-${c.id}` });
  const held = c.status === 'held';
  return h(
    'li',
    { class: 'kf-card problem agent-card', 'aria-labelledby': titleId },
    h('h3', { id: titleId, class: 'problem-title' }, line('eye', { size: 20 }), held ? t('agents.card_title', { agent: agentName(c), count: entries.length }) : t('agents.card_title_open', { agent: agentName(c) }), statusChip(c)),
    h('p', { class: 'kf-muted path', dir: 'auto' }, where(c, d?.pair_name ?? ''), ' · ', app.ago(c.started_at)),
    c.label ? h('p', { dir: 'auto' }, c.label) : null,
    held
      ? h('ul', { class: 'agent-files', role: 'list' }, entries.map((e) => fileRow(e)))
      : h('p', { class: 'kf-muted' }, t('agents.still_working')),
    held
      ? null
      : h('div', { class: 'card-actions' },
          button(t('agents.end'), () => void end(c), { kind: 'tinted', icon: 'check', key: `end-${c.id}` })),
    held ? diffs : null,
    held
      ? h('div', { class: 'card-actions' },
          button(t('agents.accept'), () => void accept(c), { kind: 'filled', icon: 'check', key: `accept-${c.id}` }),
          button(t('agents.revert'), () => void revert(c, entries.length), { kind: 'danger-tinted', icon: 'retry', key: `revert-${c.id}` }),
          toggle)
      : null,
  );
}

function fileRow(e: AgentEntry): HTMLElement {
  const tone = { added: 'kf-tone-positive', changed: 'kf-tone-attention', deleted: 'kf-tone-critical' }[e.kind];
  const size =
    e.before_size != null && e.after_size != null
      ? `${formatBytes(e.before_size, lang())} → ${formatBytes(e.after_size, lang())}`
      : e.after_size != null
        ? formatBytes(e.after_size, lang())
        : e.before_size != null
          ? formatBytes(e.before_size, lang())
          : '';
  return h('li', { class: 'agent-file' },
    chip(t(`agents.kind.${e.kind}`), tone),
    h('span', { class: 'path', dir: 'ltr' }, e.is_dir ? `${e.rel}/` : e.rel),
    size ? h('span', { class: 'kf-muted kf-tnum', dir: 'ltr' }, size) : null);
}

async function loadDiffs(c: Changeset, entries: AgentEntry[], into: HTMLElement): Promise<void> {
  into.replaceChildren(h('p', { class: 'kf-muted' }, t('agents.loading_changes')));
  const parts: HTMLElement[] = [];
  for (const e of entries.filter((x) => !x.is_dir)) {
    let text: string;
    try {
      text = await api.agentDiff(c.id, e.rel);
    } catch (err) {
      text = errorText(err);
    }
    const lines = text.split('\n').map((line) => {
      const cls = line.startsWith('+') && !line.startsWith('+++') ? 'add' : line.startsWith('-') && !line.startsWith('---') ? 'del' : line.startsWith('@@') ? 'hunk' : '';
      return h('span', { class: `diff-line ${cls}` }, line);
    });
    parts.push(h('pre', { class: 'agent-diff', dir: 'ltr', tabindex: '0', 'aria-label': e.rel }, lines));
  }
  into.replaceChildren(...parts);
}

/** Ends a session whose agent stopped without saying so (a crash, a closed terminal). */
async function end(c: Changeset): Promise<void> {
  try {
    const r = await api.agentEnd(c.id);
    toast(r ? t('agents.ended', { agent: agentName(c) }) : t('agents.ended_nothing', { agent: agentName(c) }));
  } catch (e) {
    toast(errorText(e));
  }
}

/** How to connect Claude Code, Gemini CLI and other agents. */
async function setupSection(): Promise<HTMLElement> {
  const section = h('section', { class: 'problem-section', 'aria-labelledby': 'sec-agents-setup' },
    h('h2', { id: 'sec-agents-setup', class: 'section-title' }, t('agents.setup_title')));
  let setup;
  try {
    setup = await api.agentSetup();
  } catch (e) {
    section.append(h('p', { class: 'kf-muted' }, errorText(e)));
    return section;
  }
  const run = `"${setup.cli}" agent run --as gemini -- gemini`;
  section.append(
    h('div', { class: 'kf-card problem' },
      h('h3', { class: 'problem-title' }, t('agents.setup_claude')),
      h('p', { class: 'kf-muted' }, t('agents.setup_claude_body')),
      h('pre', { class: 'agent-diff', dir: 'ltr', tabindex: '0' }, setup.claude_code_hooks),
      h('div', { class: 'card-actions' },
        button(t('agents.copy_hooks'), () => void copyText(setup.claude_code_hooks), { kind: 'tinted', key: 'copy-hooks' }))),
    h('div', { class: 'kf-card problem' },
      h('h3', { class: 'problem-title' }, t('agents.setup_other')),
      h('p', { class: 'kf-muted' }, t('agents.setup_other_body')),
      h('pre', { class: 'agent-diff', dir: 'ltr', tabindex: '0' }, run),
      h('div', { class: 'card-actions' },
        button(t('agents.copy_command'), () => void copyText(run), { kind: 'tinted', key: 'copy-run' }))),
  );
  return section;
}

/** Where an accept stands while its confirmation is open: the changeset's status decides. */
export function acceptState(status: ChangesetStatus, now: number, deadline: number): 'accepted' | 'not_confirmed' | 'waiting' {
  if (status === 'accepted') return 'accepted';
  if (status !== 'held' || now >= deadline) return 'not_confirmed';
  return 'waiting';
}

/** The wait before an accept may be asked for again, in whole minutes (at least one). */
export function retryMinutes(secs: number): number {
  return Math.max(1, Math.ceil(secs / 60));
}

/**
 * Accept asks the service, which opens its own confirmation window and accepts only when the
 * person confirms there (docs/AGENT_CHANGES.md §5): this window is a client like any other, so its
 * button alone must not be enough. Says what is shown, then how it ended.
 */
async function accept(c: Changeset): Promise<void> {
  let asked;
  try {
    asked = await api.agentAccept(c.id);
  } catch (e) {
    toast(errorText(e));
    return;
  }
  if (asked.retry_in_secs) {
    // Declined or unanswered a moment ago: the service waits before it opens another dialog, so
    // an agent asking in a loop cannot keep one on screen (docs/AGENT_CHANGES.md §5).
    toast(t('agents.try_again', { agent: agentName(c), count: retryMinutes(asked.retry_in_secs) }));
    return;
  }
  toast(t('agents.confirm_in_dialog', { agent: agentName(c), code: asked.code }));
  const deadline = Date.now() + (asked.expires_in_secs + 5) * 1000;
  for (;;) {
    await new Promise((r) => setTimeout(r, 1000));
    let status: ChangesetStatus;
    try {
      status = (await api.agentChangeset(c.id)).changeset.status;
    } catch {
      return;
    }
    const state = acceptState(status, Date.now(), deadline);
    if (state === 'accepted') toast(t('agents.accepted', { agent: agentName(c) }));
    if (state === 'not_confirmed') toast(t('agents.not_confirmed', { agent: agentName(c) }));
    if (state !== 'waiting') return;
  }
}

async function revert(c: Changeset, count: number): Promise<void> {
  const ok = await confirmDialog({
    title: t('agents.revert_title', { agent: agentName(c) }),
    body: t('agents.revert_body', { count }),
    confirm: t('agents.revert'),
    danger: true,
  });
  if (!ok) return;
  try {
    const r = await api.agentRevert(c.id);
    if (r.failed.length > 0) toast(t('agents.revert_failed', { path: r.failed[0][0], message: r.failed[0][1] }));
    else if (r.skipped.length > 0) toast(t('agents.reverted_some', { count: r.reverted.length, skipped: r.skipped.length }));
    else toast(t('agents.reverted', { count: r.reverted.length }));
  } catch (e) {
    toast(errorText(e));
  }
}

function pastRow(app: App, c: Changeset): HTMLElement {
  return h('li', { class: 'kf-row' },
    line('history'),
    h('span', { class: 'kf-row-text' },
      h('span', { class: 'kf-row-title' }, agentName(c)),
      h('span', { class: 'kf-row-sub path', dir: 'auto' }, where(c, ''), ' · ', app.ago(c.decided_at ?? c.started_at))),
    statusChip(c));
}
