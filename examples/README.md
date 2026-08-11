# Examples Workspace Contract

> **Looking for a full, deployable example?** This directory holds in-repo
> scratch examples used to test published packages, not standalone projects.
> For the 9 standalone Cloudflare Workers/Pages example repositories with
> live demos, see the
> [Examples](https://takazudomodular.com/pj/zudo-front-builder/docs/guides/examples/)
> docs page.

The `examples/*` workspace is for copyable, runnable examples that exercise
published `@takazudo/*` packages the same way an external user would consume
them. Example packages are intentionally excluded from release build and T1
typecheck lanes, but root formatting still covers this directory.

## Package Shape

Each example directory must be named `zfb-example-<topic>`.

Each example `package.json` must set:

```json
{
  "name": "zfb-example-<topic>",
  "private": true
}
```

Examples are not published from this repository. `private: true` is required so
an accidental recursive publish cannot ship an example package.

## Dependencies

Examples must depend on published registry versions of ZFB packages, not local
workspace links. Never use the `workspace:` protocol in an example package.

Pin ZFB package dependencies to the current `next` dist-tag at the time the
example is created or refreshed. At this infrastructure change, `npm dist-tag
ls @takazudo/zfb` reports:

```text
latest: 0.1.0-next.78
next: 0.1.0-next.78
```

Use exact versions:

```json
{
  "dependencies": {
    "@takazudo/zfb": "0.1.0-next.78",
    "@takazudo/zfb-runtime": "0.1.0-next.78"
  }
}
```

If the example uses SSR on Cloudflare, also pin:

```json
{
  "dependencies": {
    "@takazudo/zfb-adapter-cloudflare": "0.1.0-next.78"
  }
}
```

Keep runtime dependencies minimal. Examples participate in the weekly
`pnpm audit --prod` posture, so every production dependency must be necessary
for the example itself.

## Scripts

Every example package must provide these scripts and no `test` script:

```json
{
  "scripts": {
    "predev": "rm -rf build dist .zfb output worker",
    "dev": "zfb dev",
    "build": "zfb build",
    "preview": "zfb preview",
    "typecheck": "zfb check"
  }
}
```

The `predev` script should remove the build and output directories used by that
example. Adjust the directory list only when the example uses different output
paths.

Do not add a `test` script. The repository-wide `pnpm -r test` lane should skip
examples because they do not provide tests.

## Per-Example README

Each example must include its own `README.md` with:

- A short description of the scenario the example demonstrates.
- Local run steps: `pnpm install`, `pnpm dev`, `pnpm build`, and `pnpm preview`. For an example with `prerender = false` routes that read Cloudflare bindings, qualify `pnpm dev`: it exercises rendering only, not bindings — `pnpm preview` (or `wrangler dev`) is the loop that actually runs the route. See [SSR and Cloudflare Bindings — Local development](https://takazudomodular.com/pj/zudo-front-builder/docs/guides/ssr-and-cloudflare-bindings/#local-development).
- Cloudflare provisioning steps when the example needs Cloudflare resources.
- Placeholder IDs in `wrangler.jsonc` or `wrangler.toml`, plus the exact
  `wrangler` commands needed to create real resources.

For Cloudflare-backed examples, keep placeholder values obvious:

```jsonc
{
  "d1_databases": [
    {
      "binding": "DB",
      "database_name": "zfb-example-<topic>",
      "database_id": "REPLACE_WITH_D1_DATABASE_ID"
    }
  ]
}
```

Document provisioning commands next to the placeholder:

```bash
pnpm exec wrangler d1 create zfb-example-<topic>
pnpm exec wrangler kv namespace create zfb-example-<topic>
pnpm exec wrangler r2 bucket create zfb-example-<topic>
```

Only include commands for resources the example actually uses.

## CI And Release Posture

Examples are intentionally skipped by T1 package typecheck and release build
filters:

```bash
pnpm -r --filter '!./examples/*' --if-present typecheck
pnpm -r --filter '!./examples/*' build
```

Examples are also skipped by `pnpm -r test` because they must not define `test`
scripts. Root `pnpm format:check` deliberately covers examples, so example
source, JSON, YAML, Markdown, and MDX must stay formatted.

Do not add per-example deploy workflows. An example README can document deploy
commands, but GitHub Actions deployment stays out of each example directory.

## Moving An Example Out

Examples should be easy to extract into their own repository:

1. Copy the `examples/zfb-example-<topic>` directory.
2. Create a new repository from that directory.
3. Run `pnpm install`.

Because dependencies are exact published registry versions and never
`workspace:` links, no additional workspace cleanup should be needed.
