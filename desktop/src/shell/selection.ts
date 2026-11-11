// A list's selection (#64), as macOS and Windows lists keep it: a click selects one row, ⌘-click (macOS) or
// Ctrl-click (Windows) adds or removes one, Shift-click selects the range from the anchor, ⌘A / Ctrl+A selects
// every row. The arrows move the focus and the selection with it; with Shift they extend the range. Pure, so the
// All pairs table and later lists (#67) share it and it is unit-tested.

export interface Selection {
  /** Selected ids, in the list's order. */
  ids: string[];
  /** Where a Shift range starts: the last row clicked or toggled. */
  anchor: string | null;
  /** The row with the keyboard focus. */
  focus: string | null;
}

export const NO_SELECTION: Selection = { ids: [], anchor: null, focus: null };

/** Keeps ids in the list's order and drops any the list no longer has. */
function ordered(order: readonly string[], ids: Iterable<string>): string[] {
  const set = new Set(ids);
  return order.filter((id) => set.has(id));
}

/** A plain click, or a plain arrow: just this row. */
export function selectOne(id: string): Selection {
  return { ids: [id], anchor: id, focus: id };
}

/** ⌘-click or Ctrl-click: adds the row, or removes it if it was selected. */
export function toggle(sel: Selection, order: readonly string[], id: string): Selection {
  const has = sel.ids.includes(id);
  const ids = has ? sel.ids.filter((x) => x !== id) : [...sel.ids, id];
  return { ids: ordered(order, ids), anchor: id, focus: id };
}

/** Shift-click or Shift+arrow: every row from the anchor to this one; the anchor stays. */
export function extendTo(sel: Selection, order: readonly string[], id: string): Selection {
  const anchor = sel.anchor !== null && order.includes(sel.anchor) ? sel.anchor : id;
  const a = order.indexOf(anchor);
  const b = order.indexOf(id);
  if (b < 0) return sel;
  const [from, to] = a <= b ? [a, b] : [b, a];
  return { ids: order.slice(from, to + 1), anchor, focus: id };
}

/** ⌘A or Ctrl+A. */
export function selectAll(order: readonly string[]): Selection {
  return { ids: [...order], anchor: order[0] ?? null, focus: order[0] ?? null };
}

/**
 * The arrows. `select`: the selection follows the focus (plain arrow); `extend`: Shift+arrow; `focus`: ⌘/Ctrl+arrow
 * moves only the focus, for a later ⌘/Ctrl+Space. `to` is a row offset, or 'first' / 'last' (Home and End).
 */
export function move(sel: Selection, order: readonly string[], to: number | 'first' | 'last', mode: 'select' | 'extend' | 'focus'): Selection {
  if (order.length === 0) return NO_SELECTION;
  const at = sel.focus !== null ? order.indexOf(sel.focus) : -1;
  let i: number;
  if (to === 'first') i = 0;
  else if (to === 'last') i = order.length - 1;
  else if (at < 0) i = to > 0 ? 0 : order.length - 1;
  else i = Math.min(order.length - 1, Math.max(0, at + to));
  const id = order[i]!;
  if (mode === 'extend') return extendTo(sel.anchor === null ? { ...sel, anchor: sel.focus ?? id } : sel, order, id);
  if (mode === 'focus') return { ...sel, focus: id };
  return selectOne(id);
}

/** After the list changed (a pair removed, a search typed): drops what is gone, keeps the rest. */
export function prune(sel: Selection, order: readonly string[]): Selection {
  const ids = ordered(order, sel.ids);
  const keep = (id: string | null) => (id !== null && order.includes(id) ? id : null);
  const focus = keep(sel.focus) ?? ids[0] ?? null;
  return { ids, anchor: keep(sel.anchor), focus };
}

export function sameIds(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}
