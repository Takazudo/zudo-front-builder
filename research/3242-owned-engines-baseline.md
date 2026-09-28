# Owned Engines pre-change baseline

This note records release build and visual measurements before the Owned Engines changes.
It sets no target and makes no performance claim.

## Host and revision

Revision: `704f42c87703e4f62eb4eeaf1758a0af557520f3` (`base/owned-engines`). The host was
macOS 26.6.1 (Darwin 25.6.0), arm64, Apple A18 Pro, 6 logical CPU cores, and 8,589,934,592
bytes of memory. The pre-change check printed no diff stat:

```sh
git diff --stat $(git merge-base HEAD origin/main) HEAD -- crates packages tests
```

The merge base was `16bd41a88768761a428a326c8ba4c1e0ca18c987`; `HEAD` was the revision above.
The metadata and versions below were collected by the measurement helper invocation listed
in the measurement section.

| Tool | Version | Command |
| --- | --- | --- |
| Rust | `rustc 1.94.0 (4a4ef493e 2026-03-02) (Homebrew)` | `rustc --version` |
| Node.js | `v24.14.0` | `node --version` |
| pnpm | `11.3.0` | `pnpm --version` |
| zfb | `zfb 0.0.0`, embedded Tailwind CSS `4.2.0`, embedded esbuild `0.25.12` | `/Users/takazudo/repos/myoss/zfb/target/release/zfb --version` |

`ZFB_EMBEDDED_NODE_MODULES_CACHE` was unset (its default), and no other inherited `ZFB_`
variables were set. The timing samples set `ZFB_BUILD_TIMING=1` in each build command.

## Build setup and commands

The first full release build completed before the frozen install step because the checkout
already had `node_modules`. The measurement helper and browser capture then ran from that
release binary. The required frozen install was subsequently run and reported “Already up to
date”; a guarded incremental release build passed afterward. The release binary size and
version remained the values recorded below, and no source or dependency state changed. The
post-install recheck command was `stat -f '%N %z' target/release/zfb && target/release/zfb --version`.
Commands and results:

```sh
cd /Users/takazudo/repos/myoss/zfb
pnpm install --frozen-lockfile
```

```sh
bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- cargo build --release -p zfb
```

The full release build passed in 498 seconds. The incremental build after the frozen install
also passed. Measurement and browser commands were run through the heavy guard; browser
capture additionally used the Playwright guard.

## Release artifact sizes

The release binary and embedded vendor measurements came from the five-sample helper command
in the build measurement section. The one matching vendor directory was
`target/release/build/zfb-b32f38fe0dde90cf/out/vendor`.

| Artifact | Bytes | Exact command |
| --- | ---: | --- |
| `target/release/zfb` | 175,768,640 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |
| Vendor `@takazudo/` | 367,933 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |
| Vendor `bin/` | 86,490,962 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |
| Vendor `hono/` | 1,416,507 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |
| Vendor `preact/` | 644,857 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |
| Vendor `preact-render-to-string/` | 305,769 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |
| Vendor `bin/esbuild` | 9,934,834 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |
| Vendor `bin/tailwindcss-v4` | 76,556,128 | `bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5` |

The corresponding staged slots under `crates/zfb/binaries/` were measured separately:

| Staged slot | Bytes | Exact command, from the repository root |
| --- | ---: | --- |
| `crates/zfb/binaries/esbuild/esbuild` | 9,934,834 | `find crates/zfb/binaries -type f -not -name '*.md' -not -name '.gitkeep' -exec stat -f '%N %z' {} \;` |
| `crates/zfb/binaries/tailwindcss-v4` | 76,556,128 | `find crates/zfb/binaries -type f -not -name '*.md' -not -name '.gitkeep' -exec stat -f '%N %z' {} \;` |

The version output was:

```text
$ /Users/takazudo/repos/myoss/zfb/target/release/zfb --version
zfb 0.0.0
embedded Tailwind CSS: 4.2.0
embedded esbuild: 0.25.12
```

## Basic-blog build timings

The sampling helper copied `crates/zfb/templates/basic-blog/` to five fresh directories under
`/tmp/zfb-3245-measurements/`. Each cold build used a fresh copy; its warm build reran in that
same directory. Copies stayed outside the repository and no project install was run in them.
Two `zfb-tailwind-oxide-warmup-*.done` markers existed in the system temporary directory
before the first cold sample: `zfb-tailwind-oxide-warmup-d9e759fd6612dd44.done` and
`zfb-tailwind-oxide-warmup-de3a07a728a1a04b.done`.

