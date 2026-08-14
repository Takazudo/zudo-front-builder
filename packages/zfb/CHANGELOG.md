# Changelog

> **Newer releases:** see https://takazudomodular.com/pj/zudo-front-builder/docs/changelog/ for v0.1.0-next.5 and later. Entries below are historical (kept for npm readers).

## Unreleased

### Behavior changes

**Transcluded files now honour `markdown.gfm`** (#2390):

Files pulled in by `:::include{file="./snippet.md"}` were parsed with **every GFM construct off**, no matter what `markdown.gfm` said — no tables, strikethrough, task lists, footnotes, or autolink literals.

The symptom was that identical markdown rendered differently depending on where it was written: a pipe table in the page became a table, the byte-identical table in an included file stayed literal text. Since 2.5.0 turned `autolinkLiteral` on by default, bare URLs in included files also silently stopped autolinking.

Included files now inherit the project's resolved `markdown.gfm` configuration and render the same as the equivalent content written inline.

**This changes rendered output for existing sites.** If a project transcludes GFM-flavoured content and was (knowingly or not) relying on it staying literal, that content now renders as GFM. Set the relevant `markdown.gfm.*` flags to `false` to keep the old output. A project that never enabled GFM is unaffected.

The same hardcoded construct set was fixed at a second parse site — the body of a directive written **without** blank lines, and the page prose sitting between two such runs (`DirectiveRegistry::reparse_block`). In practice that one is **not** expected to change rendered output: the re-parse is only reached for content the main parse left as a single plain text run, and the main parse now shares the same constructs. It is fixed to keep the two parse sites in lockstep rather than to change behaviour.

The divergence #2390 left open — math constructs staying off at both secondary parse sites — is closed by #2397 below.

**Math in transcluded files now matches the surrounding page** (#2397):

This finishes what #2390 started: content reached through a secondary parse site now renders the same as the equivalent content written inline, on **each** path separately.

zfb parses markdown with a different construct set per path. The HTML serializer keeps math off, so `$$…$$` is literal text; the MDX/JSX path turns it on, because the emitter has dedicated arms for math nodes. Both secondary parse sites — transclusion, and the re-parse of a directive body written without blank lines — hardcoded the HTML set, so they diverged from the top level whenever a page was compiled to JSX.

For MDX/JSX pages that was not a rendering nuance. A single `$$…$$` in an included file leaked LaTeX as bare `{…}` expression containers, esbuild rejected the module, and the bundler's defensive skip degraded the **entire page** to `<pre data-zfb-content-fallback>`. Math in included files is now safe.

**HTML-path output is unchanged.** Math stays off there, exactly as before — turning it on would change rendered output for every existing project and would pull in markdown-rs's single-dollar behaviour, where a literal `$` in prose becomes math. The asymmetry being removed is between transcluded and inline content, not between the two paths.

The directive re-parse site was brought into lockstep the same way. As with #2390's GFM change, it is **not** expected to alter rendered output: that re-parse is only reached for content the main parse left as a single plain text run, and math rich enough to render differently is tokenised by the main parse first.

### Build compatibility

**Explicit workspace-root alias claims** (#1883):

- A nested workspace host may import root-package source through a broad
  TypeScript alias only when the workspace manifest explicitly claims `.`.
- The broad alias stages concrete runtime-imported root-package claims; it
  does not mirror the entire workspace root.
- Parent-escaping relative value imports remain rejected, and the stage-escape
  audit continues to reject genuine live-tree escapes.
- The separately reported private consumer still requires adoption validation
  against the next prerelease; this compatibility boundary does not claim that
  consumer has been run.

### New features

**`VNode`, `VNodeArray`, `VNodeObject` exported from `"@takazudo/zfb"`** (#972):

The structural JSX-node types are now part of the public API:

```ts
import type { VNode, VNodeArray, VNodeObject } from "@takazudo/zfb";
```

`VNode` now includes a bare `object` member (matching Preact's own `ComponentChild` design), making Preact's `ComponentChildren`, `VNode<Props>`, `JSX.Element`, and `JSX.Element[]` all assignable at `Island` input boundaries (`children` and `ssrFallback`) with zero `as unknown as` casts.

**Name-collision caveat for Preact consumers:** if a consumer file already has `import { VNode } from "preact"`, use a qualified import to avoid the clash:

```ts
import type { VNode as ZfbVNode } from "@takazudo/zfb";
```

### Breaking changes (pre-1.0)

**`linkValidation.allowExternal` removed** (#925):

The `allowExternal` config knob has been removed. It was accepted but never did anything — external URL network validation is out of scope. Migration: delete `allowExternal` from your `linkValidation` config; external URLs continue to be silently skipped (unchanged runtime behaviour).

## 0.1.0-next.4

### Bug fixes

**Binary executable bit + launcher EACCES** (#441, #444 §1):

- #441: The bundled `bin/zfb.mjs` launcher was missing its executable bit in the published tarball, causing `zfb: command not found` after `npm install -g`.
- #444 §1: Companion fix ensuring the per-platform native binary receives its executable bit correctly on POSIX systems.

**`--version` stamping** (#445, #444 §2):

- #445: `zfb --version` printed `0.0.0` instead of the actual release version; the binary is now stamped with `ZFB_RELEASE_VERSION` at build time.
- #444 §2: Ensures the version reported by `--version` matches the npm package version for all platforms.

**`paths()` worker / `zfb/content` snapshot flow** (#442):

- #442: Fixed a race in the content-snapshot flow where `paths()` could be invoked before the worker finished writing the snapshot, causing intermittent empty-route tables.

**`@/` tsconfig path-alias regression** (#443):

- #443: The `@/` TypeScript path alias was dropped during the build pipeline refactor in 0.1.0-next.3, breaking imports that relied on the alias in user projects.

**`create-zfb` scaffold dist-tag** (#343):

- #343: `npm create zfb@latest` was resolving the wrong dist-tag on the first install; scaffolded projects now pin to the exact CLI version (`=<ver>` rather than `^<ver>`) to prevent silent downgrade once the stable release lands.

## 0.1.0-next.1

Initial public prerelease on npm.

- Rust-built static-site engine, distributed per-platform via npm optional-deps.
- TypeScript SDK with subpath exports for `runtime`, `content`, `paginate`, `config`, `plugins`, `frontmatter`.
- Bundled `basic-blog` template via `zfb new my-site` / `npm create zfb@latest my-site`.

## Behavior change

**Extra-dirs pass now honors `.gitignore`** (Fix B for #428, closes #433):

- Gitignored top-level directories (e.g. `worktrees/`) are no longer copied into the shadow build tree. Previously the bundler would unconditionally materialise every non-infrastructure top-level directory.
- Global git ignore (`~/.config/git/ignore`) and hidden-directory rules now apply at the top level in addition to `.gitignore`.
- **Negation caveat:** if your `.gitignore` contains a pattern like `!worktrees/keep/` to opt a sub-path back in, the negation is silently ignored by this pass. The extra-dirs walk operates whole-directory-or-nothing at `max_depth=1`; the parent directory is excluded before the negation can apply. Consumers relying on negated sub-path opt-ins in an otherwise-excluded directory will need an alternative arrangement (e.g. move the directory outside the gitignored subtree).
