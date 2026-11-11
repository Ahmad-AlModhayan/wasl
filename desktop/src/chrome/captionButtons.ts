// The caption buttons Wasl draws on Windows, where the window has no native frame: minimize, maximize or restore,
// and close, each a 46px full-height backplate. Glyphs come from Segoe Fluent Icons (Segoe MDL2 Assets on Windows
// 10, same code points); a browser on another system, like the dev mock on a Mac, has neither font and gets SVG
// look-alikes. Not in the tab order, like native caption buttons: the keyboard has Alt+Space, Win+arrows and
// Alt+F4. Narrator still finds them by name.

import { t } from '../i18n';
import { h } from '../ui';

export type CaptionKind = 'minimize' | 'maximize' | 'close';

/** Segoe Fluent Icons: ChromeMinimize, ChromeMaximize, ChromeRestore, ChromeClose. */
export const GLYPHS = { minimize: '', maximize: '', restore: '', close: '' } as const;

export type Face = keyof typeof GLYPHS;

/** What a caption button shows and is called: maximize becomes restore while the window is maximized. */
export function captionFace(kind: CaptionKind, maximized: boolean): Face {
  return kind === 'maximize' && maximized ? 'restore' : kind;
}

/** Windows has the icon font; any other system draws the glyphs as SVG. */
export function glyphSource(userAgent: string): 'font' | 'svg' {
  return /Windows NT/.test(userAgent) ? 'font' : 'svg';
}

// 10px look-alikes of the Fluent glyphs, 1px strokes.
const SVG_GLYPHS: Record<Face, string> = {
  minimize: '<path d="M0 5.5h10"/>',
  maximize: '<rect x=".5" y=".5" width="9" height="9" rx="1"/>',
  restore: '<rect x=".5" y="2.5" width="7" height="7" rx="1"/><path d="M2.5 2.5v-.5a1.5 1.5 0 0 1 1.5-1.5h4a1.5 1.5 0 0 1 1.5 1.5v4a1.5 1.5 0 0 1-1.5 1.5h-.5"/>',
  close: '<path d="M.5.5l9 9M9.5.5l-9 9"/>',
};

const KINDS: CaptionKind[] = ['minimize', 'maximize', 'close'];

export type CaptionActions = Record<CaptionKind, () => void>;

/** The three buttons, in reading order: the row's direction puts them at the right in English, the left in Arabic. */
export class CaptionButtons {
  readonly el: HTMLElement;
  private readonly buttons = new Map<CaptionKind, HTMLButtonElement>();
  private maximized = false;

  constructor(
    actions: CaptionActions,
    private readonly source: 'font' | 'svg',
  ) {
    this.el = h('div', { class: 'wtb-caps' });
    for (const kind of KINDS) {
      const b = h('button', {
        type: 'button',
        class: 'wtb-cap',
        'data-cap': kind,
        tabindex: '-1',
        // A click leaves keyboard focus where it was in the window, as a native caption button does.
        onmousedown: (e: MouseEvent) => e.preventDefault(),
        onclick: actions[kind],
      });
      this.buttons.set(kind, b);
      this.el.append(b);
    }
    this.render();
  }

  get isMaximized(): boolean {
    return this.maximized;
  }

  get maximize(): HTMLButtonElement {
    return this.buttons.get('maximize')!;
  }

  setMaximized(maximized: boolean): void {
    if (maximized === this.maximized) return;
    this.maximized = maximized;
    this.render();
  }

  /** Hover and press on maximize, reported by the native window over it (which takes its mouse input). */
  setMaximizeState(state: 'hover' | 'pressed' | null): void {
    if (state) this.maximize.dataset.state = state;
    else delete this.maximize.dataset.state;
  }

  /** Names and glyphs again, after a language change or maximize/restore. */
  render(): void {
    for (const [kind, b] of this.buttons) {
      const face = captionFace(kind, this.maximized);
      const name = t(`chrome.${face}`);
      b.setAttribute('aria-label', name);
      b.title = name;
      b.replaceChildren(this.glyph(face));
    }
  }

  private glyph(face: Face): Element {
    if (this.source === 'font') return h('span', { class: 'wtb-glyph', 'aria-hidden': 'true' }, GLYPHS[face]);
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('class', 'wtb-glyph');
    svg.setAttribute('viewBox', '0 0 10 10');
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('focusable', 'false');
    svg.innerHTML = SVG_GLYPHS[face]; // our own constant markup
    return svg;
  }
}