The guarded measurement command was:

```sh
bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5
```

Wall times and exact commands:

| Sample | Wall time (ms) | Exact command |
| --- | ---: | --- |
| Cold 1 | 13,504.591 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Cold 2 | 6,876.570 | `cd '/tmp/zfb-3245-measurements/basic-blog-02' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Cold 3 | 5,761.533 | `cd '/tmp/zfb-3245-measurements/basic-blog-03' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Cold 4 | 6,992.691 | `cd '/tmp/zfb-3245-measurements/basic-blog-04' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Cold 5 | 7,048.560 | `cd '/tmp/zfb-3245-measurements/basic-blog-05' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Warm 1 | 6,639.985 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Warm 2 | 5,841.855 | `cd '/tmp/zfb-3245-measurements/basic-blog-02' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Warm 3 | 10,464.120 | `cd '/tmp/zfb-3245-measurements/basic-blog-03' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Warm 4 | 6,638.351 | `cd '/tmp/zfb-3245-measurements/basic-blog-04' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Warm 5 | 6,437.692 | `cd '/tmp/zfb-3245-measurements/basic-blog-05' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |

Cold wall time min/median/max: 5,761.533 / 6,992.691 / 13,504.591 ms. Warm wall time
min/median/max: 5,841.855 / 6,638.351 / 10,464.120 ms. These summaries use this exact
command:

`bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/baseline/measure.mjs /Users/takazudo/repos/myoss/zfb /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-measurements 5`

Raw timing lines from each sample follow. The `Command` line identifies the exact command
that produced the timing lines in that sample.

### Cold samples

```text
Sample: basic-blog-01-cold
Wall time (ms): 13504.591
Command: cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=525
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=1791
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=726
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=32
[zfb-build-timing] phase=production-assets elapsed_ms=4361
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=203

Sample: basic-blog-02-cold
Wall time (ms): 6876.570
Command: cd '/tmp/zfb-3245-measurements/basic-blog-02' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=527
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=695
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=554
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=55
[zfb-build-timing] phase=production-assets elapsed_ms=4046
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=157

Sample: basic-blog-03-cold
Wall time (ms): 5761.533
Command: cd '/tmp/zfb-3245-measurements/basic-blog-03' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=311
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=718
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=519
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=26
[zfb-build-timing] phase=production-assets elapsed_ms=3127
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=92

Sample: basic-blog-04-cold
Wall time (ms): 6992.691
Command: cd '/tmp/zfb-3245-measurements/basic-blog-04' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=357
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=855
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=577
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=28
[zfb-build-timing] phase=production-assets elapsed_ms=3888
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=116

Sample: basic-blog-05-cold
Wall time (ms): 7048.560
Command: cd '/tmp/zfb-3245-measurements/basic-blog-05' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=413
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=600
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=681
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=30
[zfb-build-timing] phase=production-assets elapsed_ms=3707
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=179
```

### Warm samples

```text
Sample: basic-blog-01-warm
Wall time (ms): 6639.985
Command: cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=344
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=515
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=504
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=58
[zfb-build-timing] phase=production-assets elapsed_ms=3925
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=122

Sample: basic-blog-02-warm
Wall time (ms): 5841.855
Command: cd '/tmp/zfb-3245-measurements/basic-blog-02' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=399
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=601
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=532
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=38
[zfb-build-timing] phase=production-assets elapsed_ms=3424
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=91

Sample: basic-blog-03-warm
Wall time (ms): 10464.120
Command: cd '/tmp/zfb-3245-measurements/basic-blog-03' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=325
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=768
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=596
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=45
[zfb-build-timing] phase=production-assets elapsed_ms=7460
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=188

Sample: basic-blog-04-warm
Wall time (ms): 6638.351
Command: cd '/tmp/zfb-3245-measurements/basic-blog-04' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=326
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=660
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=508
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=30
[zfb-build-timing] phase=production-assets elapsed_ms=3954
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=192

Sample: basic-blog-05-warm
Wall time (ms): 6437.692
Command: cd '/tmp/zfb-3245-measurements/basic-blog-05' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build
[zfb-build-timing] phase=config-load elapsed_ms=341
[zfb-build-timing] phase=plugin-host-spawn elapsed_ms=0
[zfb-build-timing] phase=plugin-setup elapsed_ms=0
[zfb-build-timing] phase=vendor-extraction-and-bundler-input elapsed_ms=545
[zfb-build-timing] phase=main-esbuild-bundle elapsed_ms=492
[zfb-build-timing] phase=v8-paths-eval elapsed_ms=43
[zfb-build-timing] phase=production-assets elapsed_ms=3527
[zfb-build-timing] phase=v8-boot-and-render elapsed_ms=167
```

