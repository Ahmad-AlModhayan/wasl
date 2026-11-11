// The notifications' words (#71) in the window's language, sent to Rust with the tray's (src-tauri/src/notify.rs
// fills in {name}, {device} and {count}, so they stay as they are here).

import type { NotifyText } from './api';
import { t } from './i18n';

export const NOTIFY_KINDS = ['conflict', 'failed', 'auth', 'deletion', 'offer', 'pairing', 'summary'] as const;

export function notifyText(): NotifyText {
  const words: Record<string, string> = {};
  for (const kind of NOTIFY_KINDS) {
    words[`${kind}Title`] = t(`notify.${kind}_title`);
    words[`${kind}Body`] = t(`notify.${kind}_body`);
  }
  return words as unknown as NotifyText;
}
