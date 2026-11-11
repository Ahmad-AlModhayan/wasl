// English and Arabic (FR-R2, NFR-8). Every string lives in en.json / ar.json; plural forms are keys with a
// suffix from Intl.PluralRules (`_zero`, `_one`, `_two`, `_few`, `_many`, `_other`). Numbers are Western digits in
// both languages.

import en from './en.json';
import ar from './ar.json';

export type Lang = 'en' | 'ar';
export type Dict = Record<string, string>;
export type Params = Record<string, string | number>;

export const DICTS: Record<Lang, Dict> = { en, ar };

export const PLURAL_SUFFIX = /_(zero|one|two|few|many|other)$/;

/** The locale for Intl formatters: Arabic with Latin (Western) digits. */
export function locale(lang: Lang): string {
  return lang === 'ar' ? 'ar-u-nu-latn' : 'en';
}

/** The language for a stored preference ("system", "en", "ar") and the OS language. */
export function resolveLang(pref: string | null | undefined, systemLang: string): Lang {
  if (pref === 'en' || pref === 'ar') return pref;
  return systemLang.toLowerCase().startsWith('ar') ? 'ar' : 'en';
}

function interpolate(s: string, params: Params | undefined, lang: Lang): string {
  if (!params) return s;
  const nf = new Intl.NumberFormat(locale(lang));
  return s.replace(/\{(\w+)\}/g, (m, k: string) => {
    const v = params[k];
    if (v === undefined) return m;
    // First-strong isolate: an Arabic name in an English sentence (or the reverse) keeps its
    // own direction without dragging the punctuation around it.
    return typeof v === 'number' ? nf.format(v) : `\u2068${v}\u2069`;
  });
}

/** Looks `key` up in `lang`, falling back to English, then to the key itself. A numeric `count` picks a plural form. */
export function translate(lang: Lang, key: string, params?: Params): string {
  const dicts = [DICTS[lang], DICTS.en];
  const count = params?.count;
  if (typeof count === 'number') {
    const rule = new Intl.PluralRules(locale(lang)).select(count);
    for (const d of dicts) {
      const s = d[`${key}_${rule}`] ?? d[`${key}_other`];
      if (s !== undefined) return interpolate(s, params, lang);
    }
  }
  for (const d of dicts) {
    const s = d[key];
    if (s !== undefined) return interpolate(s, params, lang);
  }
  return key;
}

let current: Lang = 'en';

export function setLang(lang: Lang): void {
  current = lang;
  if (typeof document !== 'undefined') {
    document.documentElement.lang = lang;
    document.documentElement.dir = lang === 'ar' ? 'rtl' : 'ltr';
  }
}

export function lang(): Lang {
  return current;
}

export function t(key: string, params?: Params): string {
  return translate(current, key, params);
}
