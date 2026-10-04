# Toolchain modernization: stage 4 Oxfmt and Markdown ownership evidence (#3558)

Compared with the stage 0 ledger ([`BASELINE.md`](./BASELINE.md)) and stage 3
([`STAGE-3-VITEST.md`](./STAGE-3-VITEST.md)). Implementation base: `1bf44d5e` (stage 3 merged into
`base/sweep-261001-toolchain-modernization`). Host: macOS 26.6.1 arm64, 8 GiB, Node v24.14.0,
pnpm 12.8.2, shared and loaded (load averages 7 to 29 during the checks). Every command ran in the
foreground with stdin closed. No Rust file was formatted and no `crates/*/templates` file changed.

| Commit | Content |
| --- | --- |
| `c43d466f` | Config and tooling: `vite.config.mjs`, scripts, hooks, dependencies, lockfile, CI filter, ownership suite |
| `cb3eb8e4` | **Pure reformat** (2 files), listed in `.git-blame-ignore-revs` |
| `0cbe5e82` | Contributor references (CONTRIBUTING, BUILDING, DEPENDENCIES, comments, release skill) |

`pnpm-lock.yaml` SHA-256 is now `781439c545bccb5fc18bbd3fae1b80276b5efbe6a696574a8bb208f750b145a5`
and both island-size contracts record it. A repeat `pnpm install` and `pnpm install --frozen-lockfile`
left it byte-identical.

## Oxfmt runs through `vp fmt` with the Prettier settings

The formatter is the `oxfmt` 0.70.0 that `vite-plus` 1.0.0 bundles. There is no separate Oxfmt
dependency or `.oxfmtrc.json`. Its config is the `fmt` block of a new root `vite.config.mjs`, which
Vite+ discovers from the working directory. `.mjs` matches the existing root `vitest.config.mjs`.
`vp test` keeps reading `vitest.config.mjs`: `vp test list --filesOnly` lists the same 24 root files
with and without `vite.config.mjs` present.

| `.prettierrc.json` / Prettier behavior | `fmt` block | Note |
| --- | --- | --- |
| `semi: true` | `semi: true` | |
| `singleQuote: false` | `singleQuote: false` | |
| `trailingComma: "all"` | `trailingComma: "all"` | |
| `printWidth: 100` | `printWidth: 100` | Oxfmt's default is also 100; stated explicitly |
| no package.json sorting | `sortPackageJson: false` | **Oxfmt sorts by default**; this keeps the grouped root scripts |
| no import sorting | not set | Oxfmt default is disabled; the ownership suite asserts it stays off |
| no Tailwind sorting | not set | default disabled; asserted |
| `.prettierignore` | `ignorePatterns` | every entry copied |
| reads root `.gitignore` | reads root `.gitignore` | Prettier 3 does (checked: an unformatted `.vitest/x.json` was skipped). The old `.prettierignore` comment saying otherwise was stale |
| `.editorconfig` | `.editorconfig` | both read it; explicit options win |

`ignorePatterns` also lists `*.md`, `*.mdx` and the other formats Oxfmt would format but Prettier
never did here (`*.html`, `*.css`, `*.toml`, `*.jsx`, `*.sublime-syntax`). A whitelist written with
gitignore negation (`*`, `!*/`, `!*.ts`, ...) re-included `dist/` fixture files, so the patterns are
plain exclusions.

## Oxfmt owns exactly Prettier's 855 files

Method: two scratch clones of `1bf44d5e`. Every tracked `js,mjs,cjs,ts,tsx,json,yml,yaml` file got
two trailing newlines so any formatter that reads it reports it. Then
`prettier --list-different "**/*.{…}"` (Prettier 3.8.3, old config) and
`vp fmt --list-different` (new config) ran on identical trees.

| Set | Files |
| --- | ---: |
| Tracked files with an owned extension | 901 |
| Prettier 3.8.3 with the glob | 855 |
| `vp fmt` with the same glob | 855, identical list |
| Bare `vp fmt` (every tracked text file mutated, all extensions) | 855, identical list |

