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

## Limits

`zfb build` reads every configured content collection, builds an
in-memory `ContentSnapshot` (one entry per `.md` / `.mdx` / `.tsx`
under each collection root), and embeds it into the worker bundle that
miniflare loads at build time. The full snapshot lives in V8 RAM for
the duration of the render pass — there is no streaming or sharding
today.

For the project sizes the engine targets (the docs site, blogs,
typical `zudo-doc`-scale content sets — hundreds of MDX files with
short bodies) this fits comfortably in default Node + workerd memory.
Very large content sets (tens of thousands of entries, or entries
with multi-megabyte bodies) will push V8 RSS up linearly with snapshot
size; if you are headed in that direction, you should monitor the
snapshot size and plan for the streaming / per-collection sharding
work tracked as future engine roadmap.

To inspect the snapshot footprint of a build, set `ZFB_DEBUG_SNAPSHOT`
to `1` (or `true`):

```sh
ZFB_DEBUG_SNAPSHOT=1 pnpm exec zfb build
```

zfb will print one line to stderr while building:

```
content snapshot: 187 entries / 412 KB
```

`entries` is the total number of content entries across all
collections. `KB` is the byte size of the deterministic JSON
serialization of the snapshot — a useful proxy for the V8 heap cost,
since that is the shape miniflare receives. Any other value (`0`,
unset, `yes`, etc.) leaves the build silent so a stray export does
not change CI output.

## License

MIT — see [LICENSE](./LICENSE).
