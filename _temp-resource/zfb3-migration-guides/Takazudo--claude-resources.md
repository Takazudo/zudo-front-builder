# zfb v3 migration guide: Takazudo/claude-resources

Generated 2026-10-04 by an automated diagnosis of `main` @ `8eb08d1`. Counts come from the commands listed; re-run them locally before relying on them.

`claude-resources` is the **public mirror** of the private `Takazudo/claude-settings` repo. Its copy of `skills/dev-basic-cloudflare-webapp/` is byte-identical to the private source (`diff -r` → no output) and is overwritten on every `/claude-resources-share` run (one-direction rsync of `$HOME/.claude/skills/` → `skills/`, see `claude-settings/skills/claude-resources-share/SKILL.md`). **Do not edit this repo directly**; apply the sibling guide `Takazudo--claude-settings.md` to the source and re-share. This guide records what is wrong here, why it matters more here than in the private repo, and the exact re-sync procedure.

## Verdict

**Small, effort S (propagation only), no external gate.** The repo contains no zfb project (`find . -name 'zfb.config.*'` → 0; no `.css`, `.tsx` or pages anywhere), so there is nothing to migrate at runtime and `zfb wind audit` / `zfb check` cannot run. It does ship the house scaffold template, and that template has been producing broken projects since 2026-09-30: `skills/dev-basic-cloudflare-webapp/assets/templates/package.json:40-42` pins `@takazudo/zfb`, `@takazudo/zfb-adapter-cloudflare` and `@takazudo/zfb-runtime` to `"latest"` (now **3.1.0** for all three per `npm view … dist-tags`, 2026-10-04) next to `preact ^10.29.1` / `preact-render-to-string ^6.6.6` (`:43-44`), `tsconfig.json:9` `"jsxImportSource": "preact"`, and prose that instructs `framework: "preact"` and `@import "tailwindcss"` (`references/stack.md:22,54`). On zfb 3 those are a config-loading error and a ZW009 error respectively (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#configuration, #stylesheets). This copy matters **more** than the private one: `README.md` advertises installation as a Claude Code plugin (`/plugin marketplace add takazudo/claude-resources`), so external users scaffold from it. The whole fix is: update the source repo (effort M there), run `/claude-resources-share -a`, confirm `diff -r` is empty. If the source fix is delayed, the one-line interim pin (`"2.22.1"` ×3) must also flow through the same share — never as a direct edit here, or the next share reverts it.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `skills/dev-basic-cloudflare-webapp/assets/templates/` (17 files; identical to `claude-settings` @ `939a657`) | `"latest"` ×3 (`package.json:40-42`) → 3.1.0; scripts `zfb dev`, `zfb build`, `zfb preview`, `typecheck: zfb check` (`:12-14,17`); **no `zfb.config.*`** | none as deps; zudo-doc recommended in prose (`SKILL.md:35`, `stack.md:21`); `@takazudo/zudo-design-token-lint: latest` (`:51`) | no CSS, no `tailwind.config.*`, no `@tailwindcss/*`; stale `gitignore:14-15` `zfb-tailwind-entry-*.css`; 0 `ZFB_TAILWIND*` | `preact ^10.29.1`, `preact-render-to-string ^6.6.6` (`:43-44`); `tsconfig.json:9` `jsxImportSource: "preact"`; 0 TSX files, 0 hooks | none shipped | none | `wrangler.toml` (Workers Static Assets, `main = "./dist/_worker.js"`, `nodejs_compat`), `deploy.yml`, `ci.yml`, `run-b4push.sh` — adapter-facing, **unchanged by v3** |
| `skills/dev-basic-cloudflare-webapp/SKILL.md` + `references/*.md` (prose, identical to source) | `stack.md:20,46-48` | `SKILL.md:35`; `stack.md:21,64-66,143,147` | `SKILL.md:34,38`; `stack.md:54,60-66,131,133,148`; `tooling.md:223-231` | `stack.md:22`; `tooling.md:128-149,219-221`; `testing.md:10,49-67`; `conventions.md:25` | `SKILL.md:36` (SSR row, still valid) | `stack.md:145` | `ci.md`, `cloudflare.md`, `cloudflare-setup.md`: 0 Preact/Tailwind mentions |
| Everything else (`commands/`, `agents/`, `hooks/`, `scripts/`, `web/`, other skills, `.claude-plugin/`) | operational mentions only (`dev-bump-zudo-deps`, `dev-reduce-deps/references/verdicts.md:118,140,180`, `cleanup-resources` `zfb-shadow-session-*` sweep, `dev-clean-mac` `.zfb-build/`) | — | — | — | — | — | **not a zfb consumer; unaffected** |

Measured with: `diff -r /home/user/claude-settings/skills/dev-basic-cloudflare-webapp /home/user/claude-resources/skills/dev-basic-cloudflare-webapp` → identical; the briefing greps on `skills/dev-basic-cloudflare-webapp` → Tailwind 3 prose hits (`stack.md:54,65,66`), Preact 4 hits (`stack.md:22`, `tooling.md:138`, `package.json:44`, `tsconfig.json:9`), hooks/`className`/`<Island` 0, `ZFB_TAILWIND|framework:|tailwind:` → `stack.md:22` only; `git log -1 -- skills/dev-basic-cloudflare-webapp` → `8eb08d1 2026-09-26 chore: sync web profile fixes from private .claude` (the last share, three days before the source's latest commit and four days before zfb 3.0.0). `zfb wind audit` / `zfb check` **skipped**: no zfb project dir.

### What a scaffold produced from this repo does today

Reproduced from the template contents, not by running it (no `pnpm install` was executed in this diagnosis):

1. `pnpm install` resolves `@takazudo/zfb@latest` → 3.1.0 (plus the 3.1.0 platform binary through `optionalDependencies`), `@takazudo/zfb-runtime@latest` → 3.1.0, `@takazudo/zfb-adapter-cloudflare@latest` → 3.1.0, and still installs `preact` 10.29.x and `preact-render-to-string` 6.6.x (`package.json:40-44`). `pnpm-workspace.yaml:16` `minimumReleaseAge: 0` means the 2026-09-30 release was picked up the same day.
2. The agent, following `references/stack.md:22,46-48,54`, writes a `zfb.config.ts` with `framework: "preact"`, an `adapter` line and (per `SKILL.md:34`) a Tailwind entry sheet with `@import "tailwindcss"` and `@theme`.
3. `pnpm dev` / `pnpm build` (`zfb dev` / `zfb build` 3.1.0) stops at config loading with `the framework key was removed in zfb 3; delete the key. zfb now uses zudo-react.` — or, if a `tailwind` key was written, `the tailwind key was removed in zfb 3; utilities are compiled by the built-in zudo-wind engine. …` (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#configuration).
4. After deleting the keys, the stylesheet fails with **ZW009** listing every leftover Tailwind directive with `file:line:column`, even under `wind: false` (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#stylesheets).
5. After removing the directives, every colour/spacing/font utility the agent wrote is **ZW006** "missing token" because v3 ships no palette or spacing unit (https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#theme-and-tokens), and any component written with `useState`/`className`/`onClick` against `jsxImportSource: "preact"` (`tsconfig.json:9`) is typed against Preact while zudo-react renders it (https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/).
6. `pnpm typecheck` (`zfb check`) and `ci.yml`'s Build step go red; `deploy.yml` never reaches `wrangler deploy`. Nothing in the Cloudflare half (`wrangler.toml`, `deploy.yml`, `smoke.mjs`) is at fault.

## Sequencing and blockers

1. **Upstream of this repo:** fix `Takazudo/claude-settings` (sibling guide; steps 1-5 there). Nothing in that fix waits on an unreleased package — zfb 3.1.0, adapter 3.1.0 and `create-zfb` 3.1.0 are all on npm `latest`.
2. **This repo, S:** run `/claude-resources-share -a` from the fixed `$HOME/.claude`. The skill scans for private info first, rsyncs `skills/` (excluding `node_modules/`, symlinks, lockfiles), preserves `.claude-plugin/`, and commits + pushes in auto mode. Verify with `diff -r`.
3. **Do not** hand-edit `skills/dev-basic-cloudflare-webapp/**` here. The share's Step 3 cleanup removes and re-copies `skills/`, so a local-only fix is lost on the next run and the public copy silently regresses.
4. **After zudo-doc 6.0.0** (zudolab/zudo-doc#4430 / #4477 / #4473; its integration needs zfb fixes that are on `main` but unreleased, so it will target the **next zfb release**): the source's docs-site prose (`SKILL.md:35`, `stack.md:21,64-66,143`, `conventions.md:93-102`) gets rewritten and re-shared. Until then the shared text must carry the "zudo-doc 5.28.x needs zfb 2.22.1; do not mix with 3.x" caveat.
5. **Next zfb release** features (`wind.strict`, `utilities.placement`, `zfb wind manifest`, `--json` audit): nothing in this template depends on them; label them if the prose mentions them.

## Required changes

All changes land in the **source repo**; this section lists them so a reviewer of the public repo can confirm the share picked them up. Paths and line numbers are identical in both repos today.

### 1. Dependencies, config, tsconfig, env

- `assets/templates/package.json:40-42` — `"latest"` → `"3.1.0"` exact, lockstep (interim: `"2.22.1"`). A scaffold template on a dist-tag inherits every future major; `SKILL.md:119`'s "replace `latest` after the first install" runs after the damage.
- `package.json:43-44` — remove `preact`, `preact-render-to-string` (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#components-and-islands). Keep `@takazudo/zfb-runtime`.
- `tsconfig.json:9` — `"jsxImportSource": "@takazudo/zfb/zudo-react"`.

  ```json
  { "compilerOptions": { "jsx": "react-jsx", "jsxImportSource": "@takazudo/zfb/zudo-react" } }
  ```

- New `components/zfb-shim.d.ts` (`declare module "zfb/config" { export * from "@takazudo/zfb/config"; }`, https://zfb.takazudomodular.com/docs/api/define-config/#typing-the-zfb-config-import) and a `zfb.config.ts` with `adapter: "@takazudo/zfb-adapter-cloudflare"` and a `wind` block (`spec: 1`, explicit `reset`, `tokens`, `breakpoints`, `dark`; https://zfb.takazudomodular.com/docs/zudo-wind/configuration/). No `framework`, no `tailwind` key.

  ```ts
  import { defineConfig } from "zfb/config";
  export default defineConfig({
    adapter: "@takazudo/zfb-adapter-cloudflare",
    wind: {
      spec: 1,
      reset: "owned-v1",
      tokens: { spacingUnit: "0.25rem", colors: { surface: "var(--color-surface)", fg: "var(--color-fg)", accent: "var(--color-accent)" } },
      breakpoints: { sm: { minWidthPx: 640 } },
      dark: { attribute: "data-theme", value: "dark" },
      authoredClasses: { prose: true },
    },
  });
  ```

- `gitignore:14-15` — drop `zfb-tailwind-entry-*.css` (zfb ≤ 2.x temp file; 0 hits in v3 source).
- `wrangler.toml`, env: unchanged (0 `ZFB_TAILWIND*`; adapter 3.x has no package-specific change — https://zfb.takazudomodular.com/docs/changelog/zfb-adapter-cloudflare/v3.0.0/).

### 2. CSS and utilities

- `stack.md:54` Tailwind v4 → zudo-wind (`wind.tokens`, no CSS directives; every Tailwind directive is ZW009 — https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#directives). Tailwind v4 stays correct only for the Vite + React "Interactive app" row (`SKILL.md:38`), which does not go through zfb.
- `stack.md:60-66,131,133,148`, `tooling.md:223-231` — semantic tokens become `wind.tokens` entries pointing at authored custom properties; v3 ships no palette/spacing scale (https://zfb.takazudomodular.com/docs/zudo-wind/tokens/); "Tailwind token linter" → "utility class-name token linter".
- Add the unsupported list (`ring-*`, `animate-*`, `scale-*`, arbitrary properties, `!important`, attribute/arbitrary-selector variants, named `group/peer`, `@apply`; ZW014 for `line-clamp-*`, `container`, `order-*`, `basis-*`) — https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#what-is-not-supported-and-what-to-do-instead.

### 3. Components and islands

- `SKILL.md:34` "zfb + Preact + Tailwind v4" → "zfb 3 (zudo-react + zudo-wind)".
- `stack.md:20,22` → zudo-react (`@takazudo/zfb/zudo-react`): `signal()`/`computed()`, `Show`/`For`, `on:click`, `class`/`for`; no Preact/React, no compat aliases; third-party widgets via https://zfb.takazudomodular.com/docs/concepts/islands/#embedding-a-third-party-widget.
- `stack.md:23` keep Vite + React for genuinely interactive apps — zudo-react has no hooks/context/portals/Suspense (https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/#what-does-not-exist).
- `tooling.md:128,138` tsconfig baseline → `@takazudo/zfb/zudo-react`; `:219-221` scope `eslint-plugin-react(-hooks)` to the Vite + React shape only.
- `conventions.md:25` `components/ Preact/React components` → zudo-react components / `"use client"` islands in `<Island>`.

### 4. md-wasm and other packages

- `stack.md:145` md-wasm: no `jsxRuntime` on 3.x; React/Preact consumers of `compile()` output are unsupported — use `renderHtml()` or md-wasm 2.x (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#mdx-and-md-wasm).
- `SKILL.md:35`, `stack.md:21,143` zudo-doc: add the 5.28.x ↔ zfb 2.22.1 lock and the 6.0.0 status (zudolab/zudo-doc#4430). `stack.md:147` zdtp: Preact widget, embedded opaquely on v3 (Takazudo/zudo-design-token-panel#1002).
- `skills/dev-reduce-deps/references/verdicts.md:118,140,180`: append a dated row that the KEEP verdicts for `preact`/`preact-render-to-string` under zfb hosts end on a v3 host.

### 5. Tests, CI, deploy

- `testing.md:10` → `vitest + happy-dom + @takazudo/zfb/zudo-react/testing`; `testing.md:49-67` replace the Preact-compat alias block with `esbuild: { jsx: "automatic", jsxImportSource: "@takazudo/zfb/zudo-react" }`, `test.environment: "happy-dom"` (https://zfb.takazudomodular.com/docs/zudo-react/testing/#configure-jsx-and-a-dom).
- `ci.yml` + `scripts/run-b4push.sh`: add `pnpm exec zfb wind audit --fail-on error` after Build, in both (https://zfb.takazudomodular.com/docs/api/cli/#zfb-wind).
- `deploy.yml`, `actionlint.yml`, `smoke.mjs`, `wrangler.toml`, `ci.md`, `cloudflare.md`: no change.

### 6. Share procedure (how the fix reaches this repo)

From `claude-settings/skills/claude-resources-share/SKILL.md`, the steps that matter for this template:

| Step | What it does | Why it matters here |
| --- | --- | --- |
| 1 Scan | `/purge-private-info` over `commands/ skills/ agents/ hooks/ scripts/ web/ CLAUDE.md`; auto mode proceeds only on a clean scan | The rewritten prose must not add `$HOME/...` paths or account names, or the share stops |
| 2 Prepare | `mkdir -p $HOME/repos/p/claude-resources`, pull latest | Needs the public clone on the machine that holds the fixed `$HOME/.claude` |
| 3 Cleanup + rsync | removes and re-copies `commands/ skills/ agents/ hooks/ scripts/ web/ CLAUDE.md` with `--no-links`, excluding `node_modules/`, lockfiles, `dist/`, `target/` … ; never deletes `.claude-plugin/` | This is why a direct edit here is overwritten, and why `skills/dev-basic-cloudflare-webapp/` lands as an exact copy |
| 4 Verify | checks no excluded artifacts leaked and that `.claude-plugin/marketplace.json` + `plugin.json` exist | Plugin install (`/plugin marketplace add takazudo/claude-resources`) depends on the manifests |
| 5-6 Commit | `/co`, push (auto mode picks the push option) | The commit that lands here should name the zfb 3 template fix so external users can find it |

## Step-by-step plan

1. Complete `Takazudo--claude-settings.md` steps 1-5 in `$HOME/.claude` (the private repo) and commit there.
2. Re-share: `/claude-resources-share -a` (auto mode: scan → rsync → `/co` with push). Target is `$HOME/repos/p/claude-resources`; on a machine without that clone, `git clone https://github.com/Takazudo/claude-resources $HOME/repos/p/claude-resources` first.
3. Verify the copy:
   ```sh
   diff -r $HOME/.claude/skills/dev-basic-cloudflare-webapp \
           $HOME/repos/p/claude-resources/skills/dev-basic-cloudflare-webapp && echo IDENTICAL
   grep -rn '"latest"' $HOME/repos/p/claude-resources/skills/dev-basic-cloudflare-webapp/assets/templates/package.json
   grep -rn 'preact\|jsxImportSource' $HOME/repos/p/claude-resources/skills/dev-basic-cloudflare-webapp/
   test -f $HOME/repos/p/claude-resources/.claude-plugin/plugin.json && echo PLUGIN_MANIFEST_OK
   ```
4. Dry-run the published skill the way an external user would: install the plugin into a scratch profile, run the skill in `--scaffold` mode, then `pnpm install && pnpm exec zfb --version && pnpm exec zfb check && pnpm exec zfb wind audit --fail-on error && pnpm exec zfb build && pnpm exec wrangler deploy --dry-run`.
5. If the source fix is delayed: apply only the `"2.22.1"` interim pin in the source and re-share the same day; the public copy is the one strangers scaffold from.
6. After zudo-doc 6.0.0: re-share once the source's docs-site prose is rewritten.

## Verification checklist

- [ ] `diff -r` between the private and public `skills/dev-basic-cloudflare-webapp` is empty after the share.
- [ ] Public `assets/templates/package.json` has no `@takazudo/zfb*: "latest"` and no `preact*` lines.
- [ ] Public `tsconfig.json:9` is `@takazudo/zfb/zudo-react`; `grep -rn 'framework:' skills/dev-basic-cloudflare-webapp` → 0.
- [ ] `.claude-plugin/marketplace.json` and `plugin.json` still present (the share must not delete them).
- [ ] Commit on `claude-resources` `main` is newer than the source commit that fixed the template.
- [ ] A scaffold produced from the plugin passes `zfb check`, `zfb wind audit --fail-on error`, `zfb build`, `wrangler deploy --dry-run`.

## Risks and open questions

- **Public users are already affected.** Anyone who installed the plugin and scaffolded after 2026-09-30 has a project with zfb 3.1.0 + Preact + Tailwind instructions. Consider a short note in the public `README.md` or a release note pointing at https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/.
- **Share lag.** The last share (`8eb08d1`, 2026-09-26) predates the source's latest commit (`939a657`, 2026-09-29) — any other skill changes since then will ride along with the next share; review the diff before pushing.
- **Private-info scan.** The share refuses to proceed on HIGH/MEDIUM findings; the new prose must not add absolute `$HOME/...` paths or account names.
- Open: whether the public skill should recommend `pnpm create zfb@latest` as the site layer (the source guide recommends it); the plugin's external audience makes that the lower-maintenance answer.

## References

- Source-of-truth guide: `Takazudo--claude-settings.md` (same directory); Japanese copy: `Takazudo--cloudflare-app-bootstrap-skill.md`
- Share procedure: `claude-settings/skills/claude-resources-share/SKILL.md` (paths, excludes, plugin-manifest preservation)
- Migration guide: https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/ · Tailwind map: https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/ · Preact map: https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/
- Config: https://zfb.takazudomodular.com/docs/api/define-config/ · wind config: https://zfb.takazudomodular.com/docs/zudo-wind/configuration/ · CLI: https://zfb.takazudomodular.com/docs/api/cli/ · testing: https://zfb.takazudomodular.com/docs/zudo-react/testing/ · adapter: https://zfb.takazudomodular.com/docs/guides/ssr-and-cloudflare-bindings/
- v3 scaffold: `crates/zfb/templates/basic-blog` (Takazudo/zudo-front-builder), `pnpm create zfb@latest`, https://create-zfb.takazudomodular.com
- zudo-doc 6.0.0: zudolab/zudo-doc#4430, #4477, #4473