The 46 excluded files are the same for both: `pnpm-lock.yaml` and the vite-6.4.3 fixture lockfile,
`expected.d.ts`, the 7 md-roundtrip `.mjs` goldens, the tracked `dist/` fixtures and the tracked
`node_modules/` fixtures. Templates under `crates/zfb/templates` are formatted by both, as before.
On the real dev tree `pnpm format:check:ts` reports 856 files (855 plus the new test suite) and
ignores local `node_modules/`, `target/` and `.vitest/`.

Explicit ignored paths (the lefthook case): `vp fmt pnpm-lock.yaml`, `expected.d.ts`, an md-roundtrip
file, `README.md` and a tracked fixture `package.json` each skip the file. With no file left Oxfmt
exits 2 ("Expected at least one target file"); Prettier exited 0. The `format-ts` hook therefore
passes `--no-error-on-unmatched-pattern`. A syntax error still exits 2 with it.

## Only two files differ from Prettier's output

`cb3eb8e4` is the whole reformat. Both differences are Oxfmt printer behavior with no option to
match Prettier:

| File | Prettier 3.8.3 | Oxfmt 0.70.0 |
| --- | --- | --- |
| `crates/zfb-md-wasm/npm/test/workerd.test.ts:35` | one 100-column `join(...)` line ending in a non-null `!` | wrapped over 4 lines |
| `research/v3-island-size/check-budget.mjs:958` | `for (let offset = 0; …; ) {` | `for (let offset = 0; …;) {` |

`check-budget.mjs` is not a hashed island-size input (the contracts hash fixtures, runner files and
lockfiles); `island-size-budget.test.mjs` passes. A second `pnpm format:ts` pass changes nothing.

## Protected files keep their bytes

Of the 1,733 tracked files outside the Oxfmt set at `1bf44d5e`, only `.prettierignore` (deleted)
and `pnpm-lock.yaml` (the dependency change) differ at `cb3eb8e4`. No `.md`/`.mdx` file changed in
any stage-4 commit. This covers `expected.d.ts`, the md-roundtrip fixtures, every template and
every HTML/CSS golden.

## mdx-formatter 1.2.1 is local, unchanged, and owns all Markdown

`@takazudo/mdx-formatter` moved from `pnpm dlx …@1.2.1` to an exact `"1.2.1"` root devDependency,
invoked as `mdx-formatter`. The installed manifest is version 1.2.1 with `bin.mdx-formatter` =
`./dist/cli.js`. Its graph is `chalk 5.6.2`, `commander 14.0.3`, `glob 13.0.6`, `js-yaml 4.3.2` and
four optional native packages (darwin arm64/x64, linux-x64-gnu, win32-x64). There is no install
script, so `allowBuilds` is unchanged. The registry marks 1.2.1 `deprecated: use @latest`; per
#3544 it is not upgraded here. No Linux arm64 native package exists, just as with `dlx`.

| Check | Result |
| --- | --- |
| Same-version parity: every tracked `.md`/`.mdx` (833) de-formatted the same way (blank lines around JSX/directive/fence lines dropped, `-` bullets → `*`, heading spaces widened), then `dlx @takazudo/mdx-formatter@1.2.1 --write` in one clone and the local bin in another | both formatted 814 files; all 833 outputs **byte-identical** |
| Idempotence | a second local `--write` changes nothing; `--check` passes |
| Clean tree | `pnpm format:check:mdx` passes |
| Oxfmt leaves Markdown alone | on the de-formatted corpus, bare `vp fmt --write`, `vp fmt --write "**/*.{md,mdx}"` and explicit `.md`/`.mdx` paths left all 833 hashes unchanged |

`scripts/__tests__/formatter-ownership.test.mjs` (23 cases, in-process, in the root `scripts`
project) keeps this enforced. It covers:

- the `fmt` settings and exclusions, and the exact `format*` scripts and hook commands.
- the absence of Prettier.
- the exact 1.2.1 pin, the `.mdx-formatter.json` block components and `HtmlPreview` exclusion.
- fixpoints on existing formatted files: `:::note` and titled admonitions, `Details`/`Note`, nested
  `::::` directives, `Tabs`, frontmatter, fences, and the Japanese admonitions page.
- pinned 1.2.1 outputs for JSX block spacing, `Tabs`, `Details`, titled directives, nested
  directives, Japanese text, frontmatter with a fence, and two `HtmlPreview` cases.

