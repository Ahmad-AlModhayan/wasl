import { describe, expect, it } from 'vitest';
import en from './en.json';
import ar from './ar.json';
import { PLURAL_SUFFIX, resolveLang, translate } from './index';

/** Keys with plural suffixes folded to their base, so Arabic's extra forms compare equal to English's. */
function baseKeys(dict: Record<string, string>): Set<string> {
  return new Set(Object.keys(dict).map((k) => k.replace(PLURAL_SUFFIX, '')));
}

function placeholders(s: string): string[] {
  return [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
}

describe('the string files', () => {
  it('have the same keys in English and Arabic', () => {
    const e = baseKeys(en);
    const a = baseKeys(ar);
    expect([...e].filter((k) => !a.has(k))).toEqual([]);
    expect([...a].filter((k) => !e.has(k))).toEqual([]);
  });

  it('give every plural an _other form in both languages', () => {
    for (const dict of [en, ar] as Array<Record<string, string>>) {
      const plurals = new Set(Object.keys(dict).filter((k) => PLURAL_SUFFIX.test(k)).map((k) => k.replace(PLURAL_SUFFIX, '')));
      for (const p of plurals) expect(dict[`${p}_other`], p).toBeDefined();
    }
  });

  it('use the same placeholders in both languages', () => {
    const arDict = ar as Record<string, string>;
    for (const [k, v] of Object.entries(en as Record<string, string>)) {
      if (PLURAL_SUFFIX.test(k)) continue; // Arabic one/two forms spell the number out
      expect(placeholders(arDict[k] ?? ''), k).toEqual(placeholders(v));
    }
  });

  it('use the glossary words in Arabic', () => {
    const all = Object.values(ar as Record<string, string>).join('\n');
    expect(all).not.toContain('زوج'); // a folder pair is ربط, never زوج
    expect(ar['nav.bin']).toBe('سلة المحذوفات');
    expect(ar['overall.up_to_date']).toBe('تمت المزامنة');
    expect(ar['overall.paused']).toBe('متوقف مؤقتًا');
    expect(ar['overall.needs_you']).toBe('بانتظارك');
    expect(ar['devices.this_computer']).toBe('هذا الحاسوب');
    expect(ar['pairing.paired']).toContain('الإقران'); // pairing a device is إقران
  });
});

describe('translate', () => {
  it('fills placeholders with Western digits in both languages', () => {
    expect(translate('en', 'history.uploaded', { count: 1234 })).toBe('Uploaded: 1,234');
    expect(translate('ar', 'history.uploaded', { count: 12 })).toBe('رُفع: 12');
  });

  it('picks plural forms by language', () => {
    expect(translate('en', 'preview.files', { count: 1 })).toBe('1 file');
    expect(translate('en', 'preview.files', { count: 3 })).toBe('3 files');
    expect(translate('ar', 'preview.files', { count: 1 })).toBe('ملف واحد');
    expect(translate('ar', 'preview.files', { count: 2 })).toBe('ملفان');
    expect(translate('ar', 'preview.files', { count: 5 })).toBe('5 ملفات');
    expect(translate('ar', 'preview.files', { count: 11 })).toBe('11 ملفًا');
  });

  it('falls back to English, then to the key', () => {
    expect(translate('ar', 'no.such.key')).toBe('no.such.key');
  });
});

describe('resolveLang', () => {
  it('follows an explicit choice, else the system language', () => {
    expect(resolveLang('ar', 'en-US')).toBe('ar');
    expect(resolveLang('en', 'ar-SA')).toBe('en');
    expect(resolveLang('system', 'ar-SA')).toBe('ar');
    expect(resolveLang(null, 'fr-FR')).toBe('en');
  });
});