## Emitted output sizes

The output summary used the same guarded measurement helper command shown above. Basic-blog
figures are from `dist/` after the warm build in sample 1:

| Basic-blog `dist/` file | Bytes | Exact build command |
| --- | ---: | --- |
| File count | 9 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Total | 96,016 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `404.html` | 3,430 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `about/index.html` | 6,368 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `blog/hello-zfb/index.html` | 7,499 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `blog/markdown-showcase/index.html` | 16,516 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `blog/styling-with-tailwind/index.html` | 8,871 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `index.html` | 5,119 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `assets/styles-98f01a7e.css` | 28,090 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `assets/islands-a862eb2b.js` | 18,880 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `__zfb/routes.json` | 1,243 | `cd '/tmp/zfb-3245-measurements/basic-blog-01' && ZFB_BUILD_TIMING=1 '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |

The built-site-smoke fixture was copied outside the repository, built with the release binary,
and its `dist/` measured:

| Built-site-smoke `dist/` file | Bytes | Exact build command |
| --- | ---: | --- |
| File count | 3 | `cd '/tmp/zfb-3245-measurements/built-site-smoke' && '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| Total | 18,833 | `cd '/tmp/zfb-3245-measurements/built-site-smoke' && '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `index.html` | 557 | `cd '/tmp/zfb-3245-measurements/built-site-smoke' && '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `assets/islands-15880d0a.js` | 18,108 | `cd '/tmp/zfb-3245-measurements/built-site-smoke' && '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |
| `__zfb/routes.json` | 168 | `cd '/tmp/zfb-3245-measurements/built-site-smoke' && '/Users/takazudo/repos/myoss/zfb/target/release/zfb' build` |

## Visual baseline

Chromium was installed with `pnpm exec playwright install chromium`. The capture used
Playwright 1.61.0 and Chromium 149.0.7827.55. It recorded six pages (`404.html` was served at
`/404.html`), both themes, and viewport widths 375, 768, and 1280 at device scale factor 1:

| Page slug | Source file | Requested path |
| --- | --- | --- |
| `404` | `404.html` | `/404.html` |
| `about` | `about/index.html` | `/about/` |
| `hello-zfb` | `blog/hello-zfb/index.html` | `/blog/hello-zfb/` |
| `markdown-showcase` | `blog/markdown-showcase/index.html` | `/blog/markdown-showcase/` |
| `styling-with-tailwind` | `blog/styling-with-tailwind/index.html` | `/blog/styling-with-tailwind/` |
| `home` | `index.html` | `/` |

The guarded capture command completed with 36 screenshot/snapshot pairs; PNGs totaled
5,888,154 bytes. The exact command was:

```sh
bash /Users/takazudo/.codex/scripts/heavy-guard.sh -- bash /Users/takazudo/.claude/scripts/playwright-guard.sh --wait 300 -- node /Users/takazudo/repos/myoss/zfb/worktrees/3245-baseline/_temp-resource/3242-owned-engines/visual-baseline/capture.mjs /tmp/zfb-3245-measurements/basic-blog-01 /Users/takazudo/repos/myoss/zfb/target/release/zfb /tmp/zfb-3245-visual-baseline-3 /Users/takazudo/repos/myoss/zfb
```

