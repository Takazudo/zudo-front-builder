# Release-notes draft: Deno exit (epic #4386)

This is a draft for `/l-make-release` to use, covering everything merged after v4.6.0 up to
`360f515` (#4416). It follows the five-lane contract in `CLAUDE.md`. Each lane section below is the
body of `docs/src/content/docs/changelog/<lane>/v<version>.mdx`, minus the frontmatter and the
`Released:` line, which the release procedure adds. The release itself, the version bump and the tag
are Takazudo's.

## Version recommendation: 4.7.0 (minor)

- npm and Homebrew users run the same commands, and the CLI flags, config and output contracts do
  not change. Only the engine inside the binary changes.
- The removed `embed_v8` Cargo feature was a source-build switch, and an opt-in one since #4403. zfb's
  crates are not published to crates.io (no workflow runs `cargo publish`), so no published API is
  removed.
- The noyalib 0.0.58 adoption grows the `zfb-md-wasm` files but keeps their API.

Pick a major only to signal the engine change itself. No published interface requires one.

## Lane: zfb

```md
## Features

- Engine: zfb now runs JavaScript (config evaluation, SSR, plugins) on its own direct V8 host
  (`zfb-js-host`, `v8` 147.4.0) instead of the `deno_core`-based host (9b7fbdad, c0f16f31). npm and
  Homebrew users need to do nothing: commands, config and output are unchanged.
- Engine: the ICU 77 data that V8 needs is vendored and embedded in the binary. No environment
  variable or download is needed at build or run time (b8699c6e).

## Breaking changes (source builds only)

- The `embed_v8` Cargo feature and the `deno_core` host are removed (c0f16f31). The default build
  uses `direct-v8`. `--no-default-features` still builds the slim, V8-free CLI. The V8-off error now
  names `direct-v8 = off` instead of `embed_v8 = off`.
- `deno_core`, `deno_ops`, `deno_error`, `serde_v8` and 17 other crates leave the dependency graph
  (21 records in total). No remaining crate's version changed.

## Bug Fixes

- Islands: chunks that hold a plugin virtual module, such as zudo-doc's shadowed `zdtp-loader`
  stub, keep the same `islands-chunk-[hash]` name across identical rebuilds. The virtual-module file
  is now named after its content (7460a65d, 8d7f581a).
- Windows: `zfb dev` re-renders a page when the target of its `:::include` changes. Verbatim
  (`\\?\C:\…`) dependency paths keep `\` separators, and virtual-module remapping handles verbatim
  imports (a8432f84, 38c251ab).

## Dependencies

- YAML frontmatter: `noyalib` / `noyalib-serde-yaml` 0.0.58 (cf92bfa5). Parsing very large
  frontmatter is about 2.3–2.9x slower than with 0.0.55. Typical pages are unaffected.
- Advisories: RUSTSEC-2025-0141 (`bincode` 1) is now reached only through `syntect`, and
  RUSTSEC-2024-0436 (`paste`) only through `v8`. Both stay recorded as accepted exceptions in
  `deny.toml` (929ed12c).

## Known limitations

- The direct host does not interrupt CPU-bound JavaScript (for example a page stuck in an endless
  loop) and sets no hard heap limit. On shutdown the CLI waits up to 10 seconds for its V8 thread,
  then detaches it.
```

## Lane: zfb-runtime

```md
- No package-specific changes.
```

## Lane: zfb-adapter-cloudflare

```md
- No package-specific changes.
```

## Lane: create-zfb

```md
- No package-specific changes.
```

The generated project is unchanged. Its `zfb` dependency picks up the engine change above.

## Lane: zfb-md-wasm

```md
## Dependencies

- YAML frontmatter parsing moves to `noyalib` 0.0.58 (cf92bfa5). The API is unchanged. The
  artifacts grow: `render` by about 33 KB (`finalWasm` 2,208,228 → 2,241,653 bytes), `parse` by
  about 34 KB and the root entry by about 35 KB. `highlight` stays within a few bytes. The `render`
  size ceiling moves from 1,100,000 to 1,125,000 gzip-9 bytes (20757516).
- All four SHA-256 digests change in this release, as they do in every release: each `.wasm`
  embeds `ZFB_RELEASE_VERSION`, so the digests move even where the compiled code and the byte sizes
  do not. Consumers that pin digests must re-verify the released files.
```

Before release, re-check these figures against `crates/zfb-md-wasm/shipped-sizes.json`: the
release bump re-measures them in CI.

## Merged PRs since v4.6.0

Epic #4386 (Deno exit):

- #4401: vendored ICU, cargo-removal island-size reason, selection-aware qualification rows (S1–S3)
- #4403: direct-v8 becomes the default JS runtime (Gate A, S4)
- #4406: embed_v8-only tests ported to the direct-v8 default (S5/S6)
- #4412: the deno_core host and the embed_v8 rollback removed (Gate B, S7/S8)
- #4414: docs describe the direct V8 host as the only runtime (S10)

Other user-facing changes:

- #4385: noyalib 0.0.58 adopted, render-only ceiling raised (#4268)
- #4382: stable chunk names for chunks holding plugin virtual modules (#4367)
- #4381: Windows path fixes in zfb-build and zfb-content (#4371)

Repo-only (no package-facing effect, so not in the notes): #4345, #4405 and #4411 (the non-default
Rolldown prototype), #4378, #4379, #4380, #4383, #4384, #4402, #4416, and this PR (the draft
itself, plus comment-only wording in the embedded JS shims).