The `HtmlPreview` exclusion is load-bearing: without it, 1.2.1 rewrites the template-literal
`html={…}` case into broken JSX.

## Hooks keep their behavior

`lefthook.yml` keeps both commands, their disjoint globs, `parallel` and `stage_fixed`. Only the
`run` lines changed. Exercised in the worktree with `lefthook run pre-commit` (then unstaged):

- `tmp hook probe/a b.ts` and `tmp hook probe/note x.mdx`: both formatted and re-staged.
- A commit staging only `pnpm-lock.yaml`: the hook passes and leaves the lockfile unformatted.
- The three stage-4 commits ran the new hooks.

`scripts/install-git-hooks.sh` and the pre-push guard are untouched. `format:check` still runs
`format:check:ts && format:check:mdx`, both non-mutating, and health and b4push still call
`pnpm format:check`.

## Format checks are faster (loaded host, alternated)

Three rounds alternated the stage-3 base (a scratch clone of `1bf44d5e` with Prettier 3.8.3 and its
config) and this branch on the same host. Load was 6.9 to 9.2.

| Command | Baseline runs (s) | Median | Candidate | Runs (s) | Median |
| --- | --- | ---: | --- | --- | ---: |
| ordinary code | `prettier --check` 11.32 / 5.16 / 5.23 | **5.23** | `vp fmt --check` | 1.49 / 1.36 / 1.30 | **1.36** |
| Markdown | `pnpm dlx …@1.2.1 --check` 7.17 / 4.32 / 5.54 | **5.54** | local `mdx-formatter --check` | 3.52 / 3.37 / 3.66 | **3.52** |
| `pnpm format:check` | stage 0 ledger 9.80, stage 3 11.4 | | | 6.62 / 6.82 / 7.61 | **6.82** |

These are single-host numbers, not a CI claim. Dropping `dlx` also removes the registry lookup
from every Markdown check and pre-commit.

## Gates

| Command | Result |
| --- | --- |
| `pnpm format:check` | pass (856 Oxfmt files, mdx-formatter clean) |
| `pnpm typecheck:workspace` | pass, 7.7 s |
| `pnpm test:workspace` | **failed twice on load**, each time one `island-size-budget` case at the root 5 s timeout (#3630), on a different case each run, load 12 to 29. Separately: root project 25 files / 536 cases pass; `island-size-budget` alone passes. Its slowest cases are 2.5 to 3.5 s on both base and branch, alternated, so this is not a regression |
| Other suites from those runs | docs 5/25, adapter 5/40, zfb 49/720, create-zfb 2/24, zfb-runtime 17/300 pass |

Totals are 103 files / 1,645 cases: stage 3's 102 / 1,622 plus the new 23-case ownership suite.
No suite was lost or duplicated.

## CI path filters

Health's `wasm_md` filter swaps `.prettierrc.json` and `.prettierignore` for `vite.config.mjs`.
`lefthook.yml`, `.mdx-formatter.json`, `.editorconfig`, `.gitignore` and `package.json` stay.
No other workflow filtered on a formatter file.

## CI must prove

- `health`:
  - `pnpm format:check` on Linux x64 Node 22: Oxfmt finds the same 856 files, and mdx-formatter
    resolves `@takazudo/mdx-formatter-linux-x64-gnu` from the frozen lockfile.
  - `test:workspace` with 103 files / 1,645 cases, including `formatter-ownership` (native
    formatter on Linux).
  - `typecheck:workspace`, and the island-size step with the new `pnpmLockSha256`.
- `pnpm audit (prod)` with Prettier removed and mdx-formatter's dev graph added.

## Regressions versus the baseline

- None found. Two reformatted lines are unavoidable printer differences (above).
- The root 5 s timeout risk from stage 3 (#3630) reproduced under local load. It is not caused by
  this stage and no timeout was raised.

## Not run here

Cargo builds, tests and `cargo fmt` (no Rust logic changed; two Rust comment/string edits only),
the md-wasm build, docs strict build, browser lanes and Windows were not run, per the 8 GiB host
rule. A Linux mdx-formatter run is left to CI.