The PNGs were uploaded as issue assets and grouped in [the #3245 screenshot comment](https://github.com/Takazudo/zudo-front-builder/issues/3245#issuecomment-5861776245).
The committed temporary visual-baseline directory contains the capture script, manifest, and
36 computed-style JSON snapshots; it does not contain PNG files. Each snapshot records every
element in document order, its structural path, tag, class, first 160 characters of own text,
bounding box, and the computed-property list in `manifest.json`.

## Measurement limits

The helper records the build timing phases currently emitted by zfb; no CSS-only phase exists
in this revision. These measurements cover one macOS arm64 host and release builds. They do
not cover dev-server timing, memory, other platforms, interactive states, or CI runners.
The visual-baseline directory is temporary and will be removed before the root PR merges.

## Measurement helper source

The following is the committed `_temp-resource/3242-owned-engines/baseline/measure.mjs`,
embedded verbatim so later tasks can reproduce the same sampling and size collection method.

```js
#!/usr/bin/env node

/**
 * Reproducible release-build baseline collector for issue #3245.
 *
 * Usage:
 *   node measure.mjs <repo-root> <release-zfb-binary> <output-dir> [sample-count]
 *
 * All scaffold copies and build output stay in output-dir, which must be
 * outside repo-root. Each basic-blog sample gets a fresh copy for its cold
 * build and then reuses that copy for its warm build. The first sample's
 * built site is left in place for the separate browser capture script.
 */

import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { spawnSync } from "node:child_process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const [repoArg, zfbArg, outputArg, samplesArg = "5"] = process.argv.slice(2);

if (!repoArg || !zfbArg || !outputArg) {
  console.error(
    "usage: node measure.mjs <repo-root> <release-zfb-binary> <output-dir> [sample-count]",
  );
  process.exit(2);
}

const repoRoot = path.resolve(repoArg);
const zfbBinary = path.resolve(zfbArg);
const outputRoot = path.resolve(outputArg);
const sampleCount = Number.parseInt(samplesArg, 10);

if (!Number.isInteger(sampleCount) || sampleCount < 1) {
  throw new Error(`sample-count must be a positive integer; received ${samplesArg}`);
}
if (!fs.statSync(repoRoot).isDirectory()) {
  throw new Error(`repo-root is not a directory: ${repoRoot}`);
}
if (!fs.statSync(zfbBinary).isFile()) {
  throw new Error(`release zfb binary does not exist: ${zfbBinary}`);
}
if (Object.hasOwn(process.env, "ZFB_EMBEDDED_NODE_MODULES_CACHE")) {
  throw new Error(
    "ZFB_EMBEDDED_NODE_MODULES_CACHE must be unset for this measurement; unset it and rerun.",
  );
}

const relativeOutput = path.relative(repoRoot, outputRoot);
if (relativeOutput === "" || (!relativeOutput.startsWith("..") && !path.isAbsolute(relativeOutput))) {
  throw new Error(`output-dir must be outside repo-root: ${outputRoot}`);
}

fs.mkdirSync(outputRoot, { recursive: true });
if (fs.readdirSync(outputRoot).length > 0) {
  throw new Error(`output-dir must be empty: ${outputRoot}`);
}

const basicBlogTemplate = path.join(repoRoot, "crates/zfb/templates/basic-blog");
const smokeFixture = path.join(repoRoot, "tests/built-site-smoke/fixture-site");
const cachelessEnv = { ...process.env };
delete cachelessEnv.ZFB_EMBEDDED_NODE_MODULES_CACHE;

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function run(command, args = [], options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed with status ${result.status}:\n${result.stderr ?? result.stdout}`,
    );
  }
  return (result.stdout ?? "").trim();
}

function tryRun(command, args = [], options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    ...options,
  });
  if (result.error || result.status !== 0) return null;
  return (result.stdout ?? "").trim();
}

function git(args) {
  return run("git", ["-C", repoRoot, ...args]);
}

function commandVersion(command, args = ["--version"]) {
  return tryRun(command, args) ?? "unavailable";
}

function hostMetadata() {
  const platform = os.platform();
  const host = {
    hostname: os.hostname(),
    platform,
    release: os.release(),
    architecture: os.arch(),
    cpuModel: os.cpus()[0]?.model ?? "unknown",
    logicalCpuCount: os.cpus().length,
    memoryBytes: os.totalmem(),
  };

  if (platform === "darwin") {
    host.osVersion = tryRun("sw_vers", ["-productVersion"]) ?? os.release();
    host.cpuModel = tryRun("sysctl", ["-n", "machdep.cpu.brand_string"]) ?? host.cpuModel;
    host.logicalCpuCount = Number(tryRun("sysctl", ["-n", "hw.logicalcpu"]) ?? host.logicalCpuCount);
    host.memoryBytes = Number(tryRun("sysctl", ["-n", "hw.memsize"]) ?? host.memoryBytes);
  } else if (platform === "linux") {
    const distro = tryRun("bash", ["-lc", "sed -n 's/^PRETTY_NAME=//p' /etc/os-release"]);
    if (distro) host.osVersion = distro.replace(/^"|"$/g, "");
    const cpu = tryRun("bash", ["-lc", "lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -1"]);
    if (cpu) host.cpuModel = cpu;
    const memory = tryRun("bash", ["-lc", "awk '/MemTotal:/ {print $2 * 1024}' /proc/meminfo"]);
    if (memory) host.memoryBytes = Number(memory);
  } else {
    host.osVersion = os.version?.() ?? os.release();
  }

  return host;
}

