# Placeholder website

Next.js landing and download pages. The production build is a static export;
it needs no Node.js server, Workers adapter, or Pages Functions.

## Local development

Use Node.js 22, then run from `site/`:

```sh
npm ci
npm run dev
```

## Deploy to Cloudflare Pages

Commit and push the website configuration to GitHub. In Cloudflare's dashboard,
open **Workers & Pages → Create application → Pages → Import an existing Git
repository** and select `fr-anz/placeholder`.

Use these build settings (paths are relative to the selected root directory):

| Setting | Value |
| --- | --- |
| Production branch | `main` |
| Root directory | `site` |
| Framework preset | Next.js (Static HTML Export) |
| Build command | `npm run build` |
| Build output directory | `out` |

Pages installs dependencies automatically. `site/.node-version` selects Node.js
22. Do not use `.next` as the output directory or add a Worker deploy command.

If using a custom domain, set the build environment variable
`NEXT_PUBLIC_SITE_URL` to its full origin, for example `https://example.com`.
Without it, the build uses Cloudflare's `CF_PAGES_URL` for social image URLs.
After deploying, attach the domain under the Pages project's **Custom domains**
tab. Rebuild after changing the origin.

Select **Save and Deploy**. Pages provides a `*.pages.dev` URL and automatically
deploys subsequent pushes to `main`. Check both `/` and `/download` directly,
then follow the download button from the homepage.

See [Cloudflare's Next.js static site guide](https://developers.cloudflare.com/pages/framework-guides/nextjs/deploy-a-static-nextjs-site/).

## Verify the production export locally

```sh
npm run lint
npm run build
npx wrangler@4 pages dev out
```

The build writes `out/index.html`, `out/download.html`, `out/404.html`, and the
supporting assets. `next start` is not used with a static export.

Browser checks against a running local site:

```sh
SITE_URL=http://localhost:8788 CHROME_BIN=/path/to/chromium npm run test:diagram
```

The download buttons point to GitHub release assets configured in `lib/site.ts`;
Cloudflare hosts the website, while GitHub hosts the application installers.
