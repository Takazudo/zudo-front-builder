---
description: "Bump the @takazudo/* npm dependencies in the zfb demo repos (../../zfb-ex/*) to their latest published version, commit, and push. For each demo: checkout main, pull, bump every @takazudo/* dep (zfb, zfb-runtime, zfb-adapter-cloudflare, …) to the newest published version (lockstep), refresh the pnpm lockfile, verify the build, then push to main when green — or open a PR and apply fixes when the bump breaks the build. Run it right after /l-make-release so the demos pick up the just-published version. Triggers on \"bump demo deps\", \"update the demos\", \"bump deps for demos\", \"bump zfb-ex\", \"update example repos\"."
user-invocable: true
argument-description: "Optional: a demo name or substring (e.g. webshop) to limit to one repo — default is all demos under ../../zfb-ex/*. --tag <next|latest|X.Y.Z-...> to force the target version/dist-tag (default: the demo's own channel — next for prerelease pins, latest otherwise). --pr to always open a PR instead of pushing to main. --dry-run to report the planned bumps without writing/committing/pushing. --confirm to print the plan and wait before the first push (default is fully autonomous)."
---

# /l-bump-deps-for-demos

Keep the zfb example/demo repositories in sync with the latest published
`@takazudo/*` packages. This is the companion to [`/l-make-release`](../l-make-release/SKILL.md):
once a new `@takazudo/zfb` version is on npm, run this to roll every demo forward
to it.

The demos live **outside** this repo, at `../../zfb-ex/*` (i.e.
`/Users/takazudo/repos/zfb-ex/*` when this repo is at
`/Users/takazudo/repos/myoss/zfb`). They are independent GitHub repos under the
`Takazudo` org, each a single (non-workspace) pnpm package that pins its
`@takazudo/*` deps to **exact** versions resolved from the npm registry.

## Invocation & autonomy

This skill is **model-invocable**: a rough request like "bump the demos",
"update the example repos", or "bump deps for demos" triggers it.

**Default: fully autonomous.** Discover the demos, bump each, verify, and push —
no confirmation prompts. The only stop is a genuine blocker the skill cannot
safely resolve (see "When to stop"). Print the plan as you go for visibility,
but do not wait. `--confirm` restores an interactive checkpoint: print the plan
and wait for the user before the first push.

## Boundaries

- Operates **only** on the demo repos under `../../zfb-ex/*`. Never touches this
  (`zfb`) repo's own packages or version.
- Bumps **only** `@takazudo/*`-scoped dependencies (in both `dependencies` and
  `devDependencies`). Other deps (wrangler, tailwindcss, …) are left untouched.
- Pushes to a demo's `main` **only after** its build verifies green. A bump that
  breaks the build never lands on `main` — it goes to a PR instead (see Step 6).
- This repo's worktree-push policy does **not** apply to the demo repos — they
  are separate repositories with their own remotes. Push to their `main`
  directly per the happy path below.

## Step 0: Resolve the demos directory

```bash
REPO_ROOT=$(git rev-parse --show-toplevel)
DEMOS_DIR=$(cd "$REPO_ROOT/../../zfb-ex" && pwd)   # -> /Users/takazudo/repos/zfb-ex
ls -d "$DEMOS_DIR"/*/ 2>/dev/null
```

Each subdirectory that is a git repo **and** has a `package.json` containing at
least one `@takazudo/*` dependency is a target demo. Skip anything that is not a
git repo or has no `@takazudo/*` dep.

If an argument names a demo (e.g. `webshop`), filter to directories whose name
contains that substring. If the filter matches nothing, list what was found and
stop.

As of writing, the demos are:

| Demo | `@takazudo/*` deps | Build command |
|---|---|---|
| `zfb-example-blog` | `@takazudo/zfb`, `@takazudo/zfb-runtime` | `pnpm build` (`zfb build`) |
| `zfb-example-corporate-website` | `@takazudo/zfb`, `@takazudo/zfb-runtime` | `pnpm build` (`zfb build`) |
| `zfb-example-webshop` | `@takazudo/zfb`, `@takazudo/zfb-runtime`, `@takazudo/zfb-adapter-cloudflare` | `pnpm build` (`zfb build && node scripts/stable-css.mjs`) |

Do **not** hardcode this list — always re-discover from `package.json`. The table
is just the current shape.

## Step 1: Determine the target version