function collectZfbEnvironment() {
  const result = {};
  for (const key of Object.keys(cachelessEnv).sort()) {
    if (key.startsWith("ZFB_")) result[key] = cachelessEnv[key];
  }
  result.ZFB_EMBEDDED_NODE_MODULES_CACHE = "unset (default)";
  return result;
}

function treeFiles(root) {
  const files = [];
  function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name);
      if (entry.isSymbolicLink()) {
        const link = fs.readlinkSync(full);
        files.push({ path: path.relative(root, full), bytes: 0, symlink: link });
      } else if (entry.isDirectory()) {
        walk(full);
      } else if (entry.isFile()) {
        files.push({ path: path.relative(root, full), bytes: fs.statSync(full).size });
      }
    }
  }
  walk(root);
  return files;
}

function fileTreeSummary(root) {
  const files = treeFiles(root);
  return {
    fileCount: files.filter((file) => !file.symlink).length,
    totalBytes: files.reduce((sum, file) => sum + file.bytes, 0),
    files,
  };
}

function copyFixture(source, destination) {
  const excludedNames = new Set(["node_modules", "dist", ".zfb-build", ".zfb"]);
  fs.cpSync(source, destination, {
    recursive: true,
    filter(sourcePath) {
      const relative = path.relative(source, sourcePath);
      if (!relative) return true;
      return !relative.split(path.sep).some((segment) => excludedNames.has(segment));
    },
  });
}

function timingLines(stderr) {
  return stderr.split(/\r?\n/).filter((line) => line.includes("[zfb-build-timing]"));
}

