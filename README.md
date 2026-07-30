# zudo-front-builder (zfb)

`zfb` is a Rust-built static-site engine for TypeScript/JSX projects — millisecond rebuilds, single binary, no cargo needed by end users.

## Install without Node

No Node.js required. zfb ships as a self-contained binary — esbuild and Tailwind are included. For the full install guide including Homebrew, Windows, and which features still need Node as an escape hatch, see the **[Install without Node](https://takazudomodular.com/pj/zudo-front-builder/docs/install/node-free/)** docs page.

**Linux / macOS (curl):**

```sh
curl -fsSL https://raw.githubusercontent.com/Takazudo/zudo-front-builder/main/install.sh | sh
```

Installs `zfb` to `$HOME/.local/bin/zfb`. Set `ZFB_INSTALL=/your/path` to change the prefix. Set `ZFB_VERSION=v1.0.0` to pin a specific release, or `ZFB_VERSION=latest-prerelease` to opt into prereleases.

**macOS (Homebrew):**

```sh
brew install Takazudo/tap/zfb
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/Takazudo/zudo-front-builder/main/install.ps1 | iex
```

This downloads the `x86_64-pc-windows-msvc` binary, verifies its SHA-256 checksum, and installs `zfb.exe` to `%LOCALAPPDATA%\zfb\bin\zfb.exe`. Set `$env:ZFB_INSTALL` to change the install root. Set `$env:ZFB_VERSION` to pin a specific release tag (e.g. `v0.2.0`) or use `latest-prerelease` to opt into pre-releases.

## Install via npm (Node required)

```sh
# Scaffold a new site
pnpm create zfb@latest my-site

# Or add zfb to an existing project
pnpm add -D @takazudo/zfb
```

`@takazudo/zfb` ships a prebuilt Rust binary per platform via npm optional-deps — no cargo or Rust toolchain required. Because the package declares `engines.node >=22.0.0`, npm-channel users will see a warning if their Node version is older than 22; the binary itself still runs, but the warning is harmless.

> **pnpm 11 note:** pnpm 11's `minimumReleaseAge` may install a previous release of `create-zfb` within ~48h of any new release. If `create-zfb` warns about a stale install at startup, re-run with `pnpm create zfb@latest --config.minimumReleaseAge=0` or `npm create zfb@latest`.

## Why Rust?

Rust makes the framework itself fast, memory-safe, and distributable as a single binary — see the [architecture doc](./docs/src/content/docs/architecture/why-rust.mdx) (or the [docs site](https://takazudomodular.com/pj/zudo-front-builder/)) for the full rationale.

## What zfb is

zfb is the **engine**: router, renderer, content pipeline, and the small set of build-time primitives (frontmatter extraction, content collections, `paths()`, MDX directive registry, and non-HTML page emission) that a framework can build on. Frameworks like a future `zudo-doc-v2` sit on top of these primitives and own the opinionated layer — sidebar generation, search, theming, blog conventions, i18n routing, versioning UI, and so on.

zfb is the engine for content sites whose hard parts live outside page rendering — build-time data pipelines, custom content collections, project-specific glue. For sites whose hard parts are page rendering itself (multi-framework, ISR, RSC, server actions), use Astro or Next. See [`concepts/choosing-zfb.mdx`](./docs/src/content/docs/concepts/choosing-zfb.mdx) for the longer answer.

The full pipeline ships today: embedded V8 host, `zfb.config.ts` config loader, `syntect`-backed syntax highlighting, islands pipeline, client router with view transitions, content-collection bridge, and dev-server are all wired.

## Docs site

The documentation site lives under [`docs/`](./docs) and is published at <https://takazudomodular.com/pj/zudo-front-builder/>.

## Limits

`zfb build` reads every configured content collection and builds an in-memory `ContentSnapshot` embedded into the worker bundle that the V8 host loads at build time. For typical project sizes (documentation sites, blogs, hundreds of MDX files) this fits comfortably in default memory. Very large content sets (tens of thousands of entries or entries with multi-megabyte bodies) will push V8 RSS up linearly with snapshot size.

To inspect the snapshot footprint of a build:

```sh
ZFB_DEBUG_SNAPSHOT=1 pnpm exec zfb build
# prints: content snapshot: 187 entries / 412 KB
```

## Contributing

See [BUILDING.md](./BUILDING.md) for the toolchain, first-build setup, and local development workflow.

This repo uses git worktrees for parallel development. **Pushing from a worktree is forbidden** — child agents commit locally and the manager session merges and pushes from the repo root. See [CLAUDE.md](./CLAUDE.md) for the full worktree-push policy.

For general contribution guidelines (formatting, CI, toolchain requirements), see [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

MIT — see [LICENSE](./LICENSE). Copyright (c) 2026 Takeshi Takatsudo.