The `@takazudo/zfb*` packages are released in **lockstep** — `@takazudo/zfb`,
`@takazudo/zfb-runtime`, and `@takazudo/zfb-adapter-cloudflare` always share the
same version. So the target version is driven by `@takazudo/zfb`.

Pick the target from the dist-tag that matches the demo's current channel, unless
`--tag` overrides it:

```bash
# A demo on a prerelease pin (current version contains "-", e.g. 0.1.0-next.38)
# follows the `next` channel; a stable pin follows `latest`.
npm view @takazudo/zfb dist-tags --json
```

- Current pin is a prerelease (`-next.` / `-beta.` / `-rc.`) → target =
  `dist-tags.next` (fall back to `dist-tags.latest` if `next` is absent).
- Current pin is stable → target = `dist-tags.latest`.
- `--tag next` / `--tag latest` → use that dist-tag explicitly.
- `--tag X.Y.Z-...` (looks like a version) → use that exact version.

> **Dual-tag note (prerelease era).** Until the first stable `@takazudo/zfb` is
> cut, `release.yml` advances **both** `next` and `latest` on every `*-next.*`
> publish, so the two dist-tags currently point at the same version. Preferring
> `next` for prerelease pins is still correct and is future-proof for when the
> tags diverge.

For each individual `@takazudo/*` dep in the demo, resolve its own target the
same way (query `npm view <name> dist-tags`). In practice every `@takazudo/zfb*`
dep resolves to the same lockstep version; resolving per-dep keeps the skill
correct if a non-lockstep `@takazudo/*` package is ever added.

**Idempotency:** if a demo's deps are already at the target version, skip it (no
commit, no push) and record it as "already up to date".

## Step 2: Sync the demo to a clean main

For each demo, in its own directory:

```bash
cd "$DEMOS_DIR/<demo>"
git fetch origin
# Refuse to proceed on a dirty tree — surface it rather than stash/clobber.
test -z "$(git status --porcelain)" || { echo "DIRTY: <demo> has uncommitted changes — skipping"; }
git checkout main
git pull --ff-only origin main
```

If the working tree is dirty, **do not** bump that demo — report it as skipped
(dirty) and move on. The user can clean it up and re-run.

## Step 3: Bump the `@takazudo/*` versions in package.json

Update each `@takazudo/*` entry in `dependencies` and `devDependencies` to its
target version, **preserving the existing range style**. The demos pin exact
versions (no `^`/`~`), so a `0.1.0-next.38` → `0.1.0-next.56` swap is a literal
string replacement of the version. If a dep uses a `^`/`~` range, keep the
prefix and replace the version.

Use a precise editor (jq or a small node script) rather than blind sed so only
the `@takazudo/*` keys change:

```bash
node -e '
const fs=require("fs"); const p="package.json";
const j=JSON.parse(fs.readFileSync(p,"utf8"));
const targets=JSON.parse(process.env.TARGETS); // { "@takazudo/zfb":"0.1.0-next.56", ... }
for (const field of ["dependencies","devDependencies"]) {
  if (!j[field]) continue;
  for (const [name,spec] of Object.entries(j[field])) {
    if (!name.startsWith("@takazudo/")) continue;
    if (!(name in targets)) continue;
    const m = String(spec).match(/^([\^~]?)/);
    j[field][name] = (m?m[1]:"") + targets[name];
  }
}
fs.writeFileSync(p, JSON.stringify(j,null,2)+"\n");
'
```

(Match the file's existing indentation/trailing-newline style — most demos use
2-space indent + trailing newline, which the snippet above produces. If a demo
differs, adjust so the diff is version-only.)

## Step 4: Refresh the lockfile

Run a normal install in the demo so `pnpm-lock.yaml` updates to the new versions.
Run pnpm **from inside the demo dir** so its pinned `packageManager`
(`pnpm@10.30.0`) is honored by corepack:

```bash
COREPACK_ENABLE_DOWNLOAD_PROMPT=0 pnpm install 2>&1 | tail -8
```

- This pulls the new `@takazudo/*` tarballs (and the platform-specific
  `@takazudo/zfb-<os>-<arch>` optional dep that ships the binary).
- If corepack cannot fetch the pinned pnpm, fall back to the ambient `pnpm`
  (note it in the report — a lockfile produced by a different pnpm major is still
  valid but may reformat; prefer the pinned one).
