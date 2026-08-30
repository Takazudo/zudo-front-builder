# DEPENDENCIES.md — Workspace dependency register

This is the durable register for the Rust and npm dependency audit in #2742. It
records the decisions made in the merged audit work, so a future audit can start
from evidence instead of repeating the same probes. The temporary baseline and
decision artifacts used during the audit were removed by #2754 after the final
comparisons; this document is their durable repository-level synthesis.

The published npm packages' runtime supply-chain surface is intentionally kept
separate in [SECURITY-DEPS.md](./SECURITY-DEPS.md).

## Removed in this epic

### Confirmed-dead Rust declarations (#2744)

The nine declarations below had no call sites after a crate-wide search of
`src/`, `tests/`, and `benches/`. The `zfb` manifest was intentionally not part
of this deletion: its `flate2`, `tar`, `hex`, and `zfb-binfetch` declarations
are used by `crates/zfb/build.rs`.

| Crate | Removed declaration | Evidence | Actual lock-file result |
| --- | --- | --- | --- |
| `zfb-build` | `fs2` normal dependency | No `fs2::` or `FileExt` use; the test lock is hand-rolled in `zfb-test-utils`. | `fs2 0.4.3` left `Cargo.lock`. |
| `zfb-css` | `walkdir` normal dependency | No `walkdir` or `WalkDir` use in the crate. | The `zfb-css` edge disappeared; the package remains through other users. |
| `zfb-css` | `serde` normal dependency | No `serde::` use or `Serialize`/`Deserialize` derive; `serde_json` is a separate live dependency. | The `zfb-css` edge disappeared; `serde` remains widely transitive. |
| `zfb-render` | `anyhow` normal dependency | No use in the crate. | The `zfb-render` edge disappeared; the package remains through other users. |
| `zfb-md-extras` | `anyhow` normal dependency | No use in the crate. | The normal edge disappeared; the package remains. |
| `zfb-md-extras` | `anyhow` dev-dependency | No use in the crate's tests or gated test targets. | The dev edge disappeared; the package remains. |
| `zfb-content` | `anyhow` normal dependency | No use in the crate. | The `zfb-content` edge disappeared; the package remains. |
| `zfb-router` | `anyhow` normal dependency | No use in the crate. | The `zfb-router` edge disappeared; the package remains. |
| `zfb-router` | `tracing-subscriber` dev-dependency | No `tracing_subscriber` use; `tracing-test` is used and was retained. | The `zfb-router` edge disappeared; the package remains. |

The only package record removed by #2744 was `fs2 0.4.3`; the baseline count
was 603 and this branch's corresponding graph was 602. The lockfile also lost
the direct package edges listed above, but no `anyhow`, `serde`, `walkdir`, or
`tracing-subscriber` package could be claimed as removed when another path still
used it. This distinction is the audit rule: an unused direct declaration can be
removed even when the package remains in the lockfile.

### Dead `BuildProgress` and `indicatif` (#2745)

`crates/zfb/src/output.rs` contained an explicitly unwired `BuildProgress`
wrapper, its private `fmt_summary` helper, and only the helper's own tests. The
source comment said it was intended for build progress but was not connected to
the build command. The deletion removed that code, its `indicatif` import and
declaration, and the tests for deleted code. `owo-colors` was not removed: the
remaining output helpers still use it.

The actual lock delta was five package records:

- `indicatif 0.18.6`;
- `console 0.16.4`;
- `unit-prefix 0.5.2`;
- `portable-atomic 1.13.1`; and
- `encode_unicode 1.0.0`, which was left unused when `console` disappeared.

The package count therefore moved from 602 to 597 for this topic's before/after
graph. `unicode-width`, `web-time`, and other shared packages were not claimed
as removals because the lock delta did not remove them.

### Narrow graph cleanups (#2747 and #2748)

