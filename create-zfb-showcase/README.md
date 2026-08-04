# create-zfb-showcase

Cloudflare Workers deploy config for the `create-zfb` scaffold showcase site
(`create-zfb.takazudomodular.com`). This directory holds **config only** —
no source, no `package.json`, no workspace membership.

## `dist/` is CI-generated — never commit it

The `dist/` this project's `wrangler.toml` serves (via `[assets] directory =
"./dist"`) is produced in CI by `scripts/smoke-packed-clean-room.sh` — the
same run that backs the required `Scaffold E2E (packed tarballs,
pre-publish)` check. It is **not** built or committed here.

The site itself is the **unmodified output of `npm create zfb@latest`**
(the `basic-blog` template, packed and installed exactly as an end user
would get it) — that's the point of the showcase: it proves what the
scaffold actually produces. The one addition is a banner injected at
**deploy time** by `scripts/showcase-inject-banner.mjs`, which is not part
of the scaffold output itself.

## Why no `package.json` / workspace membership

This directory has no dependencies of its own — it only points `wrangler`
at a `dist/` built elsewhere. Adding it to `pnpm-workspace.yaml` or giving
it a `package.json` would churn the lockfile for nothing.