- Confirm the lockfile diff is **version-only** (the bumped `@takazudo/*`
  specifiers + their resolved entries). **Expected/benign exception:** the new
  `@takazudo/*` version may carry new metadata that the lockfile records —
  notably the `peerDependencies` / `peerDependenciesMeta` that the published
  package itself declares (e.g. next.56 added an **optional** `react` peer:
  `react: ^19.2.3` with `peerDependenciesMeta.react.optional: true`). That flows
  from the package, not from repo churn, and is harmless for the Preact demos
  (react stays uninstalled). Do **not** treat it as drift. Only surface
  genuinely unrelated changes (an unrelated dep version moving, structural
  reshuffles not tied to the `@takazudo/*` bump).

## Step 5: Verify the build

Before anything is committed or pushed, prove the bump actually works:

```bash
COREPACK_ENABLE_DOWNLOAD_PROMPT=0 pnpm run typecheck 2>&1 | tail -20   # zfb check
COREPACK_ENABLE_DOWNLOAD_PROMPT=0 pnpm run build 2>&1 | tail -30       # zfb build (+ post steps)
```

- The freshly installed `@takazudo/zfb` provides the `zfb` CLI binary; the build
  exercises the new engine end-to-end against the demo's real content. This is
  the meaningful signal that the bump is safe.
- **webshop** caveat: its `build` is `zfb build && node scripts/stable-css.mjs`
  and it also has Cloudflare/D1 (`dev:cf*`) scripts. Only run `build` — do **not**
  run the D1/wrangler dev loop (it needs local D1 provisioning). `zfb build`
  itself does not need D1.
- If `typecheck` or `build` fails → this is the "needs fixes" path (Step 6, PR).

## Step 6: Land the change

### Happy path — build green → push to main

```bash
git add package.json pnpm-lock.yaml
git commit -m "chore(deps): bump @takazudo/* to <version>"
git push origin main
```

Then watch the demo's CI (`deploy.yml` runs on push to main). Report the run URL;
if it goes red for a reason caused by the bump, fix forward (commit + push) or,
if the fix is non-trivial, open a follow-up PR with the fix and note it.

Under `--pr`, skip the direct push: create `chore/bump-takazudo-<version>` from
main, commit there, push the branch, and open a PR with `gh pr create --base main`.

### Needs-fixes path — build red → branch + PR + fix

When the bump breaks `typecheck`/`build`, do **not** push to main. Instead:

1. Create a branch: `git checkout -b chore/bump-takazudo-<version>`.
2. Commit the version bump.
3. **Diagnose and fix** the breakage — it is almost always a zfb behavior/API
   change between the old and new version. Read the failing output, consult this
   repo's changelog (`docs/src/content/docs/changelog/`) for the versions
   crossed, adjust the demo's code/config, and re-run Step 5 until green.
4. Commit the fixes, push the branch, and open a PR:
   ```bash
   gh pr create --base main \
     --title "chore(deps): bump @takazudo/* to <version>" \
     --body "<what changed + the fixes the bump required + verification>"
   ```
5. If `--confirm` was NOT passed and the fixes are verified green, you may merge
   the PR (`gh pr merge --merge` after CI passes). Otherwise leave it open for the
   user.

## Step 7: Report

Print one table covering every demo:

```
| demo | from → to | build | landed |
|------|-----------|-------|--------|
| zfb-example-blog | 0.1.0-next.38 → 0.1.0-next.56 | ✅ | pushed to main (run: <url>) |
| zfb-example-webshop | … | ❌→✅ after fix | PR #N |
| ... |
```

Include: skipped (already up to date / dirty), pushed-to-main, opened-PR, and any
demo left in a needs-attention state. End with the target version and a one-line
summary.

## When to stop (the only blockers)

Stop and surface to the user — do not guess — when:

- A demo's working tree is dirty (report skipped; do not clobber).
- `npm view` cannot resolve a target version for a `@takazudo/*` dep (network or
  the package is unpublished).
- A build break needs a **product/design decision** in the demo (not a mechanical
  API adjustment) — open the PR with the bump + a description of the breakage and
  leave it for the user rather than inventing demo content.

Everything else (lockfile refresh, mechanical API adjustments, CI watching) is
handled autonomously.

## Notes

- The demos are pre-release showcases; pushing to their `main` triggers their
  `deploy.yml`. That is intended — the point is to ship the demos on the new
  engine.
- Run this **after** `/l-make-release` has published, so `npm view` already sees
  the new version. If run before publish, the demos will just bump to the prior
  latest (or no-op if already there).
- Keep the bump version-only on the happy path; mix code fixes in only on the
  needs-fixes (PR) path so the happy-path diff stays trivially reviewable.
