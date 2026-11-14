import type { Product } from '@kahf/design/lib/product';
import { kahf } from '@kahf/design/lib/product';

/** Wasl's site settings. Wasl's core sync engine is AGPL-3.0-only with Apache-2.0 protocol/CLI layers (see LICENSE and docs/OPEN_MODEL.md); the apps are on request until they reach Google Play.
    playUrl: set it then, and every "Ask for access" becomes "Get it on Google Play". */
export const playUrl = '';

const get = {
  en: playUrl ? { label: 'Get it on Google Play', href: playUrl } : { label: 'Ask for access', href: `${kahf('en', 'contact/')}?topic=early` },
  ar: playUrl ? { label: 'حمّله من Google Play', href: playUrl } : { label: 'اطلب الوصول', href: `${kahf('ar', 'contact/')}?topic=early` },
};

export const P: Product = {
  id: 'wasl',
  // The site's own address. PUBLIC_SITE_URL overrides it when the site is served from another domain (a preview, for example).
  url: import.meta.env.PUBLIC_SITE_URL || 'https://wasl.kahflabs.com',
  rootLang: 'en',
  scope: 'kf-scope-wasl',
  name: { en: 'Wasl', ar: 'وصل' },
  line: {
    en: 'Folders on your phone and on Google Drive, kept the same, safely. For Android.',
    ar: 'مجلدات هاتفك ومجلدات Google Drive، متطابقةً بأمان. لأندرويد.',
  },
  pages: ['privacy', 'terms'],
  nav: {
    en: [{ label: 'How it works', href: '#how' }, { label: 'Safety', href: '#safety' }, { label: 'Your Drive', href: '#drive' }],
    ar: [{ label: 'كيف يعمل', href: '#how' }, { label: 'الأمان', href: '#safety' }, { label: 'حسابك في Drive', href: '#drive' }],
  },
  cta: get,
  elsewhere: { en: [], ar: [] },
  email: 'hello@kahflabs.com',
};
export const GET = get;
