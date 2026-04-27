# zudo-front-builder

`zfb` (zudo-front-builder) is a frontend build orchestrator. The repository is currently in scaffolding phase — the Rust workspace, placeholder bin crate, basic tooling, the docs site, and Cloudflare Pages CI are wired up. The actual orchestration logic lands in subsequent epics.

## What zfb is

zfb is the **engine**: router, renderer, content pipeline, and the
small set of build-time primitives (frontmatter extraction, content
collections, `paths()`, MDX directive registry, non-HTML page emission,
and the `PageMeta` head/asset contract) that a framework can build
on. Frameworks like a future `zudo-doc-v2` sit on top of these
primitives and own the opinionated layer — sidebar generation, search,
theming, blog conventions, i18n routing, versioning UI, and so on.

The boundary between the two is fixed in [ADR-003: Engine vs framework boundary](./docs/architecture/adr-003-engine-vs-framework.md).

The docs site is published at <https://takazudomodular.com/pj/zudo-front-builder/>.

## Docs site

The documentation site lives under [`docs/`](./docs) and is built with [zudo-doc](https://github.com/zudolab/zudo-doc) (Astro + MDX + Tailwind v4). Once published, it is served at <https://takazudomodular.com/pj/zudo-front-builder/>.

Local commands (run from the repo root):

```sh
pnpm docs:install        # install docs workspace deps
pnpm docs:dev            # start the Astro dev server
pnpm docs:build          # static build into docs/dist/
pnpm docs:preview        # preview the built site
pnpm docs:check          # astro check (type/content validation)
```

## License

MIT — see [LICENSE](./LICENSE).
