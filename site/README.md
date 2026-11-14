# wasl.kahflabs.com

The Wasl website: static HTML, English and Arabic, no cookies, no trackers. Built on
@kahf/design, the Kahf family design system; Wasl's own touches live here.

**`@kahf/design` is a private Kahf Labs package**, installed from its private GitHub repository, so the site can't be
built from a public clone. Kahf Labs builds with read access to that repository (a token, as the CI secret
`KAHF_DESIGN_READ_TOKEN`; the same for a host such as Cloudflare Pages, Netlify or Vercel).

```sh
npm install
npm run dev        # http://localhost:4321
npm run build      # dist/
npm run preview
```

| File | What |
|---|---|
| `src/product.ts` | Name, links, call to action, Play Store link, CSS scope |
| `src/Home.astro` | The home page, both languages (copy in the `T` object at the top) |
| `src/pages/` | Routes: home, privacy, terms, 404, sitemap |
| `src/styles/site.css` | Wasl-only styling, loaded after the family CSS |
| `public/` | Icons, social images, screens (fonts and theme.js are copied in at build) |
| `kahf.assets.json` | Inputs for `npm run assets` (favicons, icons, manifest, og images) |

## Deploy

Any static host, with this folder as the base directory: Netlify (`netlify.toml`), Vercel (`vercel.json`, root
directory = this folder), or Cloudflare Pages (build `npm run build`, output `dist`). Point `wasl.kahflabs.com` at it.
