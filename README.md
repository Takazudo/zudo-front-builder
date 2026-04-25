# zudo-front-builder

`zfb` (zudo-front-builder) is a frontend build orchestrator. The repository is currently in scaffolding phase — the Rust workspace, placeholder bin crate, and basic tooling are being wired up across a series of foundation sub-tasks.

The docs site will be wired up at <https://takazudomodular.com/pj/zudo-front-builder/> once Sub 6/7 land.

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