* **`futures` umbrella (#2747).** `zfb-server` now has `futures-util` once in
  normal dependencies, and the two imports use `futures_util` directly. The
  lockfile changed only by removing the `"futures"` edge from the `zfb-server`
  package stanza. The `futures` package itself remains because `deno_core`
  pulls it in under the default `embed_v8` graph, so the package count is
  unchanged. In the `zfb --no-default-features` compile graph, both `futures`
  and `futures-executor` disappear; this is a build-no-V8 compile-graph win,
  not a workspace package-count reduction.
* **`reqwest`'s `json` feature in `zfb-build` (#2748).** There are no
  `Response::json()` calls in `zfb-build`, so its speculative feature was
  removed while `blocking` and `rustls-tls` remained. `Cargo.lock` did not
  change, and `serde_urlencoded` did not leave: it is still reached through
  `reqwest` and the workspace's `axum` path.
* **Actual feature ownership correction (#2756/#2757).** The live
  `Response::json()` call is in `crates/zfb/src/render_pipeline.rs`, where it
  parses `/__paths__` responses. The production `zfb` `reqwest` declaration
  therefore owns `json`; #2756 added it there and left it absent from
  `zfb-build`. That correction also changed no lock package. It prevents
  feature unification from hiding an undeclared feature requirement.

Across the merged audit, the baseline's 603 Cargo.lock packages became 597:
the six-package net reduction is exactly `fs2` plus the five #2745 packages.
The #2749 Unicode migration is a one-for-one leaf replacement and does not
change that count.

## Considered and deliberately kept

The governing framing is **used versus unused**, not “free versus costly”. A
crate directly imported by first-party code stays a direct dependency even when
another path also pulls it transitively. Conversely, a genuinely unused direct
declaration is removable even if its package remains in `Cargo.lock`.

### Rust dependencies

* **`hex` — terminal KEEP; do not revisit.** A production-only search finds 21
  `hex::` call sites across `zfb`, `zfb-build`, `zfb-content`, `zfb-css`,
  `zfb-islands`, `zfb-md-ast`, and `zfb-types` (tests add separate coverage).
  It is also required by `crates/zfb/build.rs`: `sha256_hex` calls
  `hex::encode` at line 169, `sha256_hex_file` calls it at line 187, and the
  helpers verify the downloaded esbuild binary. Removing it would require
  hand-rolled encoding while buying no package reduction; keep the maintained
  crate.
* **`reqwest` — KEEP.** The five principal non-optional consumers in the audit
  are `zfb`, `zfb-build`, `zfb-binfetch`, `zfb-server`, and `zfb-test-utils`;
  `zfb-render` also has a required optional edge for its embedded-V8 fetch
  transport. The current lock resolves `reqwest 0.12.28`. It shares `hyper`
  with `axum`, `zfb-binfetch` needs real HTTPS to download from GitHub, and the
  call sites span build, server, test, binary-fetch, and request-time paths, so
  replacing it would be a large refactor with little payoff. This register does
  not assign an undefined “largest contributor” ranking to its graph weight.
  The actionable sliver was the unused `zfb-build` `json` feature, handled in
  #2748; the real production `.json()` consumer is owned by `zfb` after #2756.
* **`walkdir` — KEEP.** It is directly used; zero package-count payoff from
  replacement. It drives route, source, graph, and asset walks across the
  workspace, including symlink and depth behavior that callers test explicitly.
* **`owo-colors` — KEEP.** It is directly used; zero package-count payoff from
  replacement. The CLI output and diagnostics modules still use its stream-aware
  color methods and process-wide test override after `BuildProgress` was deleted.
* **`sourcemap` — KEEP.** It is directly used; zero package-count payoff from
  replacement. `zfb-css` parses inline Source Map v3 data to attribute package
  URLs, and the build path uses the same crate for source-map handling; replacing
  it would discard the maintained parser rather than remove an unused edge.
* **`serde_path_to_error` — KEEP.** It is directly used; zero package-count
  payoff from replacement. `zfb` wraps config deserialization with it so errors
  identify fields such as `framework` and indexed collection paths.
* **`html5ever` + `markup5ever_rcdom` — KEEP as a pair in `zfb-test-utils`.**
  `html5ever` 0.27 parses HTML fragments and serializes them; `RcDom` supplies
  the mutable tree used to sort attributes, normalize nodes, and produce
  canonical test output. The versions must stay compatible with the
  `markup5ever` version they share.
* **`lol_html` — KEEP in its four current crates.** `zfb`, `zfb-build`,
  `zfb-islands`, and `zfb-server` use its selector/token rewriting for live
  HTML injection, island-marker handling, and link/base rewriting. This is not
  the same problem as the `html5ever`/`RcDom` tree normalizer: `lol_html` is a
  streaming rewriter and does not replace a mutable HTML5 tree for test
  canonicalization. These dependencies do not genuinely consolidate; leave
  both roles explicit.
* **`bincode` 2 — KEEP in `zfb-graph`.** This first-party-only direct edge is
  removable in principle, but it serializes the on-disk dependency graph cache
  through the `serde` integration. The wire-format version was bumped during
  the bincode 1→2 migration because the encoding differs, and the existing
  cache invalidation behavior depends on that explicit version.
* **`imagesize` — KEEP in `zfb-md-extras`.** This exclusively ours direct edge
  is removable in principle, but the image-dimensions feature probes raster
  headers without decoding complete image payloads, preserving the warning and
  cache behavior of the plugin.
* **`roxmltree` — KEEP in `zfb-md-extras`.** This exclusively ours direct edge
  is removable in principle, but `imagesize` cannot read SVGs, so the plugin
  uses `roxmltree` to parse SVG `width`/`height` and `viewBox` dimensions while
  skipping files with no determinable intrinsic size.
* **`html-escape` — KEEP in `zfb-content`.** This exclusively ours direct edge
  is removable in principle, but the directive parser decodes HTML character
  references in attribute values before constructing its typed nodes; it is a
  live parser operation, not an unused declaration.

### Rust decisions recorded by the audit

* **Unicode categories (#2749) — MIGRATE.** `unicode_categories 0.1.1` was
  replaced by maintained `unicode-general-category 1.1.0`. The parser needs the
  Unicode general-category `P | S` predicate for directive-name boundaries;
  preserving it with an in-house table would hit the abandon rule. The full
  scalar differential covered 1,112,064 values and found 1,855 differences
  (1,854 new punctuation/symbol boundaries and U+111C9 changing from
  punctuation to Unicode-16 `Mn`). The lock change is one leaf out and one leaf
  in, with no package-count reduction. The package-facing behavior is recorded
  in the `zfb` and `zfb-md-wasm` v2.14.0 lanes; the other three lanes say no
  package-specific changes.
* **`serde_yaml 0.9.34+deprecated` (#2750) — STAY.** It remains directly used
  for frontmatter-to-`serde_json::Value` conversion, `zfb`'s 1-based diagnostic
  locations, and the md-wasm path. `serde_yaml_ng` matched the tested API and
  behavior but its latest release was 2024-05-26 and its last repository commit
  was 2025-09-14 as of this audit; it does not resolve the maintenance concern.
  `serde_yml` is deprecated/archived, changes error locations and merge-key
  behavior, and is rejected by the repository's advisory policy. `saphyr` is
  active but is not a Serde deserializer, so it cannot preserve the contract
  without a new adapter. #2755 is the standing follow-up: re-open when a
  maintained compatible release, fork, or Serde wrapper can preserve the
  existing JSON and diagnostic behavior, and include `serde_norway`, `noyalib`,
  and `serde-saphyr` in that future scan if they remain current.
* **Platform/process utilities (#2751) — KEEP.** `local-ip-address` enumerates
  every non-loopback IPv4 interface for bind-all ready URLs; a UDP-connect
  shortcut would select only one egress address, and the repository has no
  Windows Rust test lane to prove equivalent interface enumeration. `wait-timeout`
  provides the cross-platform `ChildExt::wait_timeout` path that kills and
  reaps a timed-out child; replacing it with polling is both a semantic change
  and an abandon-rule trigger. `npm-run-all2` supervises the two long-running
  docs processes in `dev` and `dev:network`; a replacement must forward signals,
  propagate exit status, and clean up both children. No such replacement met
  the required contrary gate.
* **JavaScript checker (#2752) — recommendation, not a new dependency.** Do
  not make a JS checker required in this audit. If revisited, pilot a pinned
  **Knip** release in report-only mode rather than depcheck: it better models
  pnpm projects, entry points, exports, and dependency categories. Reconcile
  every report against the generated docs bundle, the optional/undeclared peer
  keep-lists below, and publishable package contracts; promote only after two
  clean pilots and reviewed configuration. The existing cargo-machete guard is
  the lower-noise Rust check.

### npm keep-lists from `docs/CLAUDE.md`

The docs site deliberately installs these **optional peer feature packages** of
`@takazudo/zudo-doc`: `diff`, `katex`, `preact`, `zod`, `@takazudo/zdtp`, and
`@takazudo/zudo-doc-history-server`. They satisfy opt-in package features and
the history server's docs development process; they are not dead because their
imports are not all in `docs/src`.

It also declares these **runtime imports not declared by zudo-doc itself**:
`mermaid`, `minisearch`, and `remark-cjk-friendly`. They appear in the generated
`docs/.zfb-build/bundle.mjs`, so the docs-level declarations supply module
resolution and feature behavior. `docs/CLAUDE.md` is the keep-list's source of
truth; it is not a request to add dependencies.

## walkdir consolidation is an explicit non-goal

There are approximately 75 `walkdir` call sites and no package-count payoff
from replacing them with `ignore`: `walkdir` is already directly used in the
route, graph, build, CSS, and island paths. `ignore`'s gitignore awareness is a
semantic change to directory walks, not a drop-in implementation detail; it
would change which files are visited in paths that intentionally preserve
different symlink, depth, and generated-directory rules. Do not start a future
audit by treating this as an open consolidation task.

## Duplicate-version note

The duplicate-version capture taken at the audit's parent commit recorded these
version families:

| Family | Baseline ownership/control analysis |
| --- | --- |
| `ahash` 0.7 / 0.8 | 0.7 is under `lightningcss`'s `parcel_sourcemap`/`rkyv` branch. 0.8 is shared by the `lightningcss`/`minify-html` side and the SWC `swc_atoms` side. There is no first-party `ahash` declaration to collapse; upstream graph ownership controls these versions. |
| `bincode` 1.3 / 2.0 | 1.3 is pulled by `deno_core` and `syntect`; 2.0 is the deliberate first-party `zfb-graph` cache codec. The owned 2.0 edge cannot be replaced with the old external codec without undoing the maintained-format migration. |
| `compact_str` 0.7 / 0.8 / 0.9 | 0.7 comes from SWC `swc_ecma_codegen`, 0.8 from `garde`, and 0.9 from Oxc (`oxc_resolver`/`oxc_span`). None is a first-party direct declaration; aligning them requires upstream upgrades, not a safe manifest edit here. |
| `cssparser` 0.33 / 0.36 | 0.33 is dictated by `lightningcss` and its selector/sourcemap stack; 0.36 is used by `lol_html`. Both are external parser owners with live APIs. |
| `getrandom` 0.2 / 0.3 / 0.4 | 0.2 comes through `ahash`/`ring`/Rustls, 0.3 is the optional first-party `zfb-render` Web Crypto edge (and also appears under `ahash` 0.8), and 0.4 comes from `tempfile`. The first-party 0.3 edge is required for OS CSPRNG semantics; the other versions are transitive. |
| `hashbrown` 0.12 / 0.14 / 0.16 / 0.17 | 0.12 is in the `lightningcss`/`rkyv` branch; 0.14 is under `ahash`/SWC and `lightningcss`; 0.16 is used by Oxc's allocator/data structures; 0.17 is reached through `indexmap` across `deno_core`, `lightningcss`, Oxc, and other upstreams. No first-party `hashbrown` declaration owns a version. |
| `base64-simd` 0.7 / 0.8 | 0.7 comes from `lightningcss`'s `parcel_sourcemap`; 0.8 comes from the first-party `sourcemap` path as well as Oxc/SWC sourcemap consumers. The direct `sourcemap` use is live, and its transitive version choice is not ours to pin independently. |

The only first-party ownership choices in this list are the deliberate
`bincode 2` cache codec and `getrandom 0.3` Web Crypto edge, both of which are
live and documented above. `swc_core`, `deno_core`, `lightningcss`, Oxc,
`garde`, `lol_html`, Rustls, and `tempfile` dictate the remaining versions.
There is no currently actionable duplicate for us to fix and no safe duplicate
collapse to perform in this register.

## Deliberate duplication

* **Wrangler.** The root `package.json` and `docs/package.json` each pin
  `wrangler` to exactly `4.85.0`. This is intentional: root scripts and
  showcase/deploy workflows use the root tool, while docs commands use the docs
  importer. `docs/scripts/check-wrangler-pin.mjs` checks both manifests and
  both lockfile importers, so the two pins cannot drift silently.
* **`html-validate`.** The root pins `10.17.0` exactly for root/showcase HTML
  validation, while docs declares `^10.0.0` for its `check:html` script. The
  current lock resolves the docs declaration to `10.17.0`, but the ranges
  express different ownership boundaries. Recommendation: yes, add an
  equivalent pin-guard in a follow-up issue for reproducible root/docs
  validation; do not implement that guard in this documentation issue.

## Abandon rule

This is the standing policy for any future dependency replacement. The source
lesson is
`.claude/skills/l-lessons-client-bundling/SKILL.md`.

Abandon the replacement immediately when any one of these triggers fires:

1. The replacement exceeds **40 lines of rustfmt- or prettier-formatted,
   non-test production code**, counted across every touched file. Compressed
   one-liners do not evade the limit.
2. A **second corrective round** is needed on the same dependency.
3. The replacement requires **`unsafe`, FFI, OS-specific branches, polling
   loops, signal supervision, or Unicode tables**. This is an immediate keep,
   regardless of line count.

On a trigger, restore the dependency, delete the replacement, and record a
terminal **`KEEP`** verdict. A terminal KEEP is a completed sub-task, not a
failure; downstream auto-review must not reopen removal of that dependency.

## npm-workspace inventory

The #2746 inventory covered 14 package manifests: the workspace root,
`docs/`, every `packages/*` manifest, every current `crates/*/npm` manifest, and
`tests/md-wasm-browser-smoke/package.json` (the root plus 13 pnpm projects).
The reserved `examples/*` workspace glob currently has no package manifest.
Test-fixture `package.json` files under crate fixture trees are fixtures, not
workspace package importers.

| Manifest | Package and declarations reviewed | Result |
| --- | --- | --- |
| `package.json` | Private root; six dev tools: `@playwright/test`, `html-validate`, `lefthook`, `prettier`, `vitest`, `wrangler`. | Clean: Playwright, HTML validation, hooks, formatting, tests, and Wrangler workflows/scripts each consume the declared tool. |
| `docs/package.json` | Private docs site; zudo-doc stack, the two intentional keep-lists, TypeScript/types, `html-validate`, `npm-run-all2`, `vitest`, and Wrangler. | Clean after #2746 removed only `pagefind`, `remark-directive`, and the redundant `gray-matter`; `js-yaml`'s override remains because `gray-matter` is still transitive through zudo-doc. |
| `packages/create-zfb/package.json` | Publishable scaffold with `@takazudo/zfb` dependency and Vitest dev dependency. | Clean: the CLI resolves and spawns the zfb package; tests consume Vitest. |
| `packages/zfb/package.json` | Publishable SDK with five optional platform packages, React peer, and build/test type tooling. | Clean: optional carriers and peer/dev fixtures are part of the package contract. |
| `packages/zfb-runtime/package.json` | Publishable runtime with `hono` dependency, zfb/React peers, and dev fixtures. | Clean: Hono is the runtime router; peer and dev declarations support the published API/tests. |
| `packages/zfb-adapter-cloudflare/package.json` | Publishable adapter with no runtime `dependencies`; Node types, TypeScript, and Vitest are dev-only. | Clean: shipped CLI uses Node built-ins and the project-local worker wrapper. |
| `packages/zfb-darwin-arm64/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-darwin-x64/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-linux-arm64-gnu/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-linux-x64-gnu/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `packages/zfb-win32-x64-msvc/package.json` | Publishable native carrier; no dependencies. | Clean: package ships the platform binary and metadata only. |
| `crates/zfb-islands/npm/package.json` | Private islands runtime package with zfb dependency and test/type tooling. | Clean: the runtime package and its Vitest fixture use the declarations. |
| `crates/zfb-md-wasm/npm/package.json` | Publishable md-wasm package; `@types/mdast`, `mdast-util-directive`, and `mdast-util-mdx` are dependencies, with parser/build tools dev-only. | Clean: published declarations re-export mdast/directive/MDX types and registries, so consumers need these dependencies. |
| `tests/md-wasm-browser-smoke/package.json` | Private browser smoke package with the local packed md-wasm artifact. | Clean: the browser smoke imports the packaged artifact. |

No declaration with no reference was found in the retained inventory. The three
dead docs declarations were removed and documented above. **No publishable
package's `dependencies` field was modified by #2746 or this register**; those
runtime fields remain the separate supply-chain surface audited in
[SECURITY-DEPS.md](./SECURITY-DEPS.md).
