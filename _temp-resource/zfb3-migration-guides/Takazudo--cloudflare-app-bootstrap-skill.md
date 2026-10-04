# zfb v3 migration guide: Takazudo/cloudflare-app-bootstrap-skill

Generated 2026-10-04 by an automated diagnosis of `main` @ `aa44132`. Counts come from the commands listed; re-run them locally before relying on them.

This repo is a **standalone Japanese translation** of the `dev-basic-cloudflare-webapp` skill whose English source lives in `Takazudo/claude-settings` (`skills/dev-basic-cloudflare-webapp/`). It holds one directory, `dev-basic-cloudflare-webapp/` (no README, no sync script), last touched 2026-08-19 (`aa44132 update`), six weeks before zfb 3.0.0. Every code line in its templates is identical to the English source; only comments, prose, the `echo`/step-label strings of three shell scripts and the four `// ── … ──` divider keys in `package.json` `scripts` (inert no-op script names) are translated. It therefore has exactly the same defect as the source and must be fixed by **hand-porting** the source's changes — there is no `/claude-resources-share` path for this repo.

## Verdict

**Small, effort S for the interim pin / M for the full translated port, no external gate.** The repo has no zfb project (`find . -name 'zfb.config.*'` → 0; no `.css`/`.tsx`), so nothing runs zfb here and `zfb wind audit` / `zfb check` cannot be executed. The template it ships has produced broken scaffolds since 2026-09-30: `dev-basic-cloudflare-webapp/assets/templates/package.json:40-42` pins `@takazudo/zfb`, `@takazudo/zfb-adapter-cloudflare` and `@takazudo/zfb-runtime` to `"latest"` (= **3.1.0** for all three on 2026-10-04) while installing `preact ^10.29.1` / `preact-render-to-string ^6.6.6` (`:43-44`); `tsconfig.json:9` sets `"jsxImportSource": "preact"`; `SKILL.md:32` prescribes "zfb + Preact + Tailwind v4"; `references/stack.md:22` prescribes `framework: "preact"` and `:54` `@import "tailwindcss"` + `@theme`. On zfb 3 the config keys fail loading and the Tailwind directives are ZW009 (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#configuration, #stylesheets). The work is the same ~3 template files + 1 new shim + 5 reference docs + 1 `SKILL.md` table as in the source, translated. Because the port is manual, the realistic sequence is: apply the **interim pin** (`"2.22.1"` ×3) here today so the Japanese skill matches its own Preact/Tailwind prose, then port the full v3 rewrite from the source once it lands. The docs-site row (`SKILL.md:33`, zudo-doc) stays on zudo-doc 5.28.2 / zfb 2.22.1 until **zudo-doc 6.0.0** (zudolab/zudo-doc#4430, #4477, #4473) and needs a "do not mix with zfb 3" sentence now.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `dev-basic-cloudflare-webapp/assets/templates/` (17 files) | `"latest"` ×3 (`package.json:40-42`) → 3.1.0; scripts `zfb dev`, `zfb build`, `zfb preview`, `typecheck: zfb check`; **no `zfb.config.*`** | none as deps; zudo-doc in prose (`SKILL.md:33`, `stack.md:21`); `@takazudo/zudo-design-token-lint: latest` (`:51`, = 2.1.0) | no CSS, no `tailwind.config.*`, no `@tailwindcss/*`; stale `gitignore:15` `zfb-tailwind-entry-*.css` (JP comment at `:14`); 0 `ZFB_TAILWIND*` | `preact ^10.29.1`, `preact-render-to-string ^6.6.6` (`:43-44`); `tsconfig.json:9` `jsxImportSource: "preact"`; 0 TSX, 0 hooks | none shipped | none | `wrangler.toml` (Workers Static Assets, `main = "./dist/_worker.js"`, `nodejs_compat`; JP comments), `deploy.yml`, `ci.yml`, `run-b4push.sh` (JP step labels) — adapter-facing, **unchanged by v3** |
| `dev-basic-cloudflare-webapp/SKILL.md` + `references/*.md` (Japanese prose) | `stack.md:20` "Preact SSR"; `:32-48` SSR contract (valid) | `SKILL.md:33`; `stack.md:21,58,64-65,143,144` | `SKILL.md:32,36`; `stack.md:54,57,64-65,128,130,145` | `stack.md:22`; `tooling.md:128,138,216-217`; `testing.md:10,48-64`; `conventions.md:23` | `SKILL.md:34` SSR row (valid) | `stack.md:142` | `ci.md`, `cloudflare.md`, `cloudflare-setup.md`: 0 Preact/Tailwind |
| Repo root | only `.git/` and the skill dir; no README, package.json or CI | — | — | — | — | — | n/a |

Measured with: `diff -r /home/user/claude-settings/skills/dev-basic-cloudflare-webapp /home/user/cloudflare-app-bootstrap-skill/dev-basic-cloudflare-webapp` → 21 files differ (14 of the 17 templates, all 6 reference docs, and `SKILL.md`; only `.prettierrc.json`, `prettierignore` and `tsconfig.json` are byte-identical); a comment-stripped diff of the 16 non-Markdown template files → **identical code** except translated `echo`/step-label strings in `scripts/run-b4push.sh`, `scripts/install-git-hooks.sh`, `scripts/hooks/pre-push` and the four translated `// ── … ──` divider keys in `package.json` `scripts`. Briefing greps on `dev-basic-cloudflare-webapp/`: Tailwind directive regex → 3 prose hits (`stack.md:54,64,65`); Preact regex → `stack.md:22`, `tooling.md:138`, `package.json:44`, `tsconfig.json:9`; hooks/`className`/`onClick`/`<Island` → 0; `framework:` → `stack.md:22`. `npm view @takazudo/zfb{,-runtime,-adapter-cloudflare} dist-tags` → `latest: 3.1.0`. `zfb wind audit` / `zfb check` **skipped**: no zfb project dir.

### What a scaffold produced from this repo does today

Derived from the template contents (no install was run here): `pnpm install` resolves the three `@takazudo/zfb*@latest` specs to 3.1.0 and still installs Preact (`package.json:40-44`); the agent follows `stack.md:22,54` and writes `framework: "preact"` plus `@import "tailwindcss"`; `zfb dev`/`zfb build` 3.1.0 then fails at config loading with `the framework key was removed in zfb 3; delete the key. zfb now uses zudo-react.` (or the `tailwind` key variant), next with **ZW009** for each Tailwind directive, next with **ZW006** for every untokenised colour/spacing utility (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#configuration, #stylesheets; https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#theme-and-tokens). `pnpm typecheck` types JSX against Preact (`tsconfig.json:9`) while zudo-react renders it. The Cloudflare half (`wrangler.toml`, `deploy.yml`, `smoke.mjs`) is not involved in the failure.

## Sequencing and blockers

1. **Now, S:** `package.json:40-42` `"latest"` → `"2.22.1"` (interim; matches the existing Japanese Preact/Tailwind prose) **or** straight to `"3.1.0"` if step 3 is done in the same commit. Never leave a scaffold template on `latest`: `SKILL.md:102` ("初回 install 後に `latest` を解決済みバージョンへ置換") runs after the wrong major is already installed, and `pnpm-workspace.yaml` `minimumReleaseAge: 0` removes pnpm 11's 24 h buffer.
2. **Wait for the source rewrite** in `Takazudo/claude-settings` (sibling guide `Takazudo--claude-settings.md`), or do the rewrite here first and back-port — but one of the two copies must be declared canonical; today the English one is newer (`939a657`, 2026-09-29 vs `aa44132`, 2026-08-19) and already differs in non-zfb details (e.g. `SKILL.md` names the Cloudflare plugin skills as `/cloudflare`, `/wrangler` where the source has `/cloudflare:cloudflare`, `/cloudflare:wrangler`).
3. **Port, M:** translate the source's v3 changes (listed below with JP line numbers). No upstream release is needed: zfb 3.1.0, adapter 3.1.0 and `create-zfb` 3.1.0 are on npm `latest`; adapter 3.x has no package-specific change (https://zfb.takazudomodular.com/docs/changelog/zfb-adapter-cloudflare/v3.0.0/).
4. **After zudo-doc 6.0.0** (zudolab/zudo-doc#4430 / #4477 / consumer guide #4473; blocked on zfb fixes that are merged on `main` but unreleased, so 6.0.0 targets the **next zfb release**): rewrite `SKILL.md:33`, `stack.md:21,64-65,143`, `conventions.md:21,90-98`. Until then add: "zudo-doc 5.28.x は zfb **2.22.1** 前提。同じパッケージで zfb 3.x と混ぜない。"
5. **Next zfb release** (label only): `wind.strict`, `wind.utilities.placement`, `zfb wind manifest`, `--json` audit — nothing in this template needs them.

## Required changes

Line numbers below are for **this repo's Japanese files**; the English equivalents are in `Takazudo--claude-settings.md`.

### 1. Dependencies, config, tsconfig, env

- `dev-basic-cloudflare-webapp/assets/templates/package.json:40-42` — `"latest"` → `"3.1.0"` exact (lockstep; interim `"2.22.1"`).
- `package.json:43-44` — delete `preact`, `preact-render-to-string`; keep `@takazudo/zfb-runtime` (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#components-and-islands).

  ```jsonc
  "dependencies": {
    "@takazudo/zfb": "3.1.0",
    "@takazudo/zfb-adapter-cloudflare": "3.1.0",
    "@takazudo/zfb-runtime": "3.1.0",
    "zod": "^4.3.6"
  }
  ```

- `tsconfig.json:9` — `"jsxImportSource": "@takazudo/zfb/zudo-react"` (`:8` `"jsx": "react-jsx"` stays). Consider adding `"mdx-components.tsx"`, `"content/**/*"` to `include` (`:22-29`).
- New `assets/templates/components/zfb-shim.d.ts`:

  ```ts
  declare module "zfb/config" {
    export * from "@takazudo/zfb/config";
  }
  ```

  (https://zfb.takazudomodular.com/docs/api/define-config/#typing-the-zfb-config-import; copy of `crates/zfb/templates/basic-blog/components/zfb-shim.d.ts`; comments may be translated.)
- New `assets/templates/zfb.config.ts` — `adapter: "@takazudo/zfb-adapter-cloudflare"` plus a `wind` block with `spec: 1`, an explicit `reset` (`"owned-v1"` in the v3 scaffold), semantic `tokens.colors` pointing at authored custom properties, `tokens.spacingUnit`, `breakpoints`, `dark`, `authoredClasses` (https://zfb.takazudomodular.com/docs/zudo-wind/configuration/). No `framework`, no `tailwind` key. Alternative: let scaffold mode generate it via `pnpm create zfb@latest` and add only the `adapter` line.
- `gitignore:14-15` — delete the JP comment ("異常終了時に取り残される zfb の一時ファイル") and `zfb-tailwind-entry-*.css`; that temp file belongs to zfb ≤ 2.x (0 hits in v3 source).
- `wrangler.toml`, workflows, env — unchanged; 0 `ZFB_TAILWIND*` (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#cli-and-environment); adapter config and `nodejs_compat` match https://zfb.takazudomodular.com/docs/guides/ssr-and-cloudflare-bindings/#configuring-the-cloudflare-adapter.

### 2. CSS and utilities

- `references/stack.md:54` `| ユーティリティ CSS | **Tailwind CSS v4**（`@import "tailwindcss"`、`@theme` ブロック） |` → zudo-wind（zfb 3 内蔵。`zfb.config.ts` の `wind` キーで設定、値は `wind.tokens`、CSS ディレクティブなし）. Every Tailwind directive/import is ZW009 on zfb 3, even under `wind: false` (https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#directives). Tailwind v4 via `@tailwindcss/vite` remains correct **only** for the Vite + React row (`SKILL.md:36` インタラクティブアプリ) — that path does not go through zfb; say so.
- `stack.md:57` token enforcement row — keep; `:60-62` region: v3 ships no palette/spacing scale, so the semantic tokens *are* the `wind.tokens` map (https://zfb.takazudomodular.com/docs/zudo-wind/tokens/).
- `stack.md:64-65` "zudo-doc 配下では Tailwind は zfb の内蔵エンジンがコンパイルする…" → true only for zudo-doc 5.x / zfb 2.22.1; add the version lock now, rewrite after 6.0.0 (wind.json manifest).
- `stack.md:128` "Tailwind v4 または CSS Modules" → "zudo-wind ユーティリティまたは CSS Modules（Tailwind v4 は Vite SPA のみ）"; `:130` and `:145` "Tailwind トークン Linter" → "ユーティリティクラス名のトークン Linter（Tailwind 流の綴り、zudo-wind 候補にも適用）".
- Add the unsupported list (`ring-*`, `animate-*`, `scale-*`, arbitrary properties, `!important`, `aria-*`/`data-[…]` variants, named `group/peer`, `@apply`; ZW014 warnings for `line-clamp-*`, `container`, `order-*`, `basis-*`; error under `wind.strict` — **next zfb release**) — https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#what-is-not-supported-and-what-to-do-instead, https://zfb.takazudomodular.com/docs/zudo-wind/diagnostics-and-tools/#migration-warnings-and-strict-mode.

### 3. Components and islands

- `SKILL.md:32` `zfb + Preact + Tailwind v4` → `zfb 3（zudo-react + zudo-wind）`.
- `stack.md:20` "Preact SSR" → "zudo-react SSR"; `stack.md:22` whole row → zudo-react（`@takazudo/zfb/zudo-react`、`@takazudo/zfb` に同梱）: setup-once components, `signal()`/`computed()`, `Show`/`For`, native `on:click`, HTML spellings `class`/`for`; no Preact/React, no compat aliases; Preact/React widgets embedded opaquely inside an island (https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/#hook-by-hook, https://zfb.takazudomodular.com/docs/concepts/islands/#embedding-a-third-party-widget).
- `stack.md:23` keep Vite + React 19 for genuinely interactive apps; strengthen with "zudo-react has no hooks/context/portals/forwardRef/lazy/Suspense/class components" (https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/#what-does-not-exist).
- `tooling.md:128` "zfb/Preact プロジェクト" → "zfb 3 プロジェクト"; `:138` `jsxImportSource: "@takazudo/zfb/zudo-react"`; mention `components/zfb-shim.d.ts`.
- `tooling.md:216-217` `eslint-plugin-react` + `eslint-plugin-react-hooks` → Vite + React のみ。zfb 3 では追加しない（hooks が存在せず、React 向けルールは `class`/`for`/`on:click` の zudo-react 方言を誤検出する）.
- `conventions.md:23` `components/   Preact / React コンポーネント` → `zudo-react コンポーネント（アイランドは "use client" ファイルを <Island> で包む）`.
- `SKILL.md:34` SSR row and `stack.md:32-48` SSR contract — unchanged (`prerender = false`, `getCloudflareContext<Env>()`, `adapter`).

### 4. md-wasm and other packages

- `stack.md:142` `@takazudo/zfb-md-wasm` row — add: 3.x では `jsxRuntime` オプションなし。`compile()` 出力は `@takazudo/zfb/zudo-react/jsx-runtime` を import するので、React/Preact（Vite SPA）側で使うなら `renderHtml()` か md-wasm 2.x (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#mdx-and-md-wasm).
- `SKILL.md:33`, `stack.md:21,143` zudo-doc — add the 5.28.x ↔ zfb 2.22.1 lock and 6.0.0 status (zudolab/zudo-doc#4430); `stack.md:58,144` zdtp rows — Preact widget, embedded via the widget pattern on v3 (Takazudo/zudo-design-token-panel#1002).

### 5. Tests, CI, deploy

- `testing.md:10` `vitest + @testing-library/preact（または /react）` → `vitest + happy-dom + @takazudo/zfb/zudo-react/testing`（`createIslandTest`、`@takazudo/zfb/zudo-react/server` の `renderToString`）(https://zfb.takazudomodular.com/docs/zudo-react/testing/#hydrate-and-mount-an-island).
- `testing.md:48-67` "React 互換モードの Preact エイリアス" block → delete and replace with the v3 runner config (https://zfb.takazudomodular.com/docs/zudo-react/testing/#configure-jsx-and-a-dom):

  ```ts
  // vitest.config.ts
  import { defineConfig } from "vitest/config";
  export default defineConfig({
    esbuild: { jsx: "automatic", jsxImportSource: "@takazudo/zfb/zudo-react" },
    test: { environment: "happy-dom" },
  });
  ```

  Keep the following paragraph (projects do not inherit the root config).
- `ci.yml` (`:46-47` Build step) and `scripts/run-b4push.sh` (`:34-39`, JP labels): add `pnpm exec zfb wind audit --fail-on error` after ビルド, identically in both (https://zfb.takazudomodular.com/docs/api/cli/#zfb-wind). `pnpm typecheck` (`zfb check`) already catches removed config keys (https://zfb.takazudomodular.com/docs/api/cli/#zfb-check).
- `deploy.yml`, `actionlint.yml`, `smoke.mjs`, `wrangler.toml`, `cloudflare-setup.md`, `references/ci.md`, `references/cloudflare.md` — no change.

## Step-by-step plan

1. **Today, interim** (if the full port cannot land immediately): edit `dev-basic-cloudflare-webapp/assets/templates/package.json:40-42` to `"2.22.1"`; commit "pin zfb to the last 2.x until the v3 port".
   `grep -n '"latest"' dev-basic-cloudflare-webapp/assets/templates/package.json`
2. **Port the template files** from the source's v3 commit: `package.json` (deps → `3.1.0`, drop Preact), `tsconfig.json:9`, `gitignore:14-15`, new `components/zfb-shim.d.ts`, new `zfb.config.ts` (+ `styles/global.css` if the source adds one). Translate comments only.
3. **Port the prose** (JP line numbers in Required changes 2-5): `SKILL.md:32,33,102`; `stack.md:20,21,22,54,57,58,64-65,128,130,142,143,144,145`; `tooling.md:128,138,216-217`; `testing.md:10,48-64`; `conventions.md:21,23`. Add the zudo-doc lock sentence and the zudo-wind unsupported list.
4. **Update the Scaffolding table** (`SKILL.md:100-115`) with the new template rows and, if the source adopts it, the `pnpm create zfb@latest <name>` → overlay flow (https://zfb.takazudomodular.com/docs/getting-started/your-first-site/; note the pnpm 11 `minimumReleaseAge` caveat).
5. **Dry-run** the Japanese skill in a scratch directory:
   ```sh
   cd "$(mktemp -d)" && pnpm create zfb@latest smoke-site && cd smoke-site
   # overlay dev-basic-cloudflare-webapp/assets/templates/, replace __PROJECT_NAME__/__DOMAIN__, then:
   pnpm exec zfb --version && pnpm exec zfb check
   pnpm exec zfb wind audit --fail-on error
   pnpm exec zfb build && pnpm exec wrangler deploy --dry-run
   bash scripts/run-b4push.sh
   ```
6. **Record the relationship**: add a `README.md` at the repo root so the next drift is visible, e.g.

   ```md
   # cloudflare-app-bootstrap-skill
   `dev-basic-cloudflare-webapp` の日本語版。英語の原本は Takazudo/claude-settings の
   `skills/dev-basic-cloudflare-webapp/`（追従コミット: <sha>）。コードは原本と同一、コメントと文章のみ翻訳。
   zfb 3 対応: <date>。
   ```
7. **After zudo-doc 6.0.0**: port the source's docs-site rewrite.

## Verification checklist

- [ ] `grep -n '"latest"' dev-basic-cloudflare-webapp/assets/templates/package.json` shows no `@takazudo/zfb*` line.
- [ ] `grep -rn 'preact' dev-basic-cloudflare-webapp/assets/templates/` → 0.
- [ ] `grep -rn 'jsxImportSource' dev-basic-cloudflare-webapp/` → only `@takazudo/zfb/zudo-react`.
- [ ] `grep -rn 'framework:' dev-basic-cloudflare-webapp/` → 0; `grep -rnE 'Tailwind|tailwind'` hits only the Vite + React row and the ZW009 warning text.
- [ ] Comment-stripped diff of the 16 non-Markdown templates against the English source is empty except the translated `echo`/step-label strings in the 3 shell scripts and the `// ── … ──` divider keys in `package.json`.
- [ ] Dry-run scaffold passes `zfb check`, `zfb wind audit --fail-on error`, `zfb build`, `wrangler deploy --dry-run`, `run-b4push.sh`.
- [ ] `SKILL.md:33` carries the zudo-doc 5.28.x ↔ zfb 2.22.1 lock until 6.0.0.

## Risks and open questions

- **No sync mechanism.** This copy is already 6 weeks behind the source in non-zfb details; a v3 rewrite in the source without a scheduled port makes the Japanese skill wrong in a second way. Decide whether to keep it, generate it from the source (translation step in `/claude-resources-share`-like tooling), or archive it with a pointer.
- **Two majors in one workspace.** Projects scaffolded with a 3.x root site plus a `doc/` zudo-doc 5.x package carry zfb 2.22.1 and 3.1.0 side by side; `pnpm-workspace.yaml` `linkWorkspacePackages: false` and per-package lockfile hygiene must be explained in Japanese too.
- **design-token-lint 2.1.0** describes itself as a Tailwind linter; its regexes apply to zudo-wind spellings, but confirm in the dry run that it accepts `size-8`, `tracking-[0.08em]`, `dark:bg-sky-950/40`.
- **tsconfig alias** (`@/*` → `src/*` here vs `~/*` → root in basic-blog): the overlay must pick one or `zfb check` fails on the first import.
- Open: is this repo still consumed anywhere (no README, no package manifest, no stars/forks recorded in the clone)? If not, archiving with a link to the English source is cheaper than maintaining a second migration.

## References

- Source-of-truth guide: `Takazudo--claude-settings.md`; public mirror guide: `Takazudo--claude-resources.md` (same directory)
- Migration guide: https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/ · Tailwind map: https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/ · Preact hooks map: https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/
- wind config/tokens/diagnostics: https://zfb.takazudomodular.com/docs/zudo-wind/configuration/, https://zfb.takazudomodular.com/docs/zudo-wind/tokens/, https://zfb.takazudomodular.com/docs/zudo-wind/diagnostics-and-tools/
- Config/CLI/testing/islands: https://zfb.takazudomodular.com/docs/api/define-config/, https://zfb.takazudomodular.com/docs/api/cli/, https://zfb.takazudomodular.com/docs/zudo-react/testing/, https://zfb.takazudomodular.com/docs/concepts/islands/#embedding-a-third-party-widget
- Cloudflare adapter: https://zfb.takazudomodular.com/docs/guides/ssr-and-cloudflare-bindings/ · changelogs: https://zfb.takazudomodular.com/docs/changelog/zfb/v3.0.0/, https://zfb.takazudomodular.com/docs/changelog/zfb-adapter-cloudflare/v3.0.0/
- v3 scaffold: `crates/zfb/templates/basic-blog` (Takazudo/zudo-front-builder), `pnpm create zfb@latest`, https://create-zfb.takazudomodular.com
- zudo-doc 6.0.0: zudolab/zudo-doc#4430, #4477, #4473 · zdtp: Takazudo/zudo-design-token-panel#1002
