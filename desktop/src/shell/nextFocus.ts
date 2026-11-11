// Where the focus goes when rows leave a list (#66, #67): to the row after the last one that left, else the row
// before the first, else nowhere. Resolving a problem or restoring files moves on to the next, as Mail does.

export function nextAfter(order: readonly string[], leaving: readonly string[]): string | null {
  const gone = new Set(leaving);
  const at = order.map((id, i) => (gone.has(id) ? i : -1)).filter((i) => i >= 0);
  if (at.length === 0) return null;
  const after = order.slice(Math.max(...at) + 1).find((id) => !gone.has(id));
  if (after !== undefined) return after;
  const before = order.slice(0, Math.min(...at)).reverse().find((id) => !gone.has(id));
  return before ?? null;
}

/** What has the focus: its tag and the `data-key` the shell finds it again by (its own or its nearest ancestor's). */
export interface Focused {
  tag: string;
  key: string | null;
}

export function focusedNow(): Focused | null {
  const a = document.activeElement as HTMLElement | null;
  if (!a) return null;
  return { tag: a.tagName, key: a.closest<HTMLElement>('[data-key]')?.dataset.key ?? null };
}

/**
 * Whether a screen may still put the focus on the row it chose after an action (the next row after a restore or a
 * delete, the next problem after an answer). A re-render hands the focus back by `data-key`, so it can land on the
 * very control that was used (`from`: the next row has the same footer buttons) or, when that control is gone, on
 * the heading; neither is where the person put it. Anywhere else is, and the focus stays there.
 */
export function mayTakeFocus(active: Focused | null, from: string | null): boolean {
  if (!active || active.tag === 'BODY' || active.tag === 'H1') return true;
  return from !== null && active.key === from;
}
