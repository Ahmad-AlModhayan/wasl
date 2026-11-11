// Native folder drop (#71). Tauri's drag-and-drop events carry real paths, which HTML5 drops don't. Dropping a
// folder anywhere on the window opens New pair with it filled in (D4); a file gets a message instead. The Add pair…
// button stays, so nothing needs dragging (WCAG 2.5.7). macOS also takes a folder dropped on the Dock icon
// (src-tauri/src/main.rs); the Windows taskbar and tray take no drops.

import { getCurrentWebview } from '@tauri-apps/api/webview';
import { api } from './api';
import { t } from './i18n';
import { h, line, toast } from './ui';

export type DropResult = { folder: string } | { error: 'not_folder' } | null;

/** The first folder among the dropped paths; a message when something was dropped but none is a folder. */
export async function folderOf(paths: string[], isFolder: (path: string) => Promise<boolean>): Promise<DropResult> {
  if (paths.length === 0) return null;
  for (const path of paths) if (await isFolder(path).catch(() => false)) return { folder: path };
  return { error: 'not_folder' };
}

/** What a dropped folder does now: opens New pair (or the first run's folder step), is kept for the first run's
 * folder step, or is refused with a reason (the service isn't ready, or a sheet or dialog is open). */
export type DropVerdict = 'take' | 'kept' | 'not_ready' | 'busy';

export function dropVerdict(s: { ready: boolean; dialogOpen: boolean; firstRunStep: string | null }): DropVerdict {
  if (!s.ready) return 'not_ready';
  if (s.dialogOpen) return 'busy';
  return s.firstRunStep === 'sign_in' ? 'kept' : 'take';
}

/** The toast for a verdict that doesn't simply open the folder; null for `take`. */
export function dropMessage(v: DropVerdict): string | null {
  return v === 'take' ? null : t(`drop.${v}`);
}

export interface DropHost {
  /** What a drop would do now. */
  verdict(): DropVerdict;
  takeFolder(folder: string): void;
  announce(text: string): void;
}

/** The full-window "Drop a folder to create a pair" overlay while a drag is over the window, announced once. */
export async function mountFolderDrop(host: DropHost): Promise<void> {
  const words = h('p', { class: 'drop-overlay-text' });
  const overlay = h('div', { class: 'drop-overlay', 'aria-hidden': 'true' }, h('div', { class: 'drop-overlay-panel' }, line('folder', { size: 36 }), words));
  overlay.hidden = true;
  document.body.append(overlay);
  const show = (on: boolean) => {
    if (overlay.hidden !== on) return;
    overlay.hidden = !on;
    if (!on) return;
    words.textContent = t('drop.overlay');
    host.announce(t('drop.overlay'));
  };
  await getCurrentWebview().onDragDropEvent(async (e) => {
    const p = e.payload;
    const verdict = host.verdict();
    const accepts = verdict === 'take' || verdict === 'kept';
    if (p.type === 'enter') show(accepts);
    else if (p.type === 'leave') show(false);
    else if (p.type === 'drop') {
      show(false);
      // Refused drops say why, so nothing is silently lost.
      if (!accepts) return void toast(dropMessage(verdict)!);
      const result = await folderOf(p.paths, api.isFolder);
      if (result && 'folder' in result) host.takeFolder(result.folder);
      else if (result) toast(t('drop.not_folder'));
    }
  });
}
