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
