# Website

Static site, zero build step, hosted on Cloudflare Pages (free tier).

## Pages

- `index.html` — home with CTAs (download, browse projects, apply).
- `projects.html` — who accepts donations; fetches the live `registry.json`
  release artifact at runtime with a graceful fallback.
- `apply.html` — project application form. POSTs to `/api/apply` when the
  endpoint exists; until then, prefills a GitHub application issue.
- `contributions.html` — contributions tracker (placeholder rows until the
  first landed PRs).

## Deploy (Cloudflare Pages, ~$0)

Option A — dashboard:

1. Cloudflare dashboard → Pages → Connect to Git → `ntindle/don-a-token`.
2. Build settings: framework preset None, build command empty,
   output directory `website`.
3. Deploy. Attach a custom domain when ready (DNS + SSL included).

Option B — CLI:

```sh
npx wrangler pages deploy website --project-name don-a-token
```

## Launch checklist

- [ ] Social handles: Bluesky, X/Twitter, Discord invite (footer links are
      `TODO` placeholders on every page).
- [ ] `/api/apply` endpoint: Pages Function + Turnstile, storing to D1 or
      forwarding to email. Form already degrades to GitHub issues.
- [ ] Tracker data source: decide where landed-PR records live (D1 vs.
      generated JSON artifact) and wire `contributions.html` to it.
- [ ] Custom domain + analytics (Cloudflare Web Analytics is free).
- [ ] Visual redesign pass once content settles (planned follow-up).

## Local preview

Any static server works, e.g. `python -m http.server` from this directory.
