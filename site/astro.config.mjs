// @ts-check
// wasl.kahflabs.com. The family look comes from @kahf/design; Wasl's own touches are in src/styles/site.css.
import { defineConfig } from 'astro/config';
import kahf from '@kahf/design/integration';

export default defineConfig({
  site: process.env.PUBLIC_SITE_URL || 'https://wasl.kahflabs.com',
  trailingSlash: 'ignore',
  build: { format: 'directory', inlineStylesheets: 'never' },
  compressHTML: true,
  integrations: [kahf({ styles: '/src/styles/site.css' })],
  // Scripts ship as files so the Content-Security-Policy can stay script-src 'self'.
  vite: { build: { assetsInlineLimit: 0 } },
});
