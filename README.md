# zudo-front-builder (zfb)

Almost all everyday frontend development in one CLI: create, develop, check, build and preview. Write TSX, MDX, Markdown or HTML pages; compose content APIs, zudo-react interactivity and zudo-wind CSS. The standalone binary and npm distribution provide the same CLI. Full TypeScript checking and Cloudflare tooling still require Node.js.

## Install without Node

No Node.js required. zfb ships as a self-contained binary with its esbuild executable embedded; CSS generation uses the Rust-native zudo-wind engine. For the full install guide including Homebrew, Windows, and which features still need Node as an escape hatch, see the **[Install without Node](https://zfb.takazudomodular.com/docs/getting-started/installation/#standalone-installation)** docs page.

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

For Node.js tools that need CSS directly, use the asynchronous [`compileCss` API](https://zfb.takazudomodular.com/docs/api/compile-css/) from `@takazudo/zfb/css` (Node ≥22). It accepts in-memory configuration and uses the same compiler as `zfb css`.

```sh
# Scaffold a new site
pnpm create zfb@latest my-site

# Or add zfb to an existing project
pnpm add -D @takazudo/zfb
```

`@takazudo/zfb` ships a prebuilt Rust binary per platform via npm optional-deps — no cargo or Rust toolchain required. Because the package declares `engines.node >=22.0.0`, npm-channel users will see a warning if their Node version is older than 22; the binary itself still runs, but the warning is harmless.

> **pnpm 11 note:** pnpm 11's `minimumReleaseAge` may install a previous release of `create-zfb` within ~48h of any new release. If `create-zfb` warns about a stale install at startup, re-run with `pnpm create zfb@latest --config.minimumReleaseAge=0` or `npm create zfb@latest`.

## Why Rust?

Rust makes the framework itself fast, memory-safe, and distributable as a single binary — see the [architecture doc](./docs/src/content/docs/architecture/why-rust.mdx) (or the [docs site](https://zfb.takazudomodular.com/)) for the full rationale.

## What zfb is

zfb is the **engine**: router, renderer, content pipeline, and the small set of build-time primitives (frontmatter extraction, content collections, `paths()`, MDX directive registry, and non-HTML page emission) that something else builds on. Frameworks like Astro and Next.js own the site experience; zfb is the engine a framework (zudo-doc) or an app (CCResDoc) sits on.

zfb is built by one developer for the things that developer is making — zudo-doc documentation sites, CCResDoc, and the sites those serve. It is published because someone who thinks the same way may want it: people who want to drop a content engine into a Tauri-style app the way CCResDoc does, and people who want SSG and SSR as small composable parts rather than a whole framework that owns the site — the same Request/Response application, run three ways.

At the start of the project everything felt big. Next.js is big, and Astro is lighter but still pulls in a large ecosystem of external libraries the author did not need. Today many small features can be built with AI assistance quickly and at high quality, so a library is no longer the default answer for each of them — those pieces are better kept under your own control than under someone else's release schedule. A tiny core CLI matched that goal, so zfb ships as one Rust binary with a small set of primitives rather than a plugin ecosystem. See [Design philosophy](./docs/src/content/docs/architecture/design-philosophy.mdx) for the fuller argument.

| Shape | Drives `fetch` | Example |
| --- | --- | --- |
| Static build | prerendered to HTML at build time | this docs site |
| SSR on Cloudflare Workers | the Worker request handler | `@takazudo/zfb-adapter-cloudflare` |
| Local content server | a host process spawning the `zfb` binary | [CCResDoc](https://github.com/Takazudo/ccresdoc) |

See [`architecture/choosing-zfb.mdx`](./docs/src/content/docs/architecture/choosing-zfb.mdx) for the longer answer on which shape fits.

## What ships, what is experimental, what is planned

**In the current release** (v4.6.0; see the [changelog](https://zfb.takazudomodular.com/docs/changelog/)): the `zfb` commands `new`, `dev`, `check`, `build`, `preview`, `css` and `wind`; the embedded V8 host (deno_core in this release) that loads `zfb.config.ts` and renders pages; TSX, MDX, Markdown and HTML pages; content collections; the islands pipeline with zudo-react, whose shipped bytes stay stable across unrelated edits; zudo-wind utility CSS; `syntect`-backed syntax highlighting; the client router with view transitions; the dev server; and the Cloudflare Workers adapter.

**On `main`, not yet released:** the JS runtime is now the direct V8 host in `crates/zfb-js-host`, which drives the pinned `v8` crate (147.4.0) without deno_core. [#4390](https://github.com/Takazudo/zudo-front-builder/issues/4390) made it the default `direct-v8` Cargo feature, and [#4395](https://github.com/Takazudo/zudo-front-builder/issues/4395) removed the deno_core host and its `embed_v8` feature. There is no runtime switch; `cargo build -p zfb --no-default-features` gives a slim build without V8 (see [BUILDING.md](BUILDING.md#direct-v8-developer-tooling)). The next release is the first to ship it. Other unreleased work is listed in each pending release's notes, not here.

**Known limits of the direct host:** interrupting CPU-bound JavaScript is not supported, and the qualification harness drains a killed child's output only on a best-effort basis ([#4289](https://github.com/Takazudo/zudo-front-builder/issues/4289)). See [`prototypes/direct-v8/WATCHDOG-4289.md`](prototypes/direct-v8/WATCHDOG-4289.md).

**In progress:** the final Deno-free evidence and its native platform rows ([#4302](https://github.com/Takazudo/zudo-front-builder/issues/4302), [#4303](https://github.com/Takazudo/zudo-front-builder/issues/4303)). A label-triggered, non-required CI lane (`direct-v8.yml`) records Linux x64, Linux arm64 and Windows x64 rows; Darwin rows are run on a Mac. Rolldown stays a non-default prototype.

## Docs site

The documentation site lives under [`docs/`](./docs) and is published at <https://zfb.takazudomodular.com/>.

## Limits

`zfb build` reads every configured content collection and builds an in-memory `ContentSnapshot` embedded into the worker bundle that the V8 host loads at build time. For typical project sizes (documentation sites, blogs, hundreds of MDX files) this fits comfortably in default memory. Very large content sets (tens of thousands of entries or entries with multi-megabyte bodies) will push V8 RSS up linearly with snapshot size.

To inspect the snapshot footprint of a build:

```sh
ZFB_DEBUG_SNAPSHOT=1 pnpm exec zfb build
# prints: content snapshot: 187 entries / 412 KB
```

## Contributing

The normal workflow uses reviewed development checkpoints and separately frozen native
qualification SHAs. Slow Mac qualification does not freeze independent development. See the
[development policy](./CLAUDE.md#moving-development-and-frozen-qualification) and
[current queue/evidence](./research/development-queue.md); integration is not release approval.

See [BUILDING.md](./BUILDING.md) for the toolchain, first-build setup, and local development workflow.

This repo uses git worktrees for parallel development. **Pushing from a worktree is forbidden** — child agents commit locally and the manager session merges and pushes from the repo root. See [CLAUDE.md](./CLAUDE.md) for the full worktree-push policy.

For general contribution guidelines (formatting, CI, toolchain requirements), see [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

MIT — see [LICENSE](./LICENSE). Copyright (c) 2026 Takeshi Takatsudo.
