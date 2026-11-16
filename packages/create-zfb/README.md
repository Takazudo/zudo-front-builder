# create-zfb

Scaffold a new [zfb](https://takazudomodular.com/pj/zudo-front-builder/) static-site project with a single command. When you run `npm create zfb@latest my-site`, npm resolves this package and invokes `zfb new my-site` via the `@takazudo/zfb` CLI — no global install required.

## Usage

```sh
npm create zfb@latest my-site
# or: pnpm create zfb@latest my-site
# or: yarn create zfb@latest my-site
```

This creates a new project directory `my-site/` bootstrapped from the built-in `basic-blog` template. For full documentation, configuration options, and CLI reference, see the [`@takazudo/zfb` docs](https://takazudomodular.com/pj/zudo-front-builder/).

> **pnpm 11 note:** pnpm 11's `minimumReleaseAge` may install a previous release of `create-zfb` within ~48h of any new release. If `create-zfb` warns about a stale install at startup, re-run with `pnpm create zfb@latest --config.minimumReleaseAge=0` or `npm create zfb@latest`.

## Linux arm64 npm distribution: next release

New Linux arm64 npm CLI/CSS distribution is suspended from the next release.
The SDK refuses an older leftover native carrier; reinstalling optional
dependencies cannot fix this. Pure JS helpers and importing the CSS module remain
available; calling the native CSS compiler reports the suspension.

For CLI use, the Linux arm64 standalone installer remains supported. For npm use,
use the complete historical **6.0.1** tuple: `@takazudo/zfb@6.0.1`, its matching
`@takazudo/zfb-linux-arm64-gnu@6.0.1` and `@takazudo/zfb-slugify@6.0.1`;
if used, pin `create-zfb`, `@takazudo/zfb-runtime`, `@takazudo/zfb-adapter-cloudflare`
and `@takazudo/zfb-md-wasm` to `6.0.1` too. Do not mix a newer SDK with an old carrier.
Their registry metadata, tarball availability and SHA-512 integrity were checked
on 2026-11-16. This is availability/integrity evidence, not a Linux arm64 execution
result or a promise of ongoing maintenance or safety.