function runBuild(siteDir, label, timingEnabled) {
  const env = { ...cachelessEnv };
  if (timingEnabled) env.ZFB_BUILD_TIMING = "1";
  const start = performance.now();
  const result = spawnSync(zfbBinary, ["build"], {
    cwd: siteDir,
    env,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const elapsedMs = Number((performance.now() - start).toFixed(3));
  if (result.error) throw result.error;

  const stdout = result.stdout ?? "";
  const stderr = result.stderr ?? "";
  fs.writeFileSync(path.join(outputRoot, `${label}.stdout.log`), stdout);
  fs.writeFileSync(path.join(outputRoot, `${label}.stderr.log`), stderr);
  if (result.status !== 0) {
    throw new Error(
      `build failed (${label}) with status ${result.status}; see ${label}.stdout.log and ${label}.stderr.log in ${outputRoot}`,
    );
  }

  return {
    label,
    cwd: siteDir,
    command: `cd ${shellQuote(siteDir)} && ${timingEnabled ? "ZFB_BUILD_TIMING=1 " : ""}${shellQuote(zfbBinary)} build`,
    wallMs: elapsedMs,
    timingLines: timingLines(stderr),
    stdoutLog: `${label}.stdout.log`,
    stderrLog: `${label}.stderr.log`,
  };
}

function outputSummary(siteDir) {
  const distDir = path.join(siteDir, "dist");
  const summary = fileTreeSummary(distDir);
  const normalizedFiles = summary.files.map((file) => ({
    ...file,
    path: file.path.split(path.sep).join("/"),
  }));
  const htmlFiles = normalizedFiles.filter((file) => file.path.endsWith(".html"));
  const cssFiles = normalizedFiles.filter((file) => file.path.startsWith("assets/") && file.path.endsWith(".css"));
  const javascriptAssets = normalizedFiles.filter((file) => file.path.startsWith("assets/") && file.path.endsWith(".js"));

  return {
    fileCount: summary.fileCount,
    totalBytes: summary.totalBytes,
    htmlPages: htmlFiles.map(({ path: filePath, bytes }) => ({ path: filePath, bytes })),
    cssAssets: cssFiles.map(({ path: filePath, bytes }) => ({ path: filePath, bytes })),
    javascriptAssets: javascriptAssets.map(({ path: filePath, bytes }) => ({ path: filePath, bytes })),
    files: normalizedFiles,
  };
}

function sumPathBytes(root) {
  const stat = fs.lstatSync(root);
  if (stat.isSymbolicLink()) return 0;
  if (stat.isFile()) return stat.size;
  if (!stat.isDirectory()) return 0;
  return fs.readdirSync(root).reduce((sum, name) => sum + sumPathBytes(path.join(root, name)), 0);
}

function releaseVendorEntries(releaseBuildDir) {
  const found = tryRun("find", [
    releaseBuildDir,
    "-maxdepth",
    "3",
    "-type",
    "d",
    "-name",
    "vendor",
    "-path",
    "*zfb-*",
  ]);
  if (!found) return { matches: [], selected: null, entries: [] };
  const matches = found.split(/\r?\n/).filter(Boolean);
  const selected = matches
    .map((directory) => ({ directory, mtimeMs: fs.statSync(directory).mtimeMs }))
    .sort((a, b) => b.mtimeMs - a.mtimeMs)[0]?.directory;
  if (!selected) return { matches, selected: null, entries: [] };
  const entries = fs.readdirSync(selected).sort().map((name) => {
    const full = path.join(selected, name);
    return { name, bytes: sumPathBytes(full), isDirectory: fs.lstatSync(full).isDirectory() };
  });
  return {
    matches,
    selected,
    selectionRule: matches.length > 1 ? "most recently modified vendor directory" : "only match",
    entries,
    esbuildBytes: fs.existsSync(path.join(selected, "bin/esbuild")) ? fs.statSync(path.join(selected, "bin/esbuild")).size : null,
    tailwindBytes: fs.existsSync(path.join(selected, "bin/tailwindcss-v4")) ? fs.statSync(path.join(selected, "bin/tailwindcss-v4")).size : null,
  };
}

function stagedBinaries(binaryDir) {
  if (!fs.existsSync(binaryDir)) return [];
  return treeFiles(binaryDir)
    .filter((file) => {
      const basename = path.basename(file.path);
      return basename !== ".gitkeep" && basename !== "README.md" && !file.symlink;
    })
    .map((file) => ({ path: file.path.split(path.sep).join("/"), bytes: file.bytes }));
}

const tailwindMarkerDir = os.tmpdir();
const tailwindMarkers = fs.existsSync(tailwindMarkerDir)
  ? fs.readdirSync(tailwindMarkerDir).filter((name) => /^zfb-tailwind-oxide-warmup-.*\.done$/.test(name)).sort()
  : [];

const sampleResults = [];
let basicBlogOutput = null;
for (let index = 1; index <= sampleCount; index += 1) {
  const siteDir = path.join(outputRoot, `basic-blog-${String(index).padStart(2, "0")}`);
  copyFixture(basicBlogTemplate, siteDir);
  const cold = runBuild(siteDir, `basic-blog-${String(index).padStart(2, "0")}-cold`, true);
  const warm = runBuild(siteDir, `basic-blog-${String(index).padStart(2, "0")}-warm`, true);
  sampleResults.push({ index, siteDir, cold, warm });
  if (index === 1) basicBlogOutput = outputSummary(siteDir);
}

const smokeSiteDir = path.join(outputRoot, "built-site-smoke");
copyFixture(smokeFixture, smokeSiteDir);
const smokeBuild = runBuild(smokeSiteDir, "built-site-smoke", false);
const smokeOutput = outputSummary(smokeSiteDir);

const releaseDir = path.join(repoRoot, "target/release");
const releaseBinaryStat = fs.statSync(zfbBinary);
const vendor = releaseVendorEntries(path.join(releaseDir, "build"));
const staged = stagedBinaries(path.join(repoRoot, "crates/zfb/binaries"));
const mergeBase = git(["merge-base", "HEAD", "origin/main"]);
const preChangeDiffStat = tryRun("git", [
  "-C",
  repoRoot,
  "diff",
  "--stat",
  mergeBase,
  "HEAD",
  "--",
  "crates",
  "packages",
  "tests",
]);

const results = {
  metadata: {
    revision: git(["rev-parse", "HEAD"]),
    mergeBaseWithOriginMain: mergeBase,
    preChangeDiffStat: preChangeDiffStat || "(empty)",
    host: hostMetadata(),
    tools: {
      rustc: commandVersion("rustc"),
      node: commandVersion("node"),
      pnpm: commandVersion("pnpm"),
      zfb: commandVersion(zfbBinary),
    },
    zfbEnvironment: collectZfbEnvironment(),
    tailwindWarmupMarkersBeforeFirstColdSample: tailwindMarkers,
    tailwindWarmupMarkerDirectory: tailwindMarkerDir,
    zfbBinary: { path: zfbBinary, bytes: releaseBinaryStat.size },
    embeddedVendor: vendor,
    stagedBinaries: staged,
    helper: path.relative(repoRoot, path.join(scriptDir, "measure.mjs")),
  },
  sampleCount,
  basicBlog: {
    siteForVisualCapture: sampleResults[0]?.siteDir,
    samples: sampleResults,
    outputFromFirstSampleAfterWarmBuild: basicBlogOutput,
  },
  builtSiteSmoke: {
    siteDir: smokeSiteDir,
    build: smokeBuild,
    output: smokeOutput,
  },
};

fs.writeFileSync(path.join(outputRoot, "measurements.json"), `${JSON.stringify(results, null, 2)}\n`);

const rawLines = (key) => {
  const samples = sampleResults.map((sample) => sample[key]);
  return samples.map((sample) => `### ${sample.label}\nwall_ms=${sample.wallMs}\n${sample.timingLines.join("\n")}`).join("\n\n");
};
const timingStats = (key) => {
  const values = sampleResults.map((sample) => sample[key].wallMs).sort((a, b) => a - b);
  const median = values.length % 2
    ? values[(values.length - 1) / 2]
    : (values[values.length / 2 - 1] + values[values.length / 2]) / 2;
  return { minMs: values[0], medianMs: Number(median.toFixed(3)), maxMs: values.at(-1) };
};

const report = [
  `# Measurement output for issue #3245`,
  ``,
  `Output directory: ${outputRoot}`,
  `Revision: ${results.metadata.revision}`,
  `Host: ${JSON.stringify(results.metadata.host)}`,
  `Tools: ${JSON.stringify(results.metadata.tools)}`,
  `ZFB environment: ${JSON.stringify(results.metadata.zfbEnvironment)}`,
  `Tailwind warm-up markers before first cold sample: ${JSON.stringify(tailwindMarkers)}`,
  `Pre-change diff stat: ${results.metadata.preChangeDiffStat}`,
  ``,
  `Release binary bytes: ${results.metadata.zfbBinary.bytes}`,
  `Vendor directory selected: ${vendor.selected ?? "not found"}`,
  `Vendor matches: ${JSON.stringify(vendor.matches)}`,
  `Vendor entries: ${JSON.stringify(vendor.entries)}`,
  `Staged binary entries: ${JSON.stringify(staged)}`,
  `zfb --version: ${results.metadata.tools.zfb}`,
  ``,
  `Basic-blog cold wall time min/median/max (ms): ${JSON.stringify(timingStats("cold"))}`,
  `Basic-blog warm wall time min/median/max (ms): ${JSON.stringify(timingStats("warm"))}`,
  ``,
  `## Raw cold timing lines`,
  ``,
  "```text",
  rawLines("cold"),
  "```",
  ``,
  `## Raw warm timing lines`,
  ``,
  "```text",
  rawLines("warm"),
  "```",
  ``,
  `## Emitted output summaries`,
  ``,
  `Basic-blog: ${JSON.stringify(basicBlogOutput)}`,
  ``,
  `Built-site-smoke: ${JSON.stringify(smokeOutput)}`,
  ``,
  `Basic-blog capture site: ${sampleResults[0]?.siteDir}`,
  `Built-site-smoke build command: ${smokeBuild.command}`,
  ``,
].join("\n");
fs.writeFileSync(path.join(outputRoot, "measurements.md"), report);

console.log(`Measurements written to ${path.join(outputRoot, "measurements.md")}`);
console.log(`Structured data written to ${path.join(outputRoot, "measurements.json")}`);
console.log(`Basic-blog visual-capture site: ${sampleResults[0]?.siteDir}`);

```
